//! A file the engine reads that a test can make fail, change or slow down while the document is open.

#![allow(dead_code)]

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use partridge_engine::source::{SourceFile, BLOCK};

/// No block fails.
pub const NO_BLOCK: u64 = u64::MAX;

/// What a test does to the file while the engine has it open.
#[derive(Clone)]
pub struct Controls {
    /// The bytes of the file; a test may change them.
    pub data: Arc<Mutex<Vec<u8>>>,
    /// The block (of 32 KiB) whose reads fail, or [`NO_BLOCK`].
    pub fail_block: Arc<AtomicU64>,
    /// Every read takes a tenth of a second.
    pub slow: Arc<AtomicBool>,
    /// When the last slow read began (reads made while [`Controls::slow`] was set).
    pub last_slow_read: Arc<Mutex<Option<Instant>>>,
}

impl Controls {
    pub fn fail_reads_of_block(&self, block: u64) {
        self.fail_block.store(block, Ordering::SeqCst);
    }

    pub fn reads_work_again(&self) {
        self.fail_block.store(NO_BLOCK, Ordering::SeqCst);
    }
}

pub struct ControlledFile {
    controls: Controls,
}

impl ControlledFile {
    pub fn new(bytes: &[u8]) -> (ControlledFile, Controls) {
        let controls = Controls {
            data: Arc::new(Mutex::new(bytes.to_vec())),
            fail_block: Arc::new(AtomicU64::new(NO_BLOCK)),
            slow: Arc::new(AtomicBool::new(false)),
            last_slow_read: Arc::new(Mutex::new(None)),
        };
        (
            ControlledFile {
                controls: controls.clone(),
            },
            controls,
        )
    }
}

impl SourceFile for ControlledFile {
    fn read_at(&mut self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        if self.controls.slow.load(Ordering::SeqCst) {
            *self.controls.last_slow_read.lock().unwrap() = Some(Instant::now());
            std::thread::sleep(Duration::from_millis(100));
        }
        if pos / BLOCK == self.controls.fail_block.load(Ordering::SeqCst) {
            return Err(io::Error::other("the share dropped this read"));
        }
        let data = self.controls.data.lock().unwrap();
        let pos = (pos as usize).min(data.len());
        let n = buf.len().min(data.len() - pos);
        buf[..n].copy_from_slice(&data[pos..pos + n]);
        Ok(n)
    }

    fn state(&mut self) -> io::Result<(u64, Option<SystemTime>)> {
        Ok((self.controls.data.lock().unwrap().len() as u64, None))
    }

    fn is_plain_file(&mut self) -> bool {
        true
    }
}
