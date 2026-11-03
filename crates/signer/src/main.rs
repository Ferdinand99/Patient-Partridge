//! Process entry of the signer. Standard output carries the protocol and nothing else, so this
//! program must never print to it. Everything it has to say goes into a response.

// On Windows the signer has no console: the app starts it with pipes, and a console program that
// starts without a console (or with a restricted token) fails in its start-up code.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{self, BufReader, BufWriter};
use std::process::ExitCode;

use partridge_signer::startup;

fn main() -> ExitCode {
    // The one thing the app may tell the signer on its command line is how much memory it may use
    // (the app does this for every program it starts); the signer can only lower its own limit.
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let limit = match args.as_slice() {
        [] => None,
        [flag, value] if flag == "--memory-limit" => {
            match value.to_str().and_then(|v| v.parse::<u64>().ok()) {
                Some(bytes) => Some(bytes),
                None => return ExitCode::from(2),
            }
        }
        _ => return ExitCode::from(2),
    };
    let startup::Prepared {
        mut signer,
        protocol,
        ..
    } = startup::prepare(limit);
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(protocol);
    ExitCode::from(partridge_signer::serve(
        &mut input,
        &mut output,
        &mut signer,
    ))
}
