//! The signer on Windows starts in an AppContainer with a locked-down token. The system's loader
//! loads the libraries a program imports with helper threads that may not open a library file, so
//! every library the signer imports at start has to be one the system maps without opening a file
//! (the system's list of known libraries, or the libraries that stand for them). A library that is
//! not on the list is loaded on first use instead (a delay-load import, see `build.rs`).
//!
//! This test reads the import tables of the built program and holds it to the same list as the
//! engine's: a new dependency that imports another library fails here, on the machine of whoever
//! added it, instead of as a program that does not start in the sandbox.

#![cfg(windows)]

use std::fs;

/// The libraries the engine imports (the list that is known to start in the sandbox every time).
const ALLOWED: &[&str] = &[
    "kernel32.dll",
    "ws2_32.dll",
    "advapi32.dll",
    "api-ms-win-core-synch-l1-2-0.dll",
    "user32.dll",
    "ntdll.dll",
    "bcryptprimitives.dll",
];

fn u16_at(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes([b[at], b[at + 1]]) as usize
}

fn u32_at(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
}

/// The names of the libraries in the import table (`delay` false) or the delay-load table.
fn imported(bytes: &[u8], delay: bool) -> Vec<String> {
    let pe = u32_at(bytes, 0x3c);
    assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
    let sections = u16_at(bytes, pe + 6);
    let optional_size = u16_at(bytes, pe + 20);
    let optional = pe + 24;
    let directories = optional
        + if u16_at(bytes, optional) == 0x20b {
            112
        } else {
            96
        };
    let table = |index: usize| u32_at(bytes, directories + index * 8);
    let section_table = optional + optional_size;
    let to_file = |rva: usize| -> usize {
        for n in 0..sections {
            let s = section_table + n * 40;
            let (size, address, raw) = (
                u32_at(bytes, s + 8),
                u32_at(bytes, s + 12),
                u32_at(bytes, s + 20),
            );
            if rva >= address && rva < address + size.max(1) {
                return rva - address + raw;
            }
        }
        panic!("address {rva:#x} is in no section");
    };
    let name = |rva: usize| -> String {
        let start = to_file(rva);
        let end = start + bytes[start..].iter().position(|&b| b == 0).unwrap();
        String::from_utf8_lossy(&bytes[start..end]).to_ascii_lowercase()
    };
    let (rva, entry, name_at) = if delay { (13, 32, 4) } else { (1, 20, 12) };
    let first = table(rva);
    let mut names = Vec::new();
    if first != 0 {
        let mut at = to_file(first);
        while u32_at(bytes, at + name_at) != 0 {
            names.push(name(u32_at(bytes, at + name_at)));
            at += entry;
        }
    }
    names
}

#[test]
fn the_signer_imports_at_start_only_libraries_the_system_maps_without_a_file() {
    let bytes = fs::read(env!("CARGO_BIN_EXE_partridge-signer")).unwrap();
    let now = imported(&bytes, false);
    assert!(!now.is_empty(), "no import table was found");
    for library in &now {
        assert!(
            ALLOWED.contains(&library.as_str()),
            "the signer imports {library} at start; a locked-in program may fail to load it. \
             Imported: {now:?}"
        );
    }
    // The one library the signature arithmetic needs that is not on the list is loaded on first use.
    assert!(
        imported(&bytes, true).contains(&"bcrypt.dll".to_string()),
        "bcrypt.dll is not a delay-load import"
    );
}
