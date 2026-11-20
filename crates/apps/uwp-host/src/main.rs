//! `h2uwp.exe`, the Xbox app (docs/runtime/xbox.md).
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    if let Err(e) = uwp_host::shell::main() {
        eprintln!("h2uwp: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("h2uwp is the Xbox app: build it for Windows (platform/xbox/build.sh)");
    std::process::exit(2);
}
