use std::env;
use std::process::{Command, Stdio};

/// Whether the user's ffmpeg runs, for the tests that need it.
///
/// The script follows the binary it found, not `PATH`. Cargo reruns a script
/// whose tracked variable changed, then rebuilds this crate and everything
/// above it (boot, cli), and `PATH` differs between the shells that share a
/// target directory. After installing ffmpeg, touch this file.
fn has_ffmpeg() -> bool {
    let name = format!("ffmpeg{}", env::consts::EXE_SUFFIX);
    let found = env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join(&name))
            .find(|file| file.is_file())
    });
    let Some(ffmpeg) = found else {
        return false;
    };
    println!("cargo:rerun-if-changed={}", ffmpeg.display());
    Command::new(&ffmpeg)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(has_ffmpeg)");
    println!("cargo:rerun-if-changed=build.rs");
    if has_ffmpeg() {
        println!("cargo:rustc-cfg=has_ffmpeg");
    }
}
