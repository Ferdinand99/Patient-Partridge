//! Opening a link from a PDF in the user's web browser.
//!
//! A link in a PDF is untrusted: it is never opened by itself. The window asks the user first and
//! then calls `open_link`. This module is the second line of defence on the Rust side:
//!
//! - only `http` and `https` addresses are opened, never a file, a program, a script or anything
//!   else the system could be made to run;
//! - the address must be one that the engine reported for that very document, so the window cannot
//!   be made to open an address that no document holds;
//! - the address is passed to the browser as one argument, never through a shell.

use std::collections::{HashMap, HashSet};
use std::io;
use std::process::Command;

use partridge_protocol::MAX_URI_LEN;

/// Most addresses remembered for one document.
pub const MAX_ADDRESSES_PER_DOCUMENT: usize = 20_000;

/// Why an address was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    TooLong,
    /// Contains a space, a control character or something else that has no place in an address.
    BadCharacter,
    NotAnAddress,
    /// Not `http` or `https`.
    Scheme,
    NoHost,
    /// `https://name:password@host/` looks like one site and goes to another.
    Credentials,
}

/// Checks an address from a PDF. Gives the address to pass to the browser when it is a plain
/// `http` or `https` address.
pub fn check_web_address(uri: &str) -> Result<String, Refused> {
    if uri.len() > MAX_URI_LEN {
        return Err(Refused::TooLong);
    }
    if uri
        .chars()
        .any(|c| c <= ' ' || ('\u{7f}'..='\u{9f}').contains(&c))
    {
        return Err(Refused::BadCharacter);
    }
    let url = url::Url::parse(uri).map_err(|_| Refused::NotAnAddress)?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(Refused::Scheme);
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(Refused::NoHost);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Refused::Credentials);
    }
    Ok(url.into())
}

/// The addresses the engine has reported for each open document.
#[derive(Default)]
pub struct KnownLinks(HashMap<u64, HashSet<String>>);

impl KnownLinks {
    pub fn remember(&mut self, doc: u64, uri: &str) {
        let set = self.0.entry(doc).or_default();
        if set.len() < MAX_ADDRESSES_PER_DOCUMENT {
            set.insert(uri.to_string());
        }
    }

    pub fn forget(&mut self, doc: u64) {
        self.0.remove(&doc);
    }

    /// Forgets every document: the window that knew them is gone.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn allows(&self, doc: u64, uri: &str) -> bool {
        self.0.get(&doc).is_some_and(|set| set.contains(uri))
    }
}

/// Starts the system's way of opening a web address. The address is one argument of the program,
/// so nothing in it can be taken for a command.
pub fn open_in_browser(url: &str) -> io::Result<()> {
    let mut command = launcher(url);
    let mut child = command.spawn()?;
    // Reap the helper when it is done, so no zombie stays behind.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(target_os = "windows")]
fn launcher(url: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut command = Command::new("rundll32.exe");
    command.arg("url.dll,FileProtocolHandler").arg(url);
    // CREATE_NO_WINDOW
    command.creation_flags(0x0800_0000);
    command
}

#[cfg(target_os = "macos")]
fn launcher(url: &str) -> Command {
    let mut command = Command::new("/usr/bin/open");
    command.arg(url);
    command
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn launcher(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_web_addresses_are_accepted() {
        for ok in [
            "http://example.com",
            "https://example.com/path?q=1&r=2#frag",
            "HTTPS://EXAMPLE.COM/",
            "https://example.com:8443/a",
            "https://[2001:db8::1]/",
            "https://bücher.example/",
        ] {
            assert!(check_web_address(ok).is_ok(), "{ok}");
        }
        assert_eq!(
            check_web_address("HTTPS://EXAMPLE.COM/a b").unwrap_err(),
            Refused::BadCharacter
        );
        // The address given to the browser is the cleaned up one.
        assert_eq!(
            check_web_address("HTTPS://EXAMPLE.COM").unwrap(),
            "https://example.com/"
        );
        assert_eq!(
            check_web_address("https://bücher.example/").unwrap(),
            "https://xn--bcher-kva.example/"
        );
    }

    #[test]
    fn everything_else_is_refused() {
        let cases = [
            ("javascript:alert(1)", Refused::Scheme),
            ("file:///etc/passwd", Refused::Scheme),
            ("file://server/share/a.exe", Refused::Scheme),
            ("ftp://example.com/", Refused::Scheme),
            ("mailto:a@example.com", Refused::Scheme),
            ("tel:+4712345678", Refused::Scheme),
            ("data:text/html,<script>1</script>", Refused::Scheme),
            ("vbscript:run", Refused::Scheme),
            ("ms-msdt:/id", Refused::Scheme),
            ("ssh://host", Refused::Scheme),
            ("smb://host/share", Refused::Scheme),
            ("http:", Refused::NotAnAddress),
            ("https://", Refused::NotAnAddress),
            ("example.com", Refused::NotAnAddress),
            ("//example.com", Refused::NotAnAddress),
            ("", Refused::NotAnAddress),
            ("https://user:pw@example.com/", Refused::Credentials),
            ("https://google.com@evil.example/", Refused::Credentials),
            ("https://example.com/\u{0}", Refused::BadCharacter),
            ("https://example.com/\n", Refused::BadCharacter),
            (" https://example.com/", Refused::BadCharacter),
            ("https://exa mple.com/", Refused::BadCharacter),
            ("https://example.com/\u{85}", Refused::BadCharacter),
        ];
        for (uri, why) in cases {
            assert_eq!(check_web_address(uri), Err(why), "{uri:?}");
        }
    }

    #[test]
    fn a_very_long_address_is_refused() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URI_LEN));
        assert_eq!(check_web_address(&long), Err(Refused::TooLong));
    }

    #[test]
    fn only_addresses_that_a_document_holds_are_allowed_for_that_document() {
        let mut known = KnownLinks::default();
        known.remember(1, "https://example.com/a");
        known.remember(2, "https://example.com/b");
        assert!(known.allows(1, "https://example.com/a"));
        assert!(!known.allows(1, "https://example.com/b"));
        assert!(!known.allows(2, "https://example.com/a"));
        assert!(!known.allows(3, "https://example.com/a"));
        known.forget(1);
        assert!(!known.allows(1, "https://example.com/a"));
        assert!(known.allows(2, "https://example.com/b"));
    }

    #[test]
    fn the_memory_of_one_document_is_limited() {
        let mut known = KnownLinks::default();
        for i in 0..(MAX_ADDRESSES_PER_DOCUMENT + 50) {
            known.remember(1, &format!("https://example.com/{i}"));
        }
        assert!(known.allows(1, "https://example.com/0"));
        assert!(!known.allows(
            1,
            &format!("https://example.com/{}", MAX_ADDRESSES_PER_DOCUMENT + 10)
        ));
    }
}
