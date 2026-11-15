//! Symlink-safe, no-clobber output directory for exporters.
//!
//! Every directory component below the chosen root is created one level at a
//! time and opened with `O_NOFOLLOW | O_DIRECTORY` (so a symlink, or anything
//! that is not a real directory, is refused); the descriptors are held until
//! the write finishes. Files are written to a temporary name in the target
//! directory and published only when complete: without `force` by a hard link
//! (an exclusive create that fails if the name appeared meanwhile), with
//! `force` by a rename, after re-validating what is being replaced.
//!
//! What is never overwritten, even with `force`: the disc being read (by
//! canonical path and by dev/inode), any file holding an XDVDFS volume at any
//! partition offset `xiso` supports, and any path ending in `.iso`/`.xiso`.
//! Components are validated: no empty, `.`, `..`, separators, drive colons or
//! NUL.
//!
//! Out of scope: a hostile process racing the directory tree between the
//! checks and the final link or rename. File operations after the directory
//! walk are path based. With [`OutDir::new`] the root itself is the caller's
//! choice and may be a symlink; [`OutDir::new_strict`] also walks the root's
//! own components and refuses user-controlled symlinks there.
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// Discs opened for reading by this process. Every [`OutDir`] refuses to
/// write to them, so an exporter cannot forget to say which disc it reads.
static PROTECTED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Record `disc` as a read-only input for the rest of the process.
pub fn protect(disc: &Path) {
    if let Ok(mut p) = PROTECTED.lock() {
        p.push(disc.to_path_buf());
    }
}

pub struct OutDir {
    root: PathBuf,
    force: bool,
    /// Extra source to protect, on top of [`protect`]ed discs.
    source: Option<PathBuf>,
}

fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

fn same_file(a: &Path, b: &Path) -> bool {
    if let (Ok(x), Ok(y)) = (a.canonicalize(), b.canonicalize())
        && x == y
    {
        return true;
    }
    #[cfg(unix)]
    if let (Ok(x), Ok(y)) = (fs::metadata(a), fs::metadata(b)) {
        use std::os::unix::fs::MetadataExt;
        return x.dev() == y.dev() && x.ino() == y.ino();
    }
    false
}

/// May `p` be an XDVDFS volume at any partition offset `xiso` accepts (plain
/// XISO and the Redump layouts)? Fails closed: a file whose content cannot be
/// inspected counts as a disc, so it is never replaced.
fn may_be_disc(p: &Path) -> io::Result<bool> {
    let f = File::open(p)?;
    Ok(xiso::Xiso::open(f).is_ok())
}

/// Refuse a path no exporter may ever replace, whatever `--force` says: the
/// disc being read, a disc image (by content or by extension). The extension
/// rule also covers a path that does not exist yet. Call this before the file
/// is touched.
pub fn guard_target(dest: &Path, source: Option<&Path>) -> io::Result<()> {
    let ext = dest.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("iso" | "xiso")) {
        return Err(bad(format!(
            "refusing to write {}: it looks like a disc image (did you swap the arguments? the output comes last)",
            dest.display()
        )));
    }
    if fs::symlink_metadata(dest).is_err() {
        return Ok(()); // nothing there yet
    }
    let protected = PROTECTED.lock().map(|p| p.clone()).unwrap_or_default();
    if source.into_iter().chain(protected.iter().map(PathBuf::as_path)).any(|s| same_file(dest, s)) {
        return Err(bad(format!("refusing to write {}: it is the disc being read", dest.display())));
    }
    if fs::metadata(dest).is_ok_and(|m| m.is_file()) {
        match may_be_disc(dest) {
            Ok(false) => {}
            Ok(true) => {
                return Err(bad(format!("refusing to write {}: it is an Xbox disc image", dest.display())));
            }
            Err(e) => {
                return Err(bad(format!(
                    "refusing to replace {}: cannot inspect it to rule out a disc image ({e})",
                    dest.display()
                )));
            }
        }
    }
    Ok(())
}

/// Open `path` as a real directory, never through a symlink.
fn open_real_dir(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(path)
    }
    #[cfg(not(unix))]
    {
        if fs::symlink_metadata(path)?.is_dir() { File::open(path) } else { Err(io::ErrorKind::InvalidInput.into()) }
    }
}

impl OutDir {
    pub fn new(root: impl Into<PathBuf>, force: bool) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root, force, source: None })
    }

    /// Like [`Self::new`], but every component of the root path, from the
    /// filesystem root down, is checked (and missing ones created) one level
    /// at a time. An existing component must be a real directory. The only
    /// symlinks accepted are the macOS system aliases in [`SYSTEM_ALIASES`]
    /// (`/var`, `/tmp`, `/etc`), each only when root-owned with exactly its
    /// expected target; the walk then continues, still strict, on the
    /// resolved path, which becomes the stored root. Any other symlink, a
    /// non-directory, or `..` is refused. Relative roots are resolved against
    /// the (physical) working directory.
    pub fn new_strict(root: impl Into<PathBuf>, force: bool) -> io::Result<Self> {
        let root = root.into();
        let root = if root.is_absolute() { root } else { std::env::current_dir()?.join(root) };
        let mut cur = PathBuf::new();
        for c in root.components() {
            match c {
                Component::Prefix(_) | Component::RootDir => {
                    cur.push(c);
                    continue;
                }
                Component::CurDir => continue,
                Component::ParentDir => {
                    return Err(bad(format!("output root {} contains '..'", root.display())));
                }
                Component::Normal(n) => cur.push(n),
            }
            match fs::symlink_metadata(&cur) {
                Ok(m) if m.is_dir() => {}
                Ok(m) if m.file_type().is_symlink() => {
                    let target = fs::read_link(&cur)?;
                    let resolved = system_alias(&cur, owner(&m), &target).ok_or_else(|| {
                        bad(format!("{} is a symlink; refusing an untrusted output root", cur.display()))
                    })?;
                    // Re-walk the alias target strictly from the root.
                    let mut real = PathBuf::from("/");
                    for part in resolved.components().skip(1) {
                        real.push(part);
                        if !fs::symlink_metadata(&real)?.is_dir() {
                            return Err(bad(format!("{} is not a real directory (symlink?)", real.display())));
                        }
                    }
                    cur = real;
                }
                Ok(_) => {
                    return Err(bad(format!("{} is not a real directory", cur.display())));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&cur)?;
                    if !fs::symlink_metadata(&cur)?.is_dir() {
                        return Err(bad(format!("{} is not a real directory", cur.display())));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Self { root: cur, force, source: None })
    }

    /// Never write to `source` (the disc being read), even with `force`.
    pub fn protecting(mut self, source: &Path) -> Self {
        self.source = Some(source.to_path_buf());
        self
    }

    /// Write `bytes` to `root/<components...>`; returns the final path.
    pub fn write(&self, components: &[String], bytes: &[u8]) -> io::Result<PathBuf> {
        self.write_from(components, &mut &*bytes)
    }

    /// Like `write`, but streams from `src` (no whole-file buffer).
    pub fn write_from(&self, components: &[String], src: &mut impl io::Read) -> io::Result<PathBuf> {
        self.write_with(components, |f| io::copy(src, f).map(drop))
    }

    /// Like `write`, but `fill` writes the contents into the temporary file.
    ///
    /// The data goes to a temporary name in the destination directory and is
    /// published only once fully written, so a failing `fill` never leaves a
    /// partial file and never destroys an existing one.
    pub fn write_with(
        &self,
        components: &[String],
        fill: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<PathBuf> {
        let (dest, _dirs) = self.resolve(components)?;
        let tmp = dest.with_file_name(format!(
            ".{}.tmp-{}",
            dest.file_name().and_then(|n| n.to_str()).unwrap_or("out"),
            std::process::id()
        ));
        // create_new: never follows a planted symlink at the temp name either.
        let mut f: File = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let published = fill(&mut f).and_then(|_| f.sync_all()).and_then(|()| {
            if self.force {
                // Re-validate right before replacing: only a regular file that
                // is neither the disc nor a disc image goes.
                match fs::symlink_metadata(&dest) {
                    Ok(m) if !m.is_file() => return Err(exists(&dest)),
                    Ok(_) => guard_target(&dest, self.source.as_deref())?,
                    Err(_) => {}
                }
                fs::rename(&tmp, &dest)
            } else {
                fs::hard_link(&tmp, &dest)
                    .map_err(|e| if e.kind() == io::ErrorKind::AlreadyExists { exists(&dest) } else { e })
            }
        });
        if !self.force || published.is_err() {
            let _ = fs::remove_file(&tmp); // after a rename the temp name is already gone
        }
        published.map(|()| dest)
    }

    /// Create `root/<components...>` under the same path rules as [`Self::write`]
    /// and hand back the open file, for output too large to hold in memory.
    /// Unlike `write`/`write_from` this is not published atomically: a failure
    /// part-way leaves a partial file, and under `force` a regular file already
    /// there is removed first.
    pub fn create(&self, components: &[String]) -> io::Result<(File, PathBuf)> {
        let (dest, _held) = self.resolve(components)?;
        if self.force && fs::symlink_metadata(&dest).is_ok_and(|m| m.is_file()) {
            fs::remove_file(&dest)?;
        }
        let f: File = OpenOptions::new().write(true).create_new(true).open(&dest)?;
        Ok((f, dest))
    }

    /// Create the directory `root/<components...>` (and its parents) through
    /// the same validated walk used for files.
    pub fn create_dir(&self, components: &[String]) -> io::Result<PathBuf> {
        let mut all = components.to_vec();
        all.push("x".into()); // walk every real component as a directory
        let (file, _dirs) = self.resolve_dirs(&all)?;
        Ok(file.parent().map(Path::to_path_buf).unwrap_or(file))
    }

    fn check_components(components: &[String]) -> io::Result<()> {
        for c in components {
            if c.is_empty() || c == "." || c == ".." || c.contains(['/', '\\', ':', '\0']) {
                return Err(bad(format!("unsafe output path component {c:?}")));
            }
        }
        Ok(())
    }

    /// Validate `components`, create parent directories and return the final
    /// path after checking what already sits there, with the directory
    /// descriptors held open.
    fn resolve(&self, components: &[String]) -> io::Result<(PathBuf, Vec<File>)> {
        let (cur, dirs) = self.resolve_dirs(components)?;
        guard_target(&cur, self.source.as_deref())?;
        match fs::symlink_metadata(&cur) {
            Ok(m) if self.force && m.is_file() => {}
            Ok(_) => return Err(exists(&cur)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        Ok((cur, dirs))
    }

    /// The directory walk: every component but the last is a real directory.
    fn resolve_dirs(&self, components: &[String]) -> io::Result<(PathBuf, Vec<File>)> {
        let (file, dirs) = components.split_last().ok_or_else(|| bad("empty output path".into()))?;
        Self::check_components(components)?;
        let mut cur = self.root.clone();
        let mut held = Vec::new();
        for d in dirs {
            cur.push(d);
            let f = match open_real_dir(&cur) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&cur)?;
                    open_real_dir(&cur)?
                }
                Err(_) => return Err(bad(format!("{} is not a real directory (symlink?)", cur.display()))),
            };
            held.push(f);
        }
        cur.push(file);
        Ok((cur, held))
    }
}

/// The only symlinks [`OutDir::new_strict`] follows: macOS system aliases,
/// as (link, exact `readlink` target).
pub const SYSTEM_ALIASES: [(&str, &str); 3] =
    [("/var", "private/var"), ("/tmp", "private/tmp"), ("/etc", "private/etc")];

/// The resolved absolute path of `link` if it is exactly an allowlisted
/// system alias: owned by root (`uid` 0) with exactly the expected target.
fn system_alias(link: &Path, uid: Option<u32>, target: &Path) -> Option<PathBuf> {
    if uid != Some(0) {
        return None;
    }
    SYSTEM_ALIASES
        .iter()
        .find(|(l, t)| link == Path::new(l) && target == Path::new(t))
        .map(|(_, t)| Path::new("/").join(t))
}

#[cfg(unix)]
fn owner(m: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(m.uid())
}
#[cfg(not(unix))]
fn owner(_: &fs::Metadata) -> Option<u32> {
    None
}

fn exists(p: &std::path::Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{} exists (use --force to replace a regular file)", p.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("h2-safe-out-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn parts(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn writes_nested_and_refuses_overwrite_without_force() {
        let d = scratch("nested");
        let out = OutDir::new(d.join("out"), false).unwrap();
        let p = out.write(&parts(&["a", "b", "x.wav"]), b"1").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"1");
        let e = out.write(&parts(&["a", "b", "x.wav"]), b"2").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&p).unwrap(), b"1");
        let forced = OutDir::new(d.join("out"), true).unwrap();
        forced.write(&parts(&["a", "b", "x.wav"]), b"3").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"3");
    }

    #[test]
    fn write_from_streams_and_refuses_overwrite() {
        let d = scratch("stream");
        let out = OutDir::new(d.join("out"), false).unwrap();
        let p = out.write_from(&parts(&["m.mp4"]), &mut &b"abc"[..]).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"abc");
        assert!(out.write_from(&parts(&["m.mp4"]), &mut &b"x"[..]).is_err());
    }

    struct Failing(usize);
    impl io::Read for Failing {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.0 == 0 {
                return Err(io::Error::other("boom"));
            }
            let n = self.0.min(buf.len());
            buf[..n].fill(9);
            self.0 -= n;
            Ok(n)
        }
    }

    #[test]
    fn failed_copy_leaves_nothing_and_keeps_old_file() {
        let d = scratch("failcopy");
        let root = d.join("out");
        let out = OutDir::new(&root, false).unwrap();
        assert!(out.write_from(&parts(&["m.mp4"]), &mut Failing(5000)).is_err());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "partial or temp file left behind");
        let forced = OutDir::new(&root, true).unwrap();
        forced.write(&parts(&["m.mp4"]), b"good").unwrap();
        assert!(forced.write_from(&parts(&["m.mp4"]), &mut Failing(5000)).is_err());
        assert_eq!(fs::read(root.join("m.mp4")).unwrap(), b"good");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn rejects_unsafe_components() {
        let d = scratch("unsafe");
        let out = OutDir::new(d.join("out"), true).unwrap();
        for bad in ["", ".", "..", "a/b", "a\\b", "c:", "a\0b"] {
            assert!(out.write(&parts(&[bad, "f"]), b"x").is_err(), "{bad:?}");
            assert!(out.write(&parts(&["d", bad]), b"x").is_err(), "{bad:?}");
        }
        assert!(out.write(&[], b"x").is_err());
    }

    #[test]
    fn create_follows_the_same_rules_as_write() {
        let d = scratch("create");
        let out = OutDir::new(d.join("out"), false).unwrap();
        let (mut f, p) = out.create(&parts(&["a", "x.json"])).unwrap();
        io::Write::write_all(&mut f, b"1").unwrap();
        drop(f);
        assert_eq!(fs::read(&p).unwrap(), b"1");
        assert_eq!(
            out.create(&parts(&["a", "x.json"])).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(out.create(&parts(&["..", "x.json"])).is_err());
        let forced = OutDir::new(d.join("out"), true).unwrap();
        let (mut f, p) = forced.create(&parts(&["a", "x.json"])).unwrap();
        io::Write::write_all(&mut f, b"2").unwrap();
        drop(f);
        assert_eq!(fs::read(&p).unwrap(), b"2");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_directory_and_file_cannot_escape() {
        use std::os::unix::fs::symlink;
        let d = scratch("links");
        let outside = d.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let root = d.join("out");
        let out = OutDir::new(&root, true).unwrap();
        symlink(&outside, root.join("sound")).unwrap();
        assert!(out.write(&parts(&["sound", "x.wav"]), b"x").is_err());
        assert!(!outside.join("x.wav").exists());
        // A final-component symlink is neither followed nor replaced by --force.
        let target = outside.join("victim");
        fs::write(&target, b"keep").unwrap();
        symlink(&target, root.join("y.wav")).unwrap();
        assert!(out.write(&parts(&["y.wav"]), b"x").is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        // A dangling final symlink is not created through either.
        symlink(outside.join("new"), root.join("z.wav")).unwrap();
        assert!(out.write(&parts(&["z.wav"]), b"x").is_err());
        assert!(!outside.join("new").exists());
    }

    /// A file with just an XDVDFS volume descriptor at `partition`.
    fn fake_disc(path: &Path, partition: u64) {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = File::create(path).unwrap();
        f.seek(SeekFrom::Start(partition + 0x10000)).unwrap();
        f.write_all(b"MICROSOFT*XBOX*MEDIA").unwrap();
        f.write_all(&[0u8; 8]).unwrap();
    }

    #[test]
    fn force_never_replaces_disc_images() {
        let d = scratch("images");
        let out = OutDir::new(&d, true).unwrap();
        // Every partition offset xiso supports, including the Redump ones.
        for (i, off) in xiso::PARTITION_OFFSETS.into_iter().enumerate() {
            let name = format!("magic{i}.dat");
            fake_disc(&d.join(&name), off);
            let before = fs::read(d.join(&name)).unwrap();
            assert!(out.write(&parts(&[&name]), b"x").is_err(), "offset {off:#x}");
            assert_eq!(fs::read(d.join(&name)).unwrap(), before);
        }
        for name in ["new.iso", "new.XISO"] {
            assert!(out.write(&parts(&[name]), b"x").is_err(), "{name}");
            assert!(!d.join(name).exists());
        }
    }

    #[test]
    fn big_files_are_not_special() {
        let d = scratch("big");
        let big = File::create(d.join("big.dat")).unwrap();
        big.set_len(100 << 20).unwrap();
        OutDir::new(&d, true).unwrap().write(&parts(&["big.dat"]), b"x").unwrap();
        assert_eq!(fs::read(d.join("big.dat")).unwrap(), b"x");
    }

    #[test]
    fn globally_protected_discs_are_refused_by_every_outdir() {
        let d = scratch("global");
        let disc = d.join("odd-name.png");
        fs::write(&disc, b"disc").unwrap();
        protect(&disc);
        let out = OutDir::new(&d, true).unwrap(); // no .protecting()
        assert!(out.write(&parts(&["odd-name.png"]), b"x").is_err());
        assert_eq!(fs::read(&disc).unwrap(), b"disc");
    }

    #[test]
    fn force_revalidates_the_destination_before_replacing_it() {
        let d = scratch("revalidate");
        let out = OutDir::new(&d, true).unwrap();
        // The destination is a plain file when the walk checks it, and has
        // become a disc image by the time the data is ready to publish.
        fs::write(d.join("f.dat"), b"old").unwrap();
        let dest = d.join("f.dat");
        let r = out.write_with(&parts(&["f.dat"]), |f| {
            use std::io::Write;
            f.write_all(b"new")?;
            fake_disc(&dest, 0);
            Ok(())
        });
        assert!(r.is_err());
        assert_eq!(fs::read_dir(&d).unwrap().count(), 1, "temp file left behind");
        assert!(fs::read(&dest).unwrap().len() > 0x10000, "disc image was replaced");
    }

    #[test]
    fn create_dir_makes_validated_empty_directories() {
        let d = scratch("mkdir");
        let out = OutDir::new(d.join("out"), false).unwrap();
        let p = out.create_dir(&parts(&["a", "b"])).unwrap();
        assert!(p.is_dir() && p.ends_with("a/b"));
        out.create_dir(&parts(&["a", "b"])).unwrap(); // idempotent
        assert!(out.create_dir(&parts(&["a", ".."])).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("out"), d.join("out/l")).unwrap();
            assert!(out.create_dir(&parts(&["l", "z"])).is_err());
            assert!(!d.join("out/z").exists());
        }
    }

    #[test]
    fn write_with_streams_into_the_guarded_temp_file() {
        let d = scratch("with");
        let out = OutDir::new(&d, false).unwrap();
        let p = out
            .write_with(&parts(&["s", "f.bin"]), |f| {
                use std::io::Write;
                f.write_all(b"ab")?;
                f.write_all(b"cd")
            })
            .unwrap();
        assert_eq!(fs::read(p).unwrap(), b"abcd");
        assert!(out.write_with(&parts(&["s", "f.bin"]), |_| Ok(())).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_files_are_never_replaced() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("unreadable");
        let f = d.join("unreadable.dat");
        fake_disc(&f, 0);
        fs::set_permissions(&f, fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&f).is_ok() {
            return; // running as root: mode 000 does not hide the file
        }
        let out = OutDir::new(&d, true).unwrap();
        let e = out.write(&parts(&["unreadable.dat"]), b"x").unwrap_err();
        assert!(e.to_string().contains("cannot inspect"), "{e}");
        fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(fs::read(&f).unwrap().len() > 0x10000, "file was replaced");
    }

    #[test]
    fn publication_is_exclusive_and_never_clobbers_a_late_arrival() {
        // Without --force a file that appears while ours is being written
        // must survive: publication is a hard link, not a rename.
        let d = scratch("late");
        let out = OutDir::new(&d, false).unwrap();
        let dest = d.join("f.dat");
        let r = out.write_with(&parts(&["f.dat"]), |f| {
            use std::io::Write;
            f.write_all(b"ours")?;
            fs::write(&dest, b"theirs")
        });
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&dest).unwrap(), b"theirs");
        assert_eq!(fs::read_dir(&d).unwrap().count(), 1, "temp file left behind");
    }

    #[cfg(unix)]
    #[test]
    fn strict_root_refuses_symlinks_in_the_root_and_its_ancestors() {
        use std::os::unix::fs::symlink;
        let d = scratch("strict");
        let real = d.join("real");
        fs::create_dir_all(&real).unwrap();
        symlink(&real, d.join("linked")).unwrap();
        assert!(OutDir::new_strict(d.join("linked"), true).is_err());
        assert!(OutDir::new_strict(d.join("linked").join("new"), true).is_err());
        assert!(!real.join("new").exists());
        assert!(OutDir::new_strict(d.join("real").join("..").join("real"), false).is_err());
        fs::write(d.join("file"), b"x").unwrap();
        assert!(OutDir::new_strict(d.join("file"), false).is_err());
        // Real paths work, missing levels are created, and the lax
        // constructor keeps its documented behaviour.
        let out = OutDir::new_strict(d.join("a").join("b"), false).unwrap();
        out.write(&parts(&["x"]), b"1").unwrap();
        assert_eq!(fs::read(d.join("a").join("b").join("x")).unwrap(), b"1");
        assert!(OutDir::new(d.join("linked"), false).is_ok());
        // A dangling root link is not followed either.
        symlink(d.join("target"), d.join("dangling")).unwrap();
        assert!(OutDir::new_strict(d.join("dangling"), false).is_err());
        assert!(!d.join("target").exists());
    }

    #[test]
    fn only_exact_root_owned_system_aliases_are_trusted() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(system_alias(&p("/var"), Some(0), &p("private/var")), Some(p("/private/var")));
        assert_eq!(system_alias(&p("/tmp"), Some(0), &p("private/tmp")), Some(p("/private/tmp")));
        assert_eq!(system_alias(&p("/etc"), Some(0), &p("private/etc")), Some(p("/private/etc")));
        for (link, uid, target) in [
            ("/var", Some(501), "private/var"), // right shape, wrong owner
            ("/var", None, "private/var"),
            ("/var", Some(0), "/private/var"), // absolute, not the exact target
            ("/var", Some(0), "private/tmp"),  // another alias's target
            ("/var", Some(0), "private/var/../../Users/x"),
            ("/tmp", Some(0), "/Users/attacker/out"), // root-owned, points elsewhere
            ("/usr/local", Some(0), "private/var"), // looks trusted, not listed
            ("/private/var/x", Some(0), "private/var"),
            ("/var/x", Some(0), "private/var"),
        ] {
            assert_eq!(system_alias(&p(link), uid, &p(target)), None, "{link} -> {target}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn strict_root_resolves_system_aliases_without_writing() {
        // Existing system directories: nothing is created, and the stored
        // root is the resolved real path.
        for (alias, real) in [("/var", "/private/var"), ("/etc", "/private/etc")] {
            assert_eq!(OutDir::new_strict(alias, false).unwrap().root, PathBuf::from(real));
        }
    }
}
