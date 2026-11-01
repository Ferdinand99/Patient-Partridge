//! Process entry of the PDF engine. Standard output carries the protocol and nothing else, so
//! this program must never print to it. Everything it has to say goes into a response.

use std::io::{self, BufReader, BufWriter};
use std::process::ExitCode;

use partridge_engine::{parse_args, Engine};
use partridge_protocol::{read_frame, write_response, Request};

/// Keeps Windows from showing a "program stopped working" dialog when the engine crashes. The
/// app notices the exit by itself and tells the user.
#[cfg(windows)]
fn quiet_crashes() {
    extern "system" {
        fn SetErrorMode(mode: u32) -> u32;
    }
    const SEM_FAILCRITICALERRORS: u32 = 0x0001;
    const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
    const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;
    // SAFETY: SetErrorMode only changes a process flag and has no pointer arguments.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
}

#[cfg(not(windows))]
fn quiet_crashes() {}

fn main() -> ExitCode {
    quiet_crashes();
    // Set before anything from a PDF is read. A failure to set it is not fatal.
    partridge_engine::limits::limit_memory();
    partridge_engine::limits::exit_with_parent();

    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(_) => return ExitCode::from(2),
    };
    let pdfium_dir = args.pdfium_dir.unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default()
    });

    let mut engine = Engine::new(&pdfium_dir);
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(partridge_engine::stdio::take_protocol_output());

    loop {
        let frame = match read_frame(&mut input) {
            Ok(Some(frame)) => frame,
            // The app closed our input: it is gone or does not need us any more.
            Ok(None) => return ExitCode::SUCCESS,
            Err(_) => return ExitCode::from(3),
        };
        // A request we cannot decode has no id to answer, so the only safe move is to stop.
        let Ok(request) = Request::decode(&frame) else {
            return ExitCode::from(3);
        };
        let response = engine.handle(request);
        if write_response(&mut output, &response).is_err() {
            return ExitCode::from(4);
        }
    }
}
