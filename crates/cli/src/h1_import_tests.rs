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
fn jobs_are_levels_in_play_order_then_multiplayer_maps_in_both_sandboxes_then_halo2_assets_levels()
{
    let j = jobs(&stems(&[
        "ui",
        "bloodgulch",
        "D40",
        "a10",
        "beavercreek",
        "bitmaps",
        "a30",
        "notamap",
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
            "bloodgulch/h1",
            "a10/h2",
            "a30/h2",
            "d40/h2"
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
    // The Halo 2-assets variant goes where the campaign maps backend
    // looks for it (boot::h1_variants::campaign_alternate).
    assert_eq!(
        Job::CampaignHalo2("a30").output(d),
        Path::new("/data/campaigns/halo1/h2/a30.map")
    );
    assert_eq!(
        Job::CampaignHalo2("a30").output(d),
        boot::h1_variants::campaign_alternate(&Job::Campaign("a30").output(d)).unwrap()
    );
    assert_eq!(Job::CampaignHalo2("a30").key(), "a30/h2");
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
    assert_eq!(
        Stamp::parse("converter=x\nsha256=a\nsize=1\nmtime=1\n"),
        None
    );
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
    let s = fresh_stamp(Some(&old), old.size, old.mtime + 1, || {
        Ok(old.sha256.clone())
    })
    .unwrap();
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
    assert_eq!(
        keys(pending(&all, &s, &data)),
        ["a10", "a30", "bloodgulch/h2", "a10/h2", "a30/h2"]
    );
    let a30 = Job::Campaign("a30").output(&data);
    std::fs::create_dir_all(a30.parent().unwrap()).unwrap();
    std::fs::write(&a30, b"x").unwrap();
    assert_eq!(
        keys(pending(&all, &s, &data)),
        ["a10", "bloodgulch/h2", "a10/h2", "a30/h2"]
    );
    s.done.clear();
    assert_eq!(pending(&all, &s, &data).len(), 6);
    let _ = std::fs::remove_dir_all(&data);
}

/// Fails `a30`, panics on `bloodgulch/h1`, converts the rest.
struct Flaky(std::sync::Mutex<Vec<String>>);
impl Converter for Flaky {
    fn convert(&self, job: &Job) -> Result<()> {
        self.0.lock().unwrap().push(job.key());
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
    let conv = Flaky(Default::default());
    let mut saves = Vec::new();
    let mut lines = Vec::new();
    let (sum, times) = run_jobs(
        &todo,
        1,
        &mut s,
        &conv,
        &mut |st| {
            saves.push(st.done.len());
            Ok(())
        },
        &mut |l| lines.push(l),
    );
    assert_eq!(
        sum,
        Summary {
            converted: 6,
            failed: 2,
            skipped: 0
        }
    );
    assert_eq!(conv.0.lock().unwrap().len(), 8, "every map was tried");
    assert_eq!(times.len(), 6, "a time for each map converted");
    assert_eq!(saves, [1, 2, 3, 4, 5, 6, 7, 8], "saved after each map");
    assert_eq!(s.done["a30"], Err("no room".into()));
    assert!(
        s.done["bloodgulch/h1"]
            .as_ref()
            .is_err_and(|e| e.contains("converter bug"))
    );
    assert_eq!(s.done["d40"], Ok(()));
    assert_eq!(s.done["a30/h2"], Ok(()));
    assert_eq!(lines.len(), 8, "{lines:?}");
    assert!(
        lines[0].starts_with("[1/8] campaign a10: done"),
        "{lines:?}"
    );
    assert!(
        lines[1].starts_with("[2/8] campaign a30: failed: no room"),
        "{lines:?}"
    );
    assert!(
        lines[4].starts_with("[5/8] multiplayer bloodgulch (Halo 1 sandbox): failed: panicked"),
        "{lines:?}"
    );
    assert!(
        lines[6].starts_with("[7/8] campaign a30 (Halo 2 assets): done"),
        "{lines:?}"
    );
}

/// On several workers: every job runs once, each outcome is in the stamp,
/// a line and a save for each, whatever order they finish in.
#[test]
fn jobs_run_on_several_workers_each_once() {
    let todo = jobs(&stems(&["a10", "a30", "bloodgulch", "d40", "sidewinder"]));
    let mut s = Stamp::default();
    let conv = Flaky(Default::default());
    let mut saves = 0;
    let mut lines = Vec::new();
    let (sum, times) = run_jobs(
        &todo,
        4,
        &mut s,
        &conv,
        &mut |_| {
            saves += 1;
            Ok(())
        },
        &mut |l| lines.push(l),
    );
    assert_eq!((sum.converted, sum.failed), (todo.len() - 2, 2));
    let mut tried = conv.0.lock().unwrap().clone();
    tried.sort();
    let mut want: Vec<String> = todo.iter().map(Job::key).collect();
    want.sort();
    assert_eq!(tried, want, "each job once");
    assert_eq!(s.done.len(), todo.len());
    assert_eq!(saves, todo.len());
    assert_eq!(lines.len(), todo.len());
    assert_eq!(times.len(), todo.len() - 2);
    let report = timing_text(Duration::from_secs(3), 4, &times);
    assert!(report.contains("4 workers"), "{report}");
    assert!(report.contains("sidewinder/h2"), "{report}");
}

#[test]
fn jobs_start_campaign_first_each_maps_first_job_before_its_second_biggest_first() {
    let todo = jobs(&stems(&["a10", "a30", "bloodgulch", "sidewinder"]));
    let size = |m: &str| match m {
        "a30" => 9,
        "sidewinder" => 5,
        _ => 1,
    };
    let keys: Vec<String> = schedule(&todo, size).iter().map(Job::key).collect();
    assert_eq!(
        keys,
        [
            "a30",
            "a10",
            "a30/h2",
            "a10/h2",
            "sidewinder/h2",
            "bloodgulch/h2",
            "sidewinder/h1",
            "bloodgulch/h1"
        ]
    );
    // Only what is to do.
    let some = [todo[1], todo[4]];
    assert_eq!(schedule(&some, size), some);
}

#[test]
fn workers_follow_the_cores_the_memory_and_the_override() {
    assert_eq!(workers_with(None, 10, Some(64 << 30)), 10);
    assert_eq!(workers_with(None, 10, Some(16 << 30)), 5);
    assert_eq!(workers_with(None, 10, Some(1 << 30)), 1);
    assert_eq!(workers_with(None, 4, None), 4);
    assert_eq!(workers_with(Some("2"), 10, Some(64 << 30)), 2);
    assert_eq!(workers_with(Some("0"), 10, None), 1);
    assert_eq!(workers_with(Some("x"), 3, None), 3);
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
    std::fs::write(
        d.join("combat evolved.iso"),
        h1map::testing::campaign_disc(),
    )
    .unwrap();
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
            .filter(|j| {
                only.contains(&j.key())
                    || only
                        .iter()
                        .any(|o| *o == j.key().split('/').next().unwrap())
            })
            .collect();
        let conv = DiscConverter::new(h1.clone(), iso, disc, h2.clone(), data.clone());
        let mut s = Stamp::default();
        run_jobs(&todo, workers(), &mut s, &conv, &mut |_| Ok(()), &mut |l| {
            eprintln!("{l}");
            lines.push(l)
        })
        .0
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
        let again = import(&Request { force: false, ..r }, &mut |l| eprintln!("{l}")).unwrap();
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
            assert!(
                listed.contains(&m.as_str()),
                "{m} not in the map list: {listed:?}"
            );
        }
        // Each card is Halo 1's own: its name, description and picture.
        for m in &mp {
            let dir = content::store::maps_dir(&data).join(m);
            let text = std::fs::read_to_string(dir.join(content::manifest::MANIFEST_FILE)).unwrap();
            let man = content::Manifest::from_toml(&text).unwrap();
            assert_eq!(
                man.package.image.as_deref(),
                Some(h1_cmd::PREVIEW_FILE),
                "{m}"
            );
            assert!(
                man.package
                    .description
                    .as_ref()
                    .is_some_and(|d| !d.is_empty()),
                "{m}"
            );
            let png = std::fs::read(dir.join(h1_cmd::PREVIEW_FILE)).unwrap();
            assert_eq!(content::manifest::image_signature(&png), Some("png"), "{m}");
            eprintln!(
                "  {m}: {:?}, {:?}",
                man.package.name, man.package.description
            );
            if m == "h1_bloodgulch" {
                assert_eq!(man.package.name, "Blood Gulch");
            }
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
        assert_eq!(picker.rows[0].1, "Halo: Combat Evolved");
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
            assert_eq!(
                meta.end_of_file,
                std::fs::metadata(&file).unwrap().len(),
                "{path}"
            );
            let mut head = [0u8; 0x800];
            fs.read_at(&path, 0, &mut head).unwrap();
            let want = std::fs::read(&file).unwrap();
            assert_eq!(&head[..], &want[..0x800], "{path}");
            let m = map::Map::from_reader(Cursor::new(want)).unwrap();
            assert_eq!(m.header().scenario_path, l.scenario, "{path}");
            // `--level` finds it too.
            let r = boot::level_boot::resolve_in(
                &h2,
                Some(&data),
                &l.map,
                boot::level_boot::Difficulty::Normal,
            )
            .unwrap();
            assert_eq!((r.campaign, r.map), (b.campaign_id, l.map_id));
        }
        // With Halo 2's assets chosen, each level's `h2/` map is served
        // in its place (docs/halo1/campaign.md §5).
        let choice = vfs::SandboxChoice::default();
        choice.set_halo1(false);
        let maps = boot::campaigns::host_maps(&data, &state);
        let alternates = boot::h1_variants::campaign_alternates(&maps);
        let mut fs = vfs::CampaignMapsBackend::new(vfs::open_disc(&h2).unwrap(), maps)
            .with_assets(choice, alternates);
        for l in &b.levels {
            let file = Job::CampaignHalo2(&l.map).output(&data);
            if !file.is_file() {
                assert!(only.is_some(), "{}: no Halo 2-assets map", l.map);
                continue;
            }
            let base = l.scenario.rsplit('\\').next().unwrap();
            let path = format!("maps\\{base}.map");
            let mut head = [0u8; 0x800];
            fs.read_at(&path, 0, &mut head).unwrap();
            let want = std::fs::read(&file).unwrap();
            assert_eq!(&head[..], &want[..0x800], "{path} (Halo 2 assets)");
            let m = map::Map::from_reader(Cursor::new(want)).unwrap();
            assert_eq!(
                m.header().scenario_path,
                l.scenario,
                "{path} (Halo 2 assets)"
            );
        }
    }
    if std::env::var_os("H1_IMPORT_DATA").is_none() {
        let _ = std::fs::remove_dir_all(&data);
    }
}

/// Every multiplayer map on the disc in both sandboxes through the
/// import's own conversion (`DiscConverter::multiplayer`, what a
/// multiplayer job builds before writing it), in memory: each builds.
/// The Halo 1 sandbox's objects, collections and the sounds the objects
/// play must keep the datums the placements name (a sound installed
/// before the collections moved them).
///
/// `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test --release -p cli
/// --bin h2 h1_import::tests::every_multiplayer_job -- --ignored`
/// (H1_MAPS=bloodgulch,sidewinder for some).
#[test]
#[ignore = "reads the discs named by H1_ISO and H2_DISC; converts every multiplayer map"]
fn every_multiplayer_job_builds_in_both_sandboxes() {
    let h1 = PathBuf::from(std::env::var("H1_ISO").expect("H1_ISO"));
    let h2 = PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
    let only = std::env::var("H1_MAPS").ok();
    let f = File::open(&h1).unwrap();
    let mut iso = Xiso::open(BufReader::new(f)).unwrap();
    let disc = Disc::open(&mut iso).unwrap();
    let stems: Vec<String> = disc.maps.iter().map(|e| map_stem(e).to_owned()).collect();
    let conv = DiscConverter::new(h1.clone(), iso, disc, h2, PathBuf::new());
    let mut built = 0;
    for job in jobs(&stems) {
        let Job::Multiplayer(name, sandbox) = job else {
            continue;
        };
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|m| m == name))
        {
            continue;
        }
        conv.multiplayer(name, sandbox)
            .unwrap_or_else(|e| panic!("{name} {sandbox:?}: {e:#}"));
        built += 1;
    }
    println!("{built} multiplayer jobs built");
    assert!(built > 0);
}

/// Every job the import runs (the 10 levels in both asset variants and the
/// 13 multiplayer maps in both sandboxes: 46 on the retail disc) built
/// from scratch, as the import builds it, in memory: nothing of the base
/// pinned, a dense index with no empty slot, no reference nulled, and the
/// file and tag memory inside the engine's bounds (the native campaign
/// slot, `map::writer::NATIVE_CAMPAIGN_FILE_LIMIT`; the multiplayer cache
/// limit). A map that cannot be built so is the conversion's error, so
/// this also proves no job relies on a pinned or pruned map.
///
/// `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test --release -p cli
/// --bin h2 h1_import::tests::every_job_builds_from_scratch -- --ignored
/// --nocapture` (H1_JOBS=d40,d40/h2,bloodgulch for some).
#[test]
#[ignore = "reads the discs named by H1_ISO and H2_DISC; converts every map (minutes)"]
fn every_job_builds_from_scratch() {
    let h1 = PathBuf::from(std::env::var("H1_ISO").expect("H1_ISO"));
    let h2 = PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
    let only: Option<Vec<String>> = std::env::var("H1_JOBS")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect());
    let f = File::open(&h1).unwrap();
    let mut iso = Xiso::open(BufReader::new(f)).unwrap();
    let disc = Disc::open(&mut iso).unwrap();
    let stems: Vec<String> = disc.maps.iter().map(|e| map_stem(e).to_owned()).collect();
    let todo: Vec<Job> = jobs(&stems)
        .into_iter()
        .filter(|j| only.as_ref().is_none_or(|o| o.contains(&j.key())))
        .collect();
    if only.is_none() {
        assert_eq!(todo.len(), 46, "the retail disc's jobs");
    }
    let conv = DiscConverter::new(h1.clone(), iso, disc, h2, PathBuf::new());
    conv.expect(&todo);
    let campaign_max = map::writer::max_metadata_size(map::CacheType::Campaign).unwrap();
    let mp_file_max = map::writer::cache_size_limit(map::CacheType::Multiplayer).unwrap();
    // One job's problems, empty when it built from scratch and fits.
    let check = |job: &Job| -> Vec<String> {
        let mut bad = Vec::new();
        let bytes = match *job {
            Job::Campaign(level) | Job::CampaignHalo2(level) => {
                let assets = if matches!(job, Job::Campaign(_)) {
                    h1campaign::h2assets::Assets::Halo1
                } else {
                    h1campaign::h2assets::Assets::Halo2
                };
                let done = match conv.campaign(level, assets) {
                    Ok(d) => d,
                    Err(e) => return vec![format!("{e:#}")],
                };
                let r = &done.report;
                match &r.scratch {
                    Some(s) if s.dangling.is_empty() => {}
                    Some(s) => bad.push(format!("{} nulled: {:?}", s.dangling.len(), s.dangling)),
                    None => bad.push(format!("not from scratch: {:?}", r.dropped)),
                }
                if !r.fresh {
                    bad.push("written pinned".into());
                }
                if r.memory > campaign_max {
                    bad.push(format!("tag memory {:#x} over {campaign_max:#x}", r.memory));
                }
                let limit = u64::from(map::writer::NATIVE_CAMPAIGN_FILE_LIMIT);
                if done.bytes.len() as u64 > limit {
                    bad.push(format!("file {:#x} over {limit:#x}", done.bytes.len()));
                }
                println!(
                    "{}: tag memory {:#x}, file {:#x}, lean {:?}",
                    job.key(),
                    r.memory,
                    done.bytes.len(),
                    r.lean
                );
                done.bytes
            }
            Job::Multiplayer(name, sandbox) => {
                let f = match conv.multiplayer(name, sandbox) {
                    Ok((_, f)) => f,
                    Err(e) => return vec![format!("{e:#}")],
                };
                match &f.report.scratch {
                    Some(s) if s.dangling.is_empty() => {}
                    Some(s) => bad.push(format!("{} nulled: {:?}", s.dangling.len(), s.dangling)),
                    None => bad.push("not from scratch".into()),
                }
                if f.bytes.len() as u64 > mp_file_max {
                    bad.push(format!("file {:#x} over {mp_file_max:#x}", f.bytes.len()));
                }
                println!("{}: file {:#x}", job.key(), f.bytes.len());
                f.bytes
            }
        };
        // Read back: a valid checksum and one index slot per tag (a map
        // from scratch keeps no slot of the base's index).
        let mut m = map::Map::from_reader(Cursor::new(bytes)).unwrap();
        if let Err(e) = m.validate_checksum() {
            bad.push(format!("checksum: {e}"));
        }
        let holes = (m.index_slots() as usize).saturating_sub(m.tags().len());
        if holes != 0 {
            bad.push(format!("{holes} empty index slots"));
        }
        bad
    };
    let next = AtomicUsize::new(0);
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..workers().clamp(1, todo.len().max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = todo.get(i) else { break };
                    let bad = catch_unwind(AssertUnwindSafe(|| check(job)))
                        .unwrap_or_else(|p| vec![format!("panicked: {}", panic_text(&p))]);
                    conv.release(job);
                    let mut f = failed.lock().unwrap();
                    f.extend(bad.into_iter().map(|b| format!("{}: {b}", job.key())));
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    assert!(failed.is_empty(), "{failed:#?}");
}

/// What the import shares between jobs (each Halo 1 map read and inflated
/// once for the jobs that convert it, each Halo 2 base read once, the
/// scripts' tables and donors built once) changes no byte: a level's two
/// variants and a multiplayer map's two sandboxes, converted one after
/// the other by one converter that shares, are the same as each converted
/// by a converter of its own.
///
/// `H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test --release -p cli
/// --bin h2 h1_import::tests::shared -- --ignored` (H1_LEVEL=a30 and
/// H1_MAP=bloodgulch by default).
#[test]
#[ignore = "reads the discs named by H1_ISO and H2_DISC; converts a level and a map twice"]
fn shared_reads_change_no_byte() {
    let h1 = PathBuf::from(std::env::var("H1_ISO").expect("H1_ISO"));
    let h2 = PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
    let level = std::env::var("H1_LEVEL").unwrap_or_else(|_| "a30".into());
    let mp = std::env::var("H1_MAP").unwrap_or_else(|_| "bloodgulch".into());
    let converter = || {
        let mut iso = Xiso::open(BufReader::new(File::open(&h1).unwrap())).unwrap();
        let disc = Disc::open(&mut iso).unwrap();
        DiscConverter::new(h1.clone(), iso, disc, h2.clone(), PathBuf::new())
    };
    let level: &str = &level;
    let mp: &str = &mp;
    let todo = [
        Job::Campaign(level),
        Job::CampaignHalo2(level),
        Job::Multiplayer(mp, Sandbox::Halo2),
        Job::Multiplayer(mp, Sandbox::Halo1),
    ];
    let bytes = |conv: &DiscConverter, job: &Job| -> Vec<u8> {
        let out = match *job {
            Job::Campaign(l) => conv.campaign(l, h1campaign::h2assets::Assets::Halo1),
            Job::CampaignHalo2(l) => conv.campaign(l, h1campaign::h2assets::Assets::Halo2),
            Job::Multiplayer(m, s) => {
                let (_, f) = conv.multiplayer(m, s).unwrap();
                conv.release(job);
                return f.bytes;
            }
        };
        conv.release(job);
        out.unwrap().bytes
    };
    let shared = converter();
    shared.expect(&todo);
    let with: Vec<Vec<u8>> = todo.iter().map(|j| bytes(&shared, j)).collect();
    // Everything was shared, and let go after its last job.
    assert!(shared.h1_maps.lock().unwrap().is_empty());
    assert_eq!(shared.bases.lock().unwrap().len(), 2);
    for (job, with) in todo.iter().zip(&with) {
        let alone = bytes(&converter(), job);
        assert!(
            alone == *with,
            "{}: {} bytes alone, {} shared",
            job.key(),
            alone.len(),
            with.len()
        );
        eprintln!("{}: {} bytes, the same", job.key(), with.len());
    }
}
