//! The operating system sandbox of the engine.
//!
//! The engine parses files nobody has checked, with a large C++ library. If that library is
//! tricked into running code, the code must find nothing to work with. So after everything the
//! engine needs is loaded (PDFium, fonts) and before it reads one byte of a document, the engine
//! locks itself in:
//!
//! - **Linux**: a seccomp-BPF allowlist that is always applied (no new files, no sockets, no new
//!   processes, no `ptrace`, no executable memory), and Landlock on top where the kernel has it.
//! - **macOS**: a Seatbelt profile that denies everything by default.
//! - **Windows**: the app starts the engine in an AppContainer without capabilities (which blocks the
//!   network) made from a restricted token that holds no privilege, with the untrusted integrity
//!   level, a job object and process mitigation policies (see `partridge-engine-client`); the engine only gives
//!   up the more powerful start-up identity here and checks that the restrictions are in force.
//!
//! The engine never opens files by path: the app opens the document and hands over an inherited,
//! read-only handle. Nothing in the sandbox has to allow reading any file.
//!
//! A sandbox that did not apply is a failure. Every layer is checked after it is applied (the
//! engine tries to open a file and to use the network and must be refused: a socket on Linux and
//! macOS, a TCP connection, a UDP datagram and raw sockets on Windows), and a release build that
//! cannot show a working sandbox refuses to read documents. The one exception is the answer of the
//! network on Windows: when the attempts are not refused with "access denied" the engine still
//! runs, because every layer it controls is in force, and its greeting says `network-not-proved`
//! instead of `no-network`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    pub fn lock_down() -> Result<Vec<&'static str>, String> {
        Err("this operating system has no sandbox for the engine".to_string())
    }
}

/// What is in force after [`lock_down`] succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    layers: Vec<&'static str>,
    /// Windows: what the system answered when the network block was not proved, if it was not.
    note: Option<String>,
}

impl Applied {
    /// The layers that are in force, such as `seccomp` and `landlock`.
    pub fn layers(&self) -> &[&'static str] {
        &self.layers
    }

    /// The layers as one text, such as `seccomp+landlock`. This is what the engine tells the app.
    pub fn describe(&self) -> String {
        match &self.note {
            Some(note) => format!("{} ({note})", self.layers.join("+")),
            None => self.layers.join("+"),
        }
    }
}

/// Puts the process in an empty root file system where the system allows it (Linux: a user and a
/// mount namespace), so that a path lookup finds nothing. An extra layer: an error says why it is
/// not in force, and the engine goes on. Call it while the process has only one thread, after
/// everything is loaded and before [`lock_down`].
pub fn enter_isolation() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::enter_empty_root()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

/// Linux: puts the process under Landlock where the kernel has it. Call it after
/// [`enter_isolation`] and before any thread of the engine is started: Landlock holds for the
/// thread that applies it and the threads started after that, not for a thread that already runs.
/// An error says why it is not in force; the engine goes on, and [`lock_down`] does not name the
/// layer. Elsewhere there is nothing to do.
pub fn restrict_files() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::restrict_files()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(())
    }
}

/// Locks the process in. Call it once, after PDFium and everything else is loaded and before any
/// byte of a document is read. On success every layer has been checked to work.
pub fn lock_down() -> Result<Applied, String> {
    let layers = platform::lock_down()?;
    if layers.is_empty() {
        return Err("no sandbox layer is in force".to_string());
    }
    #[cfg(windows)]
    let note = windows::network_note();
    #[cfg(not(windows))]
    let note = None;
    Ok(Applied { layers, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_greeting_names_the_layers_and_what_the_system_answered_to_the_network() {
        let proved = Applied {
            layers: vec!["restricted-token", "no-network"],
            note: None,
        };
        assert_eq!(proved.describe(), "restricted-token+no-network");
        let not_proved = Applied {
            layers: vec!["restricted-token", "network-not-proved"],
            note: Some("a TCP connection failed".to_string()),
        };
        assert_eq!(
            not_proved.describe(),
            "restricted-token+network-not-proved (a TCP connection failed)"
        );
    }
}
