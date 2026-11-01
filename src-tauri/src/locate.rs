//! Finding things on disk: the engine program, the PDFium library, and files named on the
//! command line.

use std::path::{Path, PathBuf};

/// Name of the PDFium library file on this platform.
fn library_file() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

/// Folder name used for this platform in `vendor/pdfium` (see `scripts/fetch-pdfium.mjs`).
#[cfg(debug_assertions)]
fn platform_id() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "win-arm64",
        ("windows", _) => "win-x64",
        ("macos", "aarch64") => "mac-arm64",
        ("macos", _) => "mac-x64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-x64",
    }
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_default()
}

/// The engine program. It is installed next to the app.
pub fn engine_program() -> PathBuf {
    exe_dir().join(format!("partridge-engine{}", std::env::consts::EXE_SUFFIX))
}

/// Folders that may hold the PDFium library, best first.
fn pdfium_candidates(resource_dir: Option<&Path>, override_dir: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = override_dir {
        out.push(dir);
    }
    if let Some(dir) = resource_dir {
        out.push(dir.to_path_buf());
    }
    out.push(exe_dir());
    // Running from a source checkout (`tauri dev`, `cargo run`).
    #[cfg(debug_assertions)]
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join("vendor/pdfium").join(platform_id()));
        out.push(cwd.join("../vendor/pdfium").join(platform_id()));
    }
    out
}

/// The folder to tell the engine to load PDFium from. When no folder has the library, the app
/// folder is returned, so the engine reports a missing library with a path the user can check.
pub fn pdfium_dir(resource_dir: Option<&Path>) -> PathBuf {
    // Only development builds let the environment pick the library; a release build loads it
    // from its own installation and nowhere else.
    #[cfg(debug_assertions)]
    let override_dir = std::env::var_os("PARTRIDGE_PDFIUM_DIR").map(PathBuf::from);
    #[cfg(not(debug_assertions))]
    let override_dir = None;
    first_with_library(pdfium_candidates(resource_dir, override_dir))
}

fn first_with_library(candidates: Vec<PathBuf>) -> PathBuf {
    candidates
        .iter()
        .find(|dir| dir.join(library_file()).is_file())
        .cloned()
        .unwrap_or_else(exe_dir)
}

/// The arguments that name an existing file, as absolute paths. Flags are skipped.
pub fn existing_files<I: IntoIterator<Item = String>>(args: I, cwd: Option<&Path>) -> Vec<String> {
    args.into_iter()
        .filter(|a| !a.starts_with('-'))
        .map(|a| {
            let p = PathBuf::from(&a);
            if p.is_relative() {
                cwd.map(|c| c.join(&p)).unwrap_or(p)
            } else {
                p
            }
        })
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("partridge-locate-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_existing_files_are_taken_from_the_command_line() {
        let dir = temp_dir("args");
        std::fs::write(dir.join("a.pdf"), b"x").unwrap();
        let args = vec![
            "--flag".to_string(),
            "a.pdf".to_string(),
            "missing.pdf".to_string(),
            dir.to_string_lossy().into_owned(),
        ];
        let found = existing_files(args, Some(&dir));
        assert_eq!(
            found,
            vec![dir.join("a.pdf").to_string_lossy().into_owned()]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_first_folder_with_the_library_wins() {
        let empty = temp_dir("empty");
        let full = temp_dir("full");
        std::fs::write(full.join(library_file()), b"x").unwrap();
        let chosen = first_with_library(vec![empty.clone(), full.clone()]);
        assert_eq!(chosen, full);
        std::fs::remove_dir_all(&empty).unwrap();
        std::fs::remove_dir_all(&full).unwrap();
    }

    #[test]
    fn the_override_comes_first_and_the_app_folder_is_the_fallback() {
        let c = pdfium_candidates(Some(Path::new("/res")), Some(PathBuf::from("/over")));
        assert_eq!(c[0], PathBuf::from("/over"));
        assert_eq!(c[1], PathBuf::from("/res"));
        assert_eq!(
            first_with_library(vec![PathBuf::from("/nowhere")]),
            exe_dir()
        );
    }
}
