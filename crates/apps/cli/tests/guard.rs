//! The writers must never clobber the disc, a disc image or an existing file.
//! Synthetic files only, in a private temp dir.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("h2-guard-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn h2(args: &[&Path], extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_h2"))
        .arg("extract-xbe")
        .args(args)
        .args(extra)
        .env_remove("H2_DISC")
        .output()
        .unwrap()
}

fn fake_xbe(p: &Path) {
    fs::write(p, b"XBEH synthetic").unwrap();
}

fn fake_iso(p: &Path) -> Vec<u8> {
    let mut v = vec![0u8; 0x10000];
    v.extend_from_slice(b"MICROSOFT*XBOX*MEDIA");
    v.extend_from_slice(b"precious");
    fs::write(p, &v).unwrap();
    v
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn reversed_arguments_fail_and_write_nothing() {
    let d = scratch("reversed");
    let (xbe, iso) = (d.join("default.xbe"), d.join("halo2.iso"));
    fake_xbe(&xbe);
    let before = fake_iso(&iso);
    // The original accident: output slot got the disc, with and without --force.
    for extra in [&[][..], &["--force"][..]] {
        let o = h2(&[&xbe, &iso], extra);
        assert!(!o.status.success());
        assert!(stderr(&o).contains("refusing"), "{}", stderr(&o));
        assert_eq!(fs::read(&iso).unwrap(), before);
    }
    // And the same with the disc in the first slot and a swapped pair.
    let o = h2(&[&iso, &xbe], &["--force"]);
    assert!(!o.status.success());
    assert_eq!(fs::read(&iso).unwrap(), before);
    assert_eq!(fs::read(&xbe).unwrap(), b"XBEH synthetic");
}

#[test]
fn source_disc_is_refused_even_with_force() {
    let d = scratch("source");
    let src = d.join("src.xbe");
    fake_xbe(&src);
    let o = h2(&[&src, &src], &["--force"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("disc being read"), "{}", stderr(&o));
    assert_eq!(fs::read(&src).unwrap(), b"XBEH synthetic");
    #[cfg(unix)]
    {
        let link = d.join("link.xbe");
        std::os::unix::fs::symlink(&src, &link).unwrap();
        assert!(!h2(&[&src, &link], &["--force"]).status.success());
        assert_eq!(fs::read(&src).unwrap(), b"XBEH synthetic");
    }
}

#[test]
fn existing_file_needs_force() {
    let d = scratch("exists");
    let (src, out) = (d.join("src.xbe"), d.join("out.xbe"));
    fake_xbe(&src);
    fs::write(&out, b"mine").unwrap();
    let o = h2(&[&src, &out], &[]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--force"), "{}", stderr(&o));
    assert_eq!(fs::read(&out).unwrap(), b"mine");
    assert!(h2(&[&src, &out], &["--force"]).status.success());
    assert_eq!(fs::read(&out).unwrap(), b"XBEH synthetic");
    // A fresh path just works.
    assert!(h2(&[&src, &d.join("new.xbe")], &[]).status.success());
}

#[test]
fn iso_paths_are_refused_even_when_new_or_forced() {
    let d = scratch("isopath");
    let src = d.join("src.xbe");
    fake_xbe(&src);
    for name in ["new.iso", "NEW.ISO", "new.xiso"] {
        let out = d.join(name);
        for extra in [&[][..], &["--force"][..]] {
            let o = h2(&[&src, &out], extra);
            assert!(!o.status.success(), "{name}");
            assert!(!out.exists(), "{name} was created");
        }
    }
    // An existing .bin that is really a disc image is refused by content.
    let img = d.join("disc.bin");
    let before = fake_iso(&img);
    assert!(!h2(&[&src, &img], &["--force"]).status.success());
    assert_eq!(fs::read(&img).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn unreadable_files_are_refused_even_with_force() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch("unreadable");
    let (src, out) = (d.join("source.xbe"), d.join("unreadable.dat"));
    fake_xbe(&src);
    let before = fake_iso(&out);
    fs::set_permissions(&out, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&out).is_ok() {
        return; // root
    }
    let o = h2(&[&src, &out], &["--force"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("cannot inspect"), "{}", stderr(&o));
    fs::set_permissions(&out, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(fs::read(&out).unwrap(), before);
}
