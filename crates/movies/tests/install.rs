mod support;
use movies::install::{InstallOptions, InstallStatus, discover, install};
use movies::{Manifest, locate};
use std::fs;
use support::{Temp, disc, media};

#[test]
fn discovers_case_insensitively_and_ignores_other_files() {
    let mut iso = disc(&[
        ("a.BIK", b"synthetic"),
        ("b.txt", b"text"),
        ("c.bik", b"synthetic"),
    ]);
    let files = discover(&mut iso).unwrap();
    assert_eq!(
        files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["/bink/a.BIK", "/bink/c.bik"]
    );
}

#[test]
fn absent_ffmpeg_does_not_create_destination_or_manifest() {
    let temp = Temp::new();
    let dir = temp.0.join("data/movies");
    let mut options = InstallOptions::new(&dir);
    options.ffmpeg = temp.0.join("no-ffmpeg");
    let report = install(&mut disc(&[("a.bik", b"generated")]), &options).unwrap();
    assert_eq!(report.status, InstallStatus::FfmpegAbsent);
    assert!(!dir.exists());
}

#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn transcode_resume_detect_corruption_and_source_change() {
    let temp = Temp::new();
    let bytes = media(
        &temp.0.join("source.nut"),
        "testsrc2=size=32x24:rate=30000/1001",
    );
    let changed = media(
        &temp.0.join("changed.nut"),
        "color=green:size=32x24:rate=30000/1001",
    );
    let options = InstallOptions::new(temp.0.join("data/movies"));
    let mut iso = disc(&[("a.BIK", &bytes), ("c.bik", &changed)]);
    let first = install(&mut iso, &options).unwrap();
    assert_eq!(first.status, InstallStatus::Complete);
    assert_eq!(first.converted.len(), 2);
    let before = fs::read(options.data_dir.join("movies.json")).unwrap();
    let resumed = install(&mut iso, &options).unwrap();
    assert_eq!((resumed.converted.len(), resumed.reused.len()), (0, 2));
    assert_eq!(
        before,
        fs::read(options.data_dir.join("movies.json")).unwrap()
    );
    let path = locate(&options.data_dir, "a").unwrap();
    let mut corrupt = fs::read(&path).unwrap();
    corrupt[0] ^= 0xff;
    fs::write(&path, corrupt).unwrap();
    assert!(
        locate(&options.data_dir, "a").is_none(),
        "same-size corruption must be detected"
    );
    let repaired = install(&mut iso, &options).unwrap();
    assert_eq!((repaired.converted.len(), repaired.reused.len()), (1, 1));
    assert!(locate(&options.data_dir, "a").is_some());
    let mut new_disc = disc(&[("a.BIK", &changed), ("c.bik", &changed)]);
    let updated = install(&mut new_disc, &options).unwrap();
    assert_eq!((updated.converted.len(), updated.reused.len()), (1, 1));
    let manifest = Manifest::load(&options.data_dir).unwrap().unwrap();
    for entry in &manifest.movies {
        assert!(entry.output_name.ends_with(".mov"));
        assert!(entry.duration_secs > 0.0);
        assert!(manifest.output_matches(&options.data_dir, entry).unwrap());
    }
    assert!(!fs::read_dir(&options.data_dir).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".install-")
    }));
}

#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn failure_preserves_completed_movies_and_resume_finishes_later() {
    let temp = Temp::new();
    let bytes = media(&temp.0.join("source.nut"), "testsrc2=size=32x24:rate=30");
    let options = InstallOptions::new(temp.0.join("data/movies"));
    let mut bad = disc(&[("a.bik", &bytes), ("b.bik", b"invalid synthetic media")]);
    assert!(install(&mut bad, &options).is_err());
    assert!(locate(&options.data_dir, "a").is_some());
    assert!(locate(&options.data_dir, "b").is_none());
    let mut repaired = disc(&[("a.bik", &bytes), ("b.bik", &bytes)]);
    let report = install(&mut repaired, &options).unwrap();
    assert_eq!((report.converted.len(), report.reused.len()), (1, 1));
    // Cross-process lock file remains, but the OS lock is released after failure.
    assert!(options.data_dir.join(".install.lock").exists());
}

#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn invalid_manifest_is_not_silently_overwritten() {
    let temp = Temp::new();
    let options = InstallOptions::new(temp.0.join("data"));
    fs::create_dir(&options.data_dir).unwrap();
    fs::write(options.data_dir.join("movies.json"), "broken").unwrap();
    assert!(install(&mut disc(&[("a.bik", b"generated")]), &options).is_err());
    assert_eq!(
        fs::read_to_string(options.data_dir.join("movies.json")).unwrap(),
        "broken"
    );
}

#[test]
#[cfg(unix)]
fn unsuitable_ffmpeg_is_optional_and_leaves_existing_data_alone() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    let mut options = InstallOptions::new(temp.0.join("data"));
    fs::create_dir(&options.data_dir).unwrap();
    let manifest_path = options.data_dir.join("movies.json");
    fs::write(&manifest_path, "existing data").unwrap();
    options.ffmpeg = temp.0.join("ffmpeg-stub");
    fs::write(&options.ffmpeg, "#!/bin/sh\ncase \"$1\" in\n-version) echo 'ffmpeg version synthetic' ;;\n*) exit 1 ;;\nesac\n").unwrap();
    fs::set_permissions(&options.ffmpeg, fs::Permissions::from_mode(0o700)).unwrap();
    let report = install(&mut disc(&[("a.bik", b"synthetic")]), &options).unwrap();
    assert!(matches!(report.status, InstallStatus::FfmpegUnusable(_)));
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), "existing data");
}

#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn existing_installer_lock_prevents_concurrent_publication() {
    let temp = Temp::new();
    let options = InstallOptions::new(temp.0.join("data"));
    fs::create_dir(&options.data_dir).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(options.data_dir.join(".install.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(install(&mut disc(&[("a.bik", b"synthetic")]), &options).is_err());
    assert!(!options.data_dir.join("movies.json").exists());
}

#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn legacy_manifest_is_readable_but_not_trusted_for_resume() {
    let temp = Temp::new();
    let bytes = media(&temp.0.join("source.nut"), "color=blue:size=32x24:rate=30");
    let options = InstallOptions::new(temp.0.join("data"));
    let mut iso = disc(&[("a.bik", &bytes)]);
    install(&mut iso, &options).unwrap();
    let mut legacy = Manifest::load(&options.data_dir).unwrap().unwrap();
    legacy.output_sha256.clear();
    fs::write(options.data_dir.join("movies.json"), legacy.to_json()).unwrap();
    assert!(locate(&options.data_dir, "a").is_some());
    let report = install(&mut iso, &options).unwrap();
    assert_eq!((report.converted.len(), report.reused.len()), (1, 0));
    assert!(
        !Manifest::load(&options.data_dir)
            .unwrap()
            .unwrap()
            .output_sha256
            .is_empty()
    );
}
