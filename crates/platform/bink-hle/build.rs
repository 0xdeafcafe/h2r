use std::env;
use std::path::PathBuf;
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
    if env::var_os("CARGO_FEATURE_PLAYBACK").is_none() {
        return;
    }
    if has_ffmpeg() {
        println!("cargo:rustc-cfg=has_ffmpeg");
    }
    println!("cargo:rerun-if-changed=native/avfoundation.m");
    if env::var("CARGO_CFG_TARGET_OS").unwrap() != "macos" {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        other => panic!("unsupported macOS architecture {other}"),
    };
    let object = out.join("avfoundation.o");
    // Warnings fail the build in CI only: a newer SDK must not break users.
    println!("cargo:rerun-if-env-changed=CI");
    let mut clang = Command::new("xcrun");
    clang.arg("clang");
    if env::var_os("CI").is_some_and(|v| !v.is_empty()) {
        clang.arg("-Werror");
    }
    let status = clang
        .args([
            "-c",
            "-fobjc-arc",
            "-arch",
            arch,
            "-mmacosx-version-min=11.0",
            "-Wall",
            "-Wextra",
            "-Wno-deprecated-declarations",
            "native/avfoundation.m",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("macOS SDK clang is required");
    assert!(status.success(), "compiling the AVFoundation bridge failed");
    let status = Command::new("xcrun")
        .args(["ar", "crs"])
        .arg(out.join("libbink_avfoundation.a"))
        .arg(object)
        .status()
        .expect("macOS SDK ar is required");
    assert!(status.success(), "archiving the AVFoundation bridge failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=bink_avfoundation");
    for framework in ["AVFoundation", "Foundation", "CoreMedia", "CoreVideo"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-link-lib=objc");
}
