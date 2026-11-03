//! Windows: the signer loads `bcrypt.dll` (the source of random numbers that the signature
//! arithmetic uses) when it first needs it, not when the process starts.
//!
//! The signer runs in an AppContainer with a locked-down token. The system's loader loads the
//! libraries a program imports with helper threads of the process, and those threads may not open
//! a library file, so every library a program imports has to be on the system's list of known
//! libraries, which are mapped without opening a file (see `.cargo/config.toml`). `bcrypt.dll`
//! is not on that list. Loaded on first use, it is opened by the first thread, which may open
//! files, and the signer uses it for the first time before it reads anything, in its start-up
//! (`startup::prepare`), so it is in the process before the first request arrives.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        println!("cargo:rustc-link-lib=delayimp");
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:bcrypt.dll");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
