//! The file a document is read from, watched while it is open.
//!
//! PDFium does not read a document into memory: it reads the parts it needs, when it needs them,
//! from the handle the app gave the engine. That is fine for showing pages. It is not fine for a
//! save, because a save reads everything that is not in memory yet and writes it out again: if
//! the file changed in place since the document was opened (the handle sees the new bytes), the
//! parts read at different times come from different versions of the file, and the result is a
//! file that neither version made. A read that fails (a network share drops, a stick is pulled)
//! is worse: PDFium leaves the unreadable object out and goes on, and the save succeeds.
//!
//! So every read of PDFium goes through [`GuardedReader`], which
//!
//! - reads the file in blocks of 32 KiB and remembers a digest of every block it has read, and
//!   notes when a block that is read a second time has other bytes;
//! - counts every read that fails;
//! - lets the engine ask, before and after a save, whether the size and the time of change of the
//!   handle are still what they were, whether every block that was read still has the same
//!   digest, and whether any read failed.
//!
//! The digests catch a change in the parts that were read, also when the size and the time of
//! change were put back. A new time of change with the same size is accepted once every block of
//! the file has been read and still has its digest (a program that only touched the file). If
//! parts were never read, they cannot be compared with anything: nothing seen differs, but the
//! file may. The guard then says the file was *touched* (see [`SourceGuard::touched`]) and keeps
//! saying so. A document in that state may be written to a new file, so that the user's edits can
//! always leave the app, but not over the original: that is for the app to enforce. A change in a
//! part that was never read is otherwise not seen (nothing of the document depends on it).

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

/// Size of the blocks the file is read in.
pub const BLOCK: u64 = 32 * 1024;

/// How many blocks the guard reads on its own between two signs of life (1 MiB). A read of the
/// whole file on a slow link can take minutes, and the app takes an engine that says nothing for
/// too long for hung.
pub const REPORT_BLOCKS: u64 = 32;

/// A file that can be read at a place. A plain file is the only kind the app hands over; the
/// trait is there so that a test can make reads fail.
pub trait SourceFile: Send {
    /// Reads up to `buf.len()` bytes from `pos`; fewer only at the end of the file.
    fn read_at(&mut self, pos: u64, buf: &mut [u8]) -> io::Result<usize>;
    /// The size and the time of last change, as the handle sees them now.
    fn state(&mut self) -> io::Result<(u64, Option<SystemTime>)>;
    /// False for a folder, a pipe or a device.
    fn is_plain_file(&mut self) -> bool;
}

impl SourceFile for File {
    fn read_at(&mut self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.seek(SeekFrom::Start(pos))?;
        let mut done = 0;
        while done < buf.len() {
            match self.read(&mut buf[done..]) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }

    fn state(&mut self) -> io::Result<(u64, Option<SystemTime>)> {
        let meta = self.metadata()?;
        Ok((meta.len(), meta.modified().ok()))
    }

    fn is_plain_file(&mut self) -> bool {
        self.metadata().map(|m| m.is_file()).unwrap_or(false)
    }
}

/// What is wrong with the file a document comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// A read failed at some time since the document was opened.
    Unreadable,
    /// The file is not what it was when parts of it were read.
    Changed,
}

/// FNV-1a over 64 bits: not a defence against an attacker (the one who changes the file is a
/// program that saves it, not an adversary of the digest), only a way to see that bytes differ.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

pub struct SourceGuard {
    file: Box<dyn SourceFile>,
    len: u64,
    modified: Option<SystemTime>,
    /// Digest of every block that was read, by block number.
    blocks: HashMap<u64, u64>,
    cache: Option<(u64, Vec<u8>)>,
    failures: u32,
    changed: bool,
    /// The time of change moved while some blocks had never been read; kept for good, because the
    /// blocks read later have no earlier digest to be compared with.
    touched: bool,
    /// A block was read for the first time after the time of change moved: it has no digest from
    /// before, so it proves nothing about the file as it was.
    late: bool,
}

impl SourceGuard {
    /// Starts to watch `file`, as it is now.
    pub fn new(mut file: Box<dyn SourceFile>) -> io::Result<SourceGuard> {
        let (len, modified) = file.state()?;
        Ok(SourceGuard {
            file,
            len,
            modified,
            blocks: HashMap::new(),
            cache: None,
            failures: 0,
            changed: false,
            touched: false,
            late: false,
        })
    }

    /// True when the time of change moved while parts of the file were unread: the bytes that were
    /// read are as they were, but the rest of the file cannot be said to be.
    pub fn touched(&self) -> bool {
        self.touched || self.late
    }

    /// How many reads failed so far.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// True when a read of PDFium's failed, or a block that was read twice differed: what PDFium
    /// made of the file may lack a part of it. (PDFium leaves out what it cannot read and goes on.)
    pub fn damaged(&self) -> bool {
        self.failures > 0 || self.changed
    }

    fn fail<T>(&mut self, e: io::Error) -> io::Result<T> {
        self.failures = self.failures.saturating_add(1);
        Err(e)
    }

    /// Makes block `index` the block in the cache, reading it if it is not there. `for_pdfium`:
    /// the read is one PDFium asked for, so a failure is on record for good (PDFium leaves out
    /// what it cannot read and goes on). The guard's own reads (the scan for signatures) are
    /// repeated by the next look, so a failure there is only an error.
    fn load(&mut self, index: u64, for_pdfium: bool) -> io::Result<()> {
        if matches!(&self.cache, Some((i, _)) if *i == index) {
            return Ok(());
        }
        let mut data = vec![0u8; BLOCK as usize];
        let got = match self.file.read_at(index * BLOCK, &mut data) {
            Ok(n) => n,
            Err(e) => {
                self.cache = None;
                return if for_pdfium { self.fail(e) } else { Err(e) };
            }
        };
        data.truncate(got);
        let digest = fnv(&data);
        match self.blocks.get(&index) {
            Some(known) if *known != digest => self.changed = true,
            Some(_) => {}
            None => {
                self.blocks.insert(index, digest);
                if matches!(self.file.state(), Ok((_, modified)) if modified != self.modified) {
                    self.late = true;
                }
            }
        }
        self.cache = Some((index, data));
        Ok(())
    }

    /// Reads from `pos` into `buf`, as far as the file goes.
    fn read(&mut self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        let mut done = 0;
        while done < buf.len() {
            let at = pos + done as u64;
            self.load(at / BLOCK, true)?;
            let (_, block) = self.cache.as_ref().expect("just loaded");
            let from = (at % BLOCK) as usize;
            if from >= block.len() {
                break;
            }
            let n = (block.len() - from).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&block[from..from + n]);
            done += n;
        }
        Ok(done)
    }

    /// Looks at the file again. `Ok` only if nothing is wrong: no read failed, the size is as it
    /// was, and every block read so far has the digest it had. A new time of change alone is not
    /// a change (a program that only touched the file moved the time and changed nothing), but if
    /// parts of the file were never read it is remembered (see [`SourceGuard::touched`]). A failure
    /// of this look itself is an error for now, not a mark against the file: the next look is a
    /// new try. `progress` is told how many blocks are read, about every
    /// MiB.
    pub fn verify(&mut self, progress: &mut dyn FnMut(u32)) -> Result<(), Problem> {
        if self.failures > 0 {
            return Err(Problem::Unreadable);
        }
        let time_moved = match self.file.state() {
            Ok((len, modified)) => {
                if len != self.len {
                    return Err(Problem::Changed);
                }
                (modified != self.modified).then_some(modified)
            }
            Err(_) => return Err(Problem::Unreadable),
        };
        if self.changed {
            return Err(Problem::Changed);
        }
        self.cache = None;
        let mut known: Vec<(u64, u64)> = self.blocks.iter().map(|(i, d)| (*i, *d)).collect();
        known.sort_unstable();
        let mut data = vec![0u8; BLOCK as usize];
        for (count, (index, digest)) in known.into_iter().enumerate() {
            if count as u64 % REPORT_BLOCKS == REPORT_BLOCKS - 1 {
                progress(count as u32 + 1);
            }
            let got = match self.file.read_at(index * BLOCK, &mut data) {
                Ok(n) => n,
                Err(_) => return Err(Problem::Unreadable),
            };
            if fnv(&data[..got]) != digest {
                self.changed = true;
                return Err(Problem::Changed);
            }
        }
        if let Some(modified) = time_moved {
            // Only the time moved, and every block that was read has the digest it had: a program
            // that touched the file, or saved the same bytes again. With every block known, the
            // file is what was read. If not, a part that was never read cannot be compared with
            // anything: the file is marked, and may be copied but not replaced. The new time is the
            // one to compare with from now on.
            let all_read = (0..self.len.div_ceil(BLOCK)).all(|i| self.blocks.contains_key(&i));
            if !all_read {
                self.touched = true;
            }
            self.modified = modified;
        }
        if self.late {
            self.touched = true;
            self.late = false;
        }
        Ok(())
    }

    /// True when any of `needles` is in the file. Reads the whole file (and so notes every block).
    /// `progress` is told how many blocks are read, about every MiB.
    pub fn contains_any(
        &mut self,
        needles: &[&[u8]],
        progress: &mut dyn FnMut(u32),
    ) -> io::Result<bool> {
        let longest = needles.iter().map(|n| n.len()).max().unwrap_or(0);
        let mut tail: Vec<u8> = Vec::new();
        let mut index = 0u64;
        loop {
            self.load(index, false)?;
            let (_, block) = self.cache.as_ref().expect("just loaded");
            if block.is_empty() {
                return Ok(false);
            }
            let mut window = std::mem::take(&mut tail);
            window.extend_from_slice(block);
            if needles
                .iter()
                .any(|needle| window.windows(needle.len()).any(|w| w == *needle))
            {
                return Ok(true);
            }
            let keep = longest.saturating_sub(1).min(window.len());
            tail = window[window.len() - keep..].to_vec();
            if (block.len() as u64) < BLOCK {
                return Ok(false);
            }
            index += 1;
            if index.is_multiple_of(REPORT_BLOCKS) {
                progress(index as u32);
            }
        }
    }
}

/// The reader PDFium gets. All readers of one document share the guard.
pub struct GuardedReader {
    guard: Arc<Mutex<SourceGuard>>,
    pos: u64,
}

impl GuardedReader {
    pub fn new(guard: Arc<Mutex<SourceGuard>>) -> GuardedReader {
        GuardedReader { guard, pos: 0 }
    }
}

pub fn lock(guard: &Mutex<SourceGuard>) -> MutexGuard<'_, SourceGuard> {
    guard.lock().unwrap_or_else(|e| e.into_inner())
}

impl Read for GuardedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = lock(&self.guard).read(self.pos, buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for GuardedReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => {
                let len = lock(&self.guard).file.state()?.0;
                len.checked_add_signed(d)
            }
        };
        match target {
            Some(n) => {
                self.pos = n;
                Ok(n)
            }
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a seek to before the start of the file",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A file in memory that can change under the guard and can be made to fail.
    struct Memory {
        data: Arc<Mutex<Vec<u8>>>,
        broken: Arc<AtomicBool>,
        modified: Arc<Mutex<Option<SystemTime>>>,
    }

    impl SourceFile for Memory {
        fn read_at(&mut self, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
            if self.broken.load(Ordering::SeqCst) {
                return Err(io::Error::other("the share is gone"));
            }
            let data = self.data.lock().unwrap();
            let pos = (pos as usize).min(data.len());
            let n = buf.len().min(data.len() - pos);
            buf[..n].copy_from_slice(&data[pos..pos + n]);
            Ok(n)
        }

        fn state(&mut self) -> io::Result<(u64, Option<SystemTime>)> {
            Ok((
                self.data.lock().unwrap().len() as u64,
                *self.modified.lock().unwrap(),
            ))
        }

        fn is_plain_file(&mut self) -> bool {
            true
        }
    }

    struct Rig {
        data: Arc<Mutex<Vec<u8>>>,
        broken: Arc<AtomicBool>,
        modified: Arc<Mutex<Option<SystemTime>>>,
        guard: Arc<Mutex<SourceGuard>>,
    }

    fn rig(len: usize) -> Rig {
        let data = Arc::new(Mutex::new(
            (0..len).map(|i| (i % 251) as u8).collect::<Vec<u8>>(),
        ));
        let broken = Arc::new(AtomicBool::new(false));
        let modified = Arc::new(Mutex::new(Some(SystemTime::UNIX_EPOCH)));
        let memory = Memory {
            data: data.clone(),
            broken: broken.clone(),
            modified: modified.clone(),
        };
        let guard = Arc::new(Mutex::new(SourceGuard::new(Box::new(memory)).unwrap()));
        Rig {
            data,
            broken,
            modified,
            guard,
        }
    }

    #[test]
    fn the_reader_gives_the_bytes_of_the_file_across_blocks_and_to_the_end() {
        let rig = rig(100_000);
        let mut reader = GuardedReader::new(rig.guard.clone());
        reader.seek(SeekFrom::Start(32_760)).unwrap();
        let mut buf = vec![0u8; 20];
        reader.read_exact(&mut buf).unwrap();
        let expected: Vec<u8> = (32_760..32_780).map(|i| (i % 251) as u8).collect();
        assert_eq!(buf, expected);
        assert_eq!(reader.seek(SeekFrom::End(-5)).unwrap(), 99_995);
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).unwrap();
        assert_eq!(rest.len(), 5);
        // Past the end there is nothing.
        reader.seek(SeekFrom::Start(200_000)).unwrap();
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
        assert!(reader.seek(SeekFrom::Current(-300_000)).is_err());
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Ok(()));
    }

    #[test]
    fn a_change_of_size_is_seen() {
        let longer = rig(100_000);
        let mut reader = GuardedReader::new(longer.guard.clone());
        let mut buf = [0u8; 10];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(lock(&longer.guard).verify(&mut |_| {}), Ok(()));
        longer.data.lock().unwrap().push(1);
        assert_eq!(
            lock(&longer.guard).verify(&mut |_| {}),
            Err(Problem::Changed)
        );
    }

    fn later(secs: u64) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
    }

    #[test]
    fn a_touch_before_the_whole_file_is_known_is_passed_but_marked() {
        let touched = rig(100_000);
        let mut reader = GuardedReader::new(touched.guard.clone());
        let mut buf = [0u8; 10];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(lock(&touched.guard).verify(&mut |_| {}), Ok(()));
        assert!(!lock(&touched.guard).touched());
        // Most of the file was never read, and the time of change is new: the blocks that were
        // read have their digests, but the rest cannot be compared with anything.
        *touched.modified.lock().unwrap() = later(5);
        assert_eq!(lock(&touched.guard).verify(&mut |_| {}), Ok(()));
        assert!(lock(&touched.guard).touched());
        // Reading the rest later does not clear the mark: those blocks have no earlier digest.
        assert!(!lock(&touched.guard)
            .contains_any(&[b"/ByteRange"], &mut |_| {})
            .unwrap());
        assert_eq!(lock(&touched.guard).verify(&mut |_| {}), Ok(()));
        assert!(lock(&touched.guard).touched());
    }

    #[test]
    fn a_touch_when_every_block_is_known_is_not_marked() {
        let rig = read_through(40);
        *rig.modified.lock().unwrap() = later(5);
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Ok(()));
        assert!(!lock(&rig.guard).touched());
    }

    #[test]
    fn a_block_read_for_the_first_time_after_a_touch_is_marked() {
        let rig = rig(100_000);
        let mut reader = GuardedReader::new(rig.guard.clone());
        let mut buf = [0u8; 10];
        reader.read_exact(&mut buf).unwrap();
        *rig.modified.lock().unwrap() = later(5);
        // The rest is read before anyone looked at the time: it has no digest from before.
        assert!(!lock(&rig.guard)
            .contains_any(&[b"/ByteRange"], &mut |_| {})
            .unwrap());
        assert!(lock(&rig.guard).touched());
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Ok(()));
        assert!(lock(&rig.guard).touched());
    }

    #[test]
    fn other_bytes_or_another_size_are_a_change_touched_or_not() {
        let touched = rig(100_000);
        let mut reader = GuardedReader::new(touched.guard.clone());
        let mut buf = [0u8; 10];
        reader.read_exact(&mut buf).unwrap();
        touched.data.lock().unwrap()[3] ^= 0xff;
        *touched.modified.lock().unwrap() = later(5);
        assert_eq!(
            lock(&touched.guard).verify(&mut |_| {}),
            Err(Problem::Changed)
        );
        let longer = rig(100_000);
        let mut reader = GuardedReader::new(longer.guard.clone());
        reader.read_exact(&mut buf).unwrap();
        longer.data.lock().unwrap().push(1);
        *longer.modified.lock().unwrap() = later(5);
        assert_eq!(
            lock(&longer.guard).verify(&mut |_| {}),
            Err(Problem::Changed)
        );
    }

    #[test]
    fn a_touch_that_changes_only_the_time_is_not_a_change_once_the_whole_file_is_known() {
        let rig = read_through(40);
        *rig.modified.lock().unwrap() =
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(5));
        // Every block was read and has the digest it had: the file is what was read.
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Ok(()));
        // The new time is the one to compare with from now on, and a real change is still seen.
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Ok(()));
        rig.data.lock().unwrap()[100] ^= 0xff;
        *rig.modified.lock().unwrap() =
            Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(9));
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Err(Problem::Changed));
    }

    #[test]
    fn a_failure_of_the_guards_own_look_is_not_held_against_the_file() {
        let rig = read_through(40);
        rig.broken.store(true, Ordering::SeqCst);
        // The look fails now ...
        assert_eq!(
            lock(&rig.guard).verify(&mut |_| {}),
            Err(Problem::Unreadable)
        );
        let mut again = SourceGuard::new(Box::new(Memory {
            data: rig.data.clone(),
            broken: rig.broken.clone(),
            modified: rig.modified.clone(),
        }))
        .unwrap();
        assert!(again.contains_any(&[b"/ByteRange"], &mut |_| {}).is_err());
        assert_eq!(again.failures(), 0);
        // ... and works again when the share is back; PDFium lost nothing in between.
        rig.broken.store(false, Ordering::SeqCst);
        assert_eq!(lock(&rig.guard).failures(), 0);
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Ok(()));
        assert!(!again.contains_any(&[b"/ByteRange"], &mut |_| {}).unwrap());
    }

    #[test]
    fn a_change_of_a_block_that_was_read_is_seen_even_when_size_and_time_are_the_same() {
        let rig = rig(100_000);
        let mut reader = GuardedReader::new(rig.guard.clone());
        let mut buf = [0u8; 10];
        reader.seek(SeekFrom::Start(40_000)).unwrap();
        reader.read_exact(&mut buf).unwrap();
        // A block that was never read may change without anyone noticing: nothing depends on it.
        rig.data.lock().unwrap()[90_000] ^= 0xff;
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Ok(()));
        rig.data.lock().unwrap()[40_001] ^= 0xff;
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Err(Problem::Changed));
    }

    #[test]
    fn a_block_that_is_read_again_with_other_bytes_is_noted_at_once() {
        let rig = rig(100_000);
        let mut reader = GuardedReader::new(rig.guard.clone());
        let mut buf = [0u8; 10];
        reader.read_exact(&mut buf).unwrap();
        // Another block in between, so the first one is read again from the file.
        reader.seek(SeekFrom::Start(70_000)).unwrap();
        reader.read_exact(&mut buf).unwrap();
        rig.data.lock().unwrap()[3] ^= 0xff;
        reader.seek(SeekFrom::Start(0)).unwrap();
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(lock(&rig.guard).verify(&mut |_| {}), Err(Problem::Changed));
    }

    #[test]
    fn a_failed_read_is_counted_and_never_forgotten() {
        let rig = rig(100_000);
        let mut reader = GuardedReader::new(rig.guard.clone());
        let mut buf = [0u8; 10];
        reader.read_exact(&mut buf).unwrap();
        rig.broken.store(true, Ordering::SeqCst);
        reader.seek(SeekFrom::Start(60_000)).unwrap();
        assert!(reader.read(&mut buf).is_err());
        assert_eq!(lock(&rig.guard).failures(), 1);
        // The share comes back: the failure stays on record.
        rig.broken.store(false, Ordering::SeqCst);
        assert_eq!(
            lock(&rig.guard).verify(&mut |_| {}),
            Err(Problem::Unreadable)
        );
    }

    #[test]
    fn the_scan_finds_a_word_across_the_edge_of_two_blocks_and_only_when_it_is_there() {
        let rig = rig(0);
        {
            let mut data = rig.data.lock().unwrap();
            *data = vec![b'a'; 100_000];
            let at = BLOCK as usize - 4;
            data[at..at + 10].copy_from_slice(b"/ByteRange");
        }
        let mut guard = SourceGuard::new(Box::new(Memory {
            data: rig.data.clone(),
            broken: rig.broken.clone(),
            modified: rig.modified.clone(),
        }))
        .unwrap();
        assert!(guard
            .contains_any(&[b"/DocMDP", b"/ByteRange"], &mut |_| {})
            .unwrap());
        assert!(!guard
            .contains_any(&[b"/DocMDP", b"/SigFlags"], &mut |_| {})
            .unwrap());
        // A file that is empty, or has a size that is a whole number of blocks, ends the scan.
        *rig.data.lock().unwrap() = vec![b'a'; BLOCK as usize * 2];
        let mut again = SourceGuard::new(Box::new(Memory {
            data: rig.data.clone(),
            broken: rig.broken.clone(),
            modified: rig.modified.clone(),
        }))
        .unwrap();
        assert!(!again.contains_any(&[b"/ByteRange"], &mut |_| {}).unwrap());
    }

    /// A file of `blocks` blocks, with every block read once.
    fn read_through(blocks: usize) -> Rig {
        let rig = rig(blocks * BLOCK as usize);
        let mut guard = lock(&rig.guard);
        // A word that is not in the file: the scan reads all of it.
        assert!(!guard.contains_any(&[b"/ByteRange"], &mut |_| {}).unwrap());
        drop(guard);
        rig
    }

    #[test]
    fn checking_a_big_file_reports_progress_with_a_rising_count() {
        let rig = read_through(200);
        let mut seen: Vec<u32> = Vec::new();
        assert_eq!(lock(&rig.guard).verify(&mut |done| seen.push(done)), Ok(()));
        // 200 blocks are 6.25 MiB: a sign of life about every MiB.
        assert!(seen.len() >= 5, "too few signs of life: {seen:?}");
        assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
        assert!(*seen.last().unwrap() <= 200);
    }

    #[test]
    fn scanning_a_big_file_reports_progress_with_a_rising_count() {
        let rig = rig(200 * BLOCK as usize);
        let mut seen: Vec<u32> = Vec::new();
        let found = lock(&rig.guard)
            .contains_any(&[b"/ByteRange"], &mut |done| seen.push(done))
            .unwrap();
        assert!(!found);
        assert!(seen.len() >= 5, "too few signs of life: {seen:?}");
        assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    }

    #[test]
    fn digests_differ_for_different_bytes() {
        assert_ne!(fnv(b"abc"), fnv(b"abd"));
        assert_eq!(fnv(b""), 0xcbf2_9ce4_8422_2325);
    }
}
