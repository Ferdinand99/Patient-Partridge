fn main() {
    tauri_build::build();

    // The test program of the library needs the same manifest as the app: the menus and dialogs of
    // Windows import functions that only version 6 of the common controls has, and without the
    // manifest the program does not start (STATUS_ENTRYPOINT_NOT_FOUND).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo::rustc-link-arg-tests=/MANIFEST:EMBED");
        println!(
            "cargo::rustc-link-arg-tests=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
