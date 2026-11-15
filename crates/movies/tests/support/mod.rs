//! Synthetic fixtures only: generated media in TMPDIR, no retail assets.
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Temp(pub PathBuf);
impl Temp {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let dir = std::env::temp_dir().join(format!(
                "h2-movies-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&dir) {
                Ok(()) => return Self(dir),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("scratch: {e}"),
            }
        }
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn media(path: &Path, filter: &str) -> Vec<u8> {
    media_for(path, filter, "0.2002")
}

#[allow(dead_code)]
pub fn media_for(path: &Path, filter: &str, seconds: &str) -> Vec<u8> {
    let output = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            filter,
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=44100",
            "-t",
            seconds,
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
            "-f",
            "nut",
            "-n",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read(path).unwrap()
}

/// Hand-built XDVDFS directory containing /bink and a right-child chain.
/// Payloads are generated NUT, even though discovery sees .bik file names.
pub fn disc(files: &[(&str, &[u8])]) -> xiso::Xiso<Cursor<Vec<u8>>> {
    fn node(name: &str, sector: u32, size: u32, attr: u8) -> Vec<u8> {
        let mut bytes = vec![0; (14 + name.len()).next_multiple_of(4)];
        bytes[4..8].copy_from_slice(&sector.to_le_bytes());
        bytes[8..12].copy_from_slice(&size.to_le_bytes());
        bytes[12] = attr;
        bytes[13] = name.len() as u8;
        bytes[14..14 + name.len()].copy_from_slice(name.as_bytes());
        bytes
    }
    assert!(!files.is_empty());
    let mut image = vec![0; 35 * 2048];
    image[32 * 2048..32 * 2048 + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
    image[32 * 2048 + 20..32 * 2048 + 24].copy_from_slice(&33u32.to_le_bytes());
    image[32 * 2048 + 24..32 * 2048 + 28].copy_from_slice(&2048u32.to_le_bytes());
    let root = node("bink", 34, 2048, 0x10);
    image[33 * 2048..33 * 2048 + root.len()].copy_from_slice(&root);
    let mut directory = Vec::new();
    for (i, (name, content)) in files.iter().enumerate() {
        let sector = image.len() / 2048;
        image.extend_from_slice(content);
        image.resize(image.len().next_multiple_of(2048), 0);
        let mut entry = node(name, sector as u32, content.len() as u32, 0);
        if i + 1 < files.len() {
            let next = (directory.len() + entry.len()) as u16 / 4;
            entry[2..4].copy_from_slice(&next.to_le_bytes());
        }
        directory.extend(entry);
    }
    assert!(directory.len() <= 2048);
    image[34 * 2048..34 * 2048 + directory.len()].copy_from_slice(&directory);
    xiso::Xiso::open(Cursor::new(image)).unwrap()
}

/// A stand-in that passes capability probing but cannot encode: enough to
/// reach the lock and manifest handling without the user's real ffmpeg.
#[cfg(unix)]
#[allow(dead_code)]
pub fn stub_ffmpeg(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("ffmpeg-capable-stub");
    fs::write(
        &path,
        "#!/bin/sh\n\
         case \"$1$2\" in\n\
         -version) echo 'ffmpeg version stub' ;;\n\
         -hide_banner-decoders) printf ' V....D binkvideo x\\n A....D binkaudio_dct x\\n A....D binkaudio_rdft x\\n' ;;\n\
         -hide_banner-encoders) printf ' V....D libx264 x\\n A....D aac x\\n' ;;\n\
         *) exit 1 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
