use std::process::{Command, Stdio};
fn main() {
    println!("cargo:rustc-check-cfg=cfg(has_ffmpeg)");
    println!("cargo:rerun-if-env-changed=PATH");
    if Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
    {
        println!("cargo:rustc-cfg=has_ffmpeg");
    }
}
