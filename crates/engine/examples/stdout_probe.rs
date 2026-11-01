//! Test helper: takes the protocol output the way the engine does, prints garbage to standard
//! output, and then writes one valid frame to the protocol. The test checks that only the frame
//! comes out.

use std::io::Write;

use partridge_protocol::{write_response, Response, ResponseBody};

fn main() {
    let mut protocol = partridge_engine::stdio::take_protocol_output();
    println!("garbage from a library");
    print!("more garbage without a line break");
    let _ = std::io::stdout().flush();
    write_response(
        &mut protocol,
        &Response {
            id: 7,
            body: ResponseBody::Closed,
        },
    )
    .unwrap();
}
