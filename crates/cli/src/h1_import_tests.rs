use super::*;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("h2-h1import-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn stems(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn jobs_are_campaign_levels_in_play_order_then_known_multiplayer_maps_in_both_sandboxes() {
    let j = jobs(&stems(&[
        "ui", "bloodgulch", "D40", "a10", "beavercreek", "bitmaps", "a30", "notamap",
    ]));
    let keys: Vec<String> = j.iter().map(Job::key).collect();
    assert_eq!(
        keys,
        [
            "a10",
            "a30",
            "d40",
            "beavercreek/h2",
            "beavercreek/h1",
            "bloodgulch/h2",
            "bloodgulch/h1"
        ]
    );
    assert!(jobs(&stems(&["ui"])).is_empty());
}

#[test]
fn each_job_writes_where_the_game_reads() {
    let d = Path::new("/data");
    assert_eq!(
        Job::Campaign("a30").output(d),
        Path::new("/data/campaigns/halo1/a30.map")
    );
    assert_eq!(
        Job::Multiplayer("bloodgulch", Sandbox::Halo1).output(d),
        Path::new("/data/maps/h1_bloodgulch/h1/h1_bloodgulch.map")
    );
    // The map packages' folder is the one boot::new_maps reads.
    assert_eq!(
        content::store::maps_dir(d),
        boot::map_override::packages_dir(d)
    );
}

fn stamp() -> Stamp {
    Stamp {
        converter: CONVERTER_VERSION,
        sha256: "ab".repeat(32),
        size: 3_625_713_664,
        mtime: 1_759_777_380,
        done: BTreeMap::from([
            ("a30".into(), Ok(())),
            ("bloodgulch/h1".into(), Err("tag memory\nfull".into())),
        ]),
    }
}

#[test]
fn the_stamp_reads_back() {
    let s = stamp();
    let back = Stamp::parse(&s.to_text()).unwrap();
    assert_eq!(back.done["bloodgulch/h1"], Err("tag memory full".into()));
    assert_eq!(
        Stamp {
            done: BTreeMap::new(),
            ..back.clone()
        },
        Stamp {
            done: BTreeMap::new(),
            ..s.clone()
        }
    );
    assert_eq!(back.done["a30"], Ok(()));
    assert_eq!(Stamp::parse(""), None);
    assert_eq!(Stamp::parse("converter=x\nsha256=a\nsize=1\nmtime=1\n"), None);
    assert_eq!(Stamp::parse("what=1\n"), None);
}

#[test]
fn an_unchanged_disc_is_not_hashed_and_its_outcomes_carry_on() {
    let old = stamp();
    let s = fresh_stamp(Some(&old), old.size, old.mtime, || panic!("hashed")).unwrap();
    assert_eq!(s, old);
}

#[test]
fn a_touched_disc_is_hashed_and_kept_when_the_hash_is_the_same() {
    let old = stamp();
    let s = fresh_stamp(Some(&old), old.size, old.mtime + 1, || Ok(old.sha256.clone())).unwrap();
    assert_eq!((s.done.len(), s.mtime), (2, old.mtime + 1));
    // Another disc: start again.
    let s = fresh_stamp(Some(&old), old.size, old.mtime + 1, || Ok("cd".repeat(32))).unwrap();
    assert!(s.done.is_empty());
    let s = fresh_stamp(None, 1, 2, || Ok("ef".into())).unwrap();
    assert_eq!((s.sha256.as_str(), s.converter), ("ef", CONVERTER_VERSION));
}

#[test]
fn a_new_converter_version_converts_everything_again() {
    let old = Stamp {
        converter: CONVERTER_VERSION - 1,
        ..stamp()
    };
    let s = fresh_stamp(Some(&old), old.size, old.mtime, || panic!("hashed")).unwrap();
    assert_eq!(s.converter, CONVERTER_VERSION);
    assert!(s.done.is_empty());
}

#[test]
fn pending_is_what_is_not_done_or_whose_map_is_gone() {
    let data = scratch("pending");
    let all = jobs(&stems(&["a10", "a30", "bloodgulch"]));
    let mut s = stamp();
    // a30 is stamped done but its map isn't there: to do again. The
    // failed bloodgulch/h1 isn't retried.
    let keys = |v: Vec<Job>| v.iter().map(Job::key).collect::<Vec<_>>();
    assert_eq!(keys(pending(&all, &s, &data)), ["a10", "a30", "bloodgulch/h2"]);
    let a30 = Job::Campaign("a30").output(&data);
    std::fs::create_dir_all(a30.parent().unwrap()).unwrap();
    std::fs::write(&a30, b"x").unwrap();
    assert_eq!(keys(pending(&all, &s, &data)), ["a10", "bloodgulch/h2"]);
    s.done.clear();
    assert_eq!(pending(&all, &s, &data).len(), 4);
    let _ = std::fs::remove_dir_all(&data);
}

/// Fails `a30`, panics on `bloodgulch/h1`, converts the rest.
struct Flaky(Vec<String>);
impl Converter for Flaky {
    fn convert(&mut self, job: &Job) -> Result<()> {
        self.0.push(job.key());
        match job.key().as_str() {
            "a30" => bail!("no room"),
            "bloodgulch/h1" => panic!("converter bug"),
            _ => Ok(()),
        }
    }
}

#[test]
fn a_failing_or_panicking_map_is_recorded_and_the_rest_go_on() {
    let todo = jobs(&stems(&["a10", "a30", "bloodgulch", "d40"]));
    let mut s = Stamp::default();
    let mut conv = Flaky(Vec::new());
    let mut saves = Vec::new();
    let mut lines = Vec::new();
    let sum = run_jobs(
        &todo,
        &mut s,
        &mut conv,
        &mut |st| {
            saves.push(st.done.len());
            Ok(())
        },
        &mut |l| lines.push(l),
    );
    assert_eq!(
        sum,
        Summary {
            converted: 3,
            failed: 2,
            skipped: 0
        }
    );
    assert_eq!(conv.0.len(), 5, "every map was tried");
    assert_eq!(saves, [1, 2, 3, 4, 5], "saved after each map");
    assert_eq!(s.done["a30"], Err("no room".into()));
    assert!(
        s.done["bloodgulch/h1"]
            .as_ref()
            .is_err_and(|e| e.contains("converter bug"))
    );
    assert_eq!(s.done["d40"], Ok(()));
    assert_eq!(lines.len(), 5, "{lines:?}");
    assert!(lines[0].starts_with("[1/5] campaign a10: done"), "{lines:?}");
    assert!(
        lines[1].starts_with("[2/5] campaign a30: failed: no room"),
        "{lines:?}"
    );
    assert!(
        lines[4].starts_with("[5/5] multiplayer bloodgulch (Halo 1 sandbox): failed: panicked"),
        "{lines:?}"
    );
}

#[test]
fn auto_import_can_be_switched_off() {
    assert!(auto_enabled_with(false, None));
    assert!(auto_enabled_with(false, Some("1")));
    assert!(!auto_enabled_with(true, None));
    for v in ["0", "false", "No", " off "] {
        assert!(!auto_enabled_with(false, Some(v)), "{v}");
    }
}

#[test]
fn discovers_halo1_iso_beside_the_game_disc() {
    let d = scratch("discover");
    let game = d.join("halo2.iso");
    std::fs::write(&game, b"game").unwrap();
    assert_eq!(discover(&game), None);
    // Not a Halo 1 disc and not named so: not it.
    std::fs::write(d.join("other.iso"), b"junk").unwrap();
    std::fs::write(d.join("mappack.iso"), b"junk").unwrap();
    assert_eq!(discover(&game), None);
    std::fs::write(d.join("HALO1.ISO"), b"named").unwrap();
    assert_eq!(discover(&game), Some(d.join("HALO1.ISO")));
    // The game disc itself never is.
    assert_eq!(discover(&d.join("HALO1.ISO")), None);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn discovers_a_halo1_disc_by_its_xbe_under_another_name() {
    let d = scratch("discover-xbe");
    let game = d.join("halo2.iso");
    std::fs::write(&game, b"game").unwrap();
    std::fs::write(d.join("combat evolved.iso"), h1map::testing::campaign_disc()).unwrap();
    assert_eq!(discover(&game), Some(d.join("combat evolved.iso")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn auto_import_without_a_halo1_disc_does_nothing() {
    let d = scratch("auto-none");
    let game = d.join("halo2.iso");
    std::fs::write(&game, b"game").unwrap();
    let data = d.join("data");
    auto_import(&game, &data);
    assert!(!data.exists());
    // With one but a game "disc" that is no image: a warning, nothing written.
    std::fs::write(d.join("halo1.iso"), h1map::testing::campaign_disc()).unwrap();
    auto_import(&game, &data);
    assert!(!data.exists());
    let _ = std::fs::remove_dir_all(&d);
}

/// Against your discs: the whole import into a scratch data root, then what
/// the game reads from it at boot: `boot::new_maps` puts every converted
/// multiplayer map in the map list, the campaign picker's Halo 1 lists the
/// ten levels in order with their names, and `vfs::CampaignMapsBackend`
/// serves each level's map at `maps\<level>.map`. A second run converts
/// nothing.
///
/// `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test --release -p cli
/// --bin h2 h1_import::tests::real -- --ignored --nocapture` (H1_JOBS=a30,
/// bloodgulch/h2 for some; H1_IMPORT_DATA=DIR to keep the output).
#[test]
#[ignore = "reads the discs named by H1_ISO and H2_DISC; converts every map (minutes)"]
fn real_import_is_found_by_the_game() {
    use vfs::Backend;
    let h1 = PathBuf::from(std::env::var("H1_ISO").expect("H1_ISO"));
    let h2 = PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
    let data = std::env::var_os("H1_IMPORT_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| scratch("real"));
    let only: Option<Vec<String>> = std::env::var("H1_JOBS")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect());
    let mut lines = Vec::new();
    let sum = if let Some(only) = &only {
        // Some jobs only: the converter directly.
        let f = File::open(&h1).unwrap();
        let mut iso = Xiso::open(BufReader::new(f)).unwrap();
        let disc = Disc::open(&mut iso).unwrap();
        let st: Vec<String> = disc.maps.iter().map(|e| map_stem(e).to_owned()).collect();
        let todo: Vec<Job> = jobs(&st)
            .into_iter()
            .filter(|j| only.contains(&j.key()) || only.iter().any(|o| *o == j.key().split('/').next().unwrap()))
            .collect();
        let mut conv = DiscConverter {
            h1_path: h1.clone(),
            iso,
            disc,
            h2: h2.clone(),
            data: data.clone(),
            scripting: None,
        };
        let mut s = Stamp::default();
        run_jobs(&todo, &mut s, &mut conv, &mut |_| Ok(()), &mut |l| {
            eprintln!("{l}");
            lines.push(l)
        })
    } else {
        let r = Request {
            h1_iso: &h1,
            h2_disc: &h2,
            data: &data,
            force: true,
            dry_run: false,
        };
        let sum = import(&r, &mut |l| {
            eprintln!("{l}");
            lines.push(l)
        })
        .unwrap();
        // Again: nothing to do.
        let again = import(
            &Request {
                force: false,
                ..r
            },
            &mut |l| eprintln!("{l}"),
        )
        .unwrap();
        assert_eq!((again.converted, again.failed), (0, 0));
        assert_eq!(again.skipped, sum.converted + sum.failed);
        sum
    };
    eprintln!("{sum:?}");
    assert_eq!(sum.failed, 0, "{lines:#?}");

    // Multiplayer: in the map list.
    let mp: Vec<String> = std::fs::read_dir(content::store::maps_dir(&data))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("h1_"))
                .collect()
        })
        .unwrap_or_default();
    if !mp.is_empty() {
        let mut disc = boot::new_maps::IsoDisc::new(&h2);
        let r = boot::new_maps::install(&data, &mut disc).unwrap();
        eprintln!("{}", r.line().unwrap_or_default());
        assert!(r.skipped.is_empty(), "{:?}", r.skipped);
        let listed: Vec<&str> = r.installed.iter().map(|(n, ..)| n.as_str()).collect();
        for m in &mp {
            assert!(listed.contains(&m.as_str()), "{m} not in the map list: {listed:?}");
        }
    }

    // Campaign: the picker, the levels, the files the VFS serves.
    let levels: Vec<&str> = CAMPAIGN_LEVELS
        .iter()
        .copied()
        .filter(|l| Job::Campaign(l).output(&data).is_file())
        .collect();
    if !levels.is_empty() {
        let state = boot::campaigns::boot_state(&data, &h2).expect("Halo 1 installed");
        let picker = boot::campaigns::Picker::new(&state.built);
        assert_eq!(picker.rows[1].1, "Halo: Combat Evolved");
        let b = &state.built[0];
        let got: Vec<&str> = b.levels.iter().map(|l| l.map.as_str()).collect();
        assert_eq!(got, levels, "play order");
        assert!(b.levels.iter().all(|l| !l.names[0].is_empty()));
        if only.is_none() {
            assert_eq!(got, CAMPAIGN_LEVELS);
        }
        let maps = boot::campaigns::host_maps(&data, &state);
        let mut fs = vfs::CampaignMapsBackend::new(vfs::open_disc(&h2).unwrap(), maps);
        for l in &b.levels {
            let base = l.scenario.rsplit('\\').next().unwrap();
            let path = format!("maps\\{base}.map");
            let file = Job::Campaign(&l.map).output(&data);
            let meta = fs.stat(&path).unwrap();
            assert_eq!(meta.end_of_file, std::fs::metadata(&file).unwrap().len(), "{path}");
            let mut head = [0u8; 0x800];
            fs.read_at(&path, 0, &mut head).unwrap();
            let want = std::fs::read(&file).unwrap();
            assert_eq!(&head[..], &want[..0x800], "{path}");
            let m = map::Map::from_reader(Cursor::new(want)).unwrap();
            assert_eq!(m.header().scenario_path, l.scenario, "{path}");
            // `--level` finds it too.
            let r = boot::level_boot::resolve_in(&h2, Some(&data), &l.map, boot::level_boot::Difficulty::Normal)
                .unwrap();
            assert_eq!((r.campaign, r.map), (b.campaign_id, l.map_id));
        }
    }
    if std::env::var_os("H1_IMPORT_DATA").is_none() {
        let _ = std::fs::remove_dir_all(&data);
    }
}
