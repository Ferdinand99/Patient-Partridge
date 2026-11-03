//! Process entry of the signer. Standard output carries the protocol and nothing else, so this
//! program must never print to it. Everything it has to say goes into a response.

// On Windows the signer has no console: the app starts it with pipes, and a console program that
// starts without a console (or with a restricted token) fails in its start-up code.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::io::{self, BufReader, BufWriter};
use std::process::ExitCode;

use partridge_signer::startup;

fn main() -> ExitCode {
    // The signer takes no arguments: nothing the app could tell it on a command line changes what
    // it does.
    if std::env::args_os().len() > 1 {
        return ExitCode::from(2);
    }
    let startup::Prepared {
        mut signer,
        protocol,
        ..
    } = startup::prepare();
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(protocol);
    ExitCode::from(partridge_signer::serve(&mut input, &mut output, &mut signer))
}
