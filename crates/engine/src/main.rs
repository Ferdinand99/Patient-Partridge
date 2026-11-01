//! Process entry of the PDF engine. Standard output carries the protocol and nothing else, so
//! this program must never print to it. Everything it has to say goes into a response.

use std::io::{self, BufReader, BufWriter};
use std::process::ExitCode;

use partridge_engine::{parse_args, startup};
use partridge_protocol::{read_frame, write_response, Request};

fn main() -> ExitCode {
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(_) => return ExitCode::from(2),
    };
    let startup::Prepared {
        mut engine,
        protocol,
        ..
    } = startup::prepare(args);
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(protocol);

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
        // An engine that could not lock itself in has said so; it serves nothing.
        if engine.is_refusing() {
            return ExitCode::from(5);
        }
    }
}
