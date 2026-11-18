//! The mod repository's state, screens and Live menu label over a fake
//! source: no guest, no server. Every entry is invented.

use super::*;

#[derive(Default)]
struct Fake {
    lists: Mutex<Vec<ListDone>>,
    installs: Mutex<Vec<(MapId, Progress, InstallDone)>>,
    here: Mutex<BTreeSet<MapId>>,
    refuse: Mutex<Option<ModsError>>,
}

impl Source for Fake {
    fn list(&self, _limit: u32, done: ListDone) -> Result<(), ModsError> {
        if let Some(e) = self.refuse.lock().unwrap().clone() {
            return Err(e);
        }
        self.lists.lock().unwrap().push(done);
        Ok(())
    }
    fn install(&self, id: MapId, progress: Progress, done: InstallDone) -> Result<(), ModsError> {
        self.installs.lock().unwrap().push((id, progress, done));
        Ok(())
    }
    fn installed(&self, ids: &[MapId]) -> BTreeSet<MapId> {
        let here = self.here.lock().unwrap();
        ids.iter().filter(|i| here.contains(i)).copied().collect()
    }
}

fn id(n: u8) -> MapId {
    MapId([n; 32])
}

fn entry(n: u8, name: &str, position: Option<u64>) -> ModEntry {
    ModEntry {
        id: id(n),
        name: name.into(),
        description: None,
        author: "Modder".into(),
        base: "lockout".into(),
        image: false,
        size: 3 * 1024 * 1024 + 400 * 1024,
        position,
        downloads: None,
        published_at: "2026-11-18T10:00:00Z".into(),
    }
}

fn three() -> Vec<ModEntry> {
    vec![
        entry(3, "Lockout Swords", Some(30)),
        entry(2, "Lockout Snipers", Some(20)),
        entry(1, "Lockout Rockets", Some(10)),
    ]
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "h2-boot-mods-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A repository over a fake, the list read once and answered with `items`.
fn listed(items: Vec<ModEntry>, seen_file: Option<PathBuf>) -> (Arc<Fake>, Arc<Repo>) {
    let fake = Arc::new(Fake::default());
    let repo = Repo::new(fake.clone(), seen_file);
    repo.fetch(Instant::now());
    let done = fake
        .lists
        .lock()
        .unwrap()
        .pop()
        .expect("the list was asked for");
    done(Ok(ModPage {
        items,
        cursor: None,
        position: 99,
    }));
    (fake, repo)
}

#[test]
fn the_live_menu_row_counts_what_is_new_until_the_screen_opens() {
    let dir = temp("seen");
    let file = dir.join(SEEN_FILE);
    let (_, repo) = listed(three(), Some(file.clone()));
    assert_eq!(repo.menu_text(), "Mods (3 new)", "never opened: all new");
    assert_eq!(repo.texts.get(MENU_SLOT).as_deref(), Some("Mods (3 new)"));
    repo.opened();
    assert_eq!(repo.menu_text(), "Mods");
    assert_eq!(repo.texts.get(MENU_SLOT).as_deref(), Some("Mods"));
    assert_eq!(read_seen(&file), 30, "kept on disk");
    // This visit still shows them as new.
    assert_eq!(repo.list_row(0).unwrap().1, "New");
    // The next start reads the kept position: one newer map is one new.
    let mut four = three();
    four.insert(0, entry(4, "Lockout Lasers", Some(41)));
    let (_, again) = listed(four, Some(file.clone()));
    assert_eq!(again.menu_text(), "Mods (1 new)");
    again.opened();
    assert_eq!(
        again.list_row(0).unwrap(),
        ("Lockout Lasers".into(), "New".into())
    );
    assert_eq!(
        again.list_row(1).unwrap().1,
        "3.4 MB",
        "seen before: its size"
    );
    assert_eq!(read_seen(&file), 41);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn menu_text_reads_short() {
    assert_eq!(menu_text(0), "Mods");
    assert_eq!(menu_text(3), "Mods (3 new)");
    assert_eq!(menu_text(150), "Mods (99+ new)");
    assert!(menu_text(99).len() <= crate::custom_games::TEXT.len());
}

#[test]
fn the_list_says_why_it_is_empty() {
    let fake = Arc::new(Fake::default());
    let repo = Repo::new(fake.clone(), None);
    assert_eq!(repo.list_row(0).unwrap().0, "Loading...");
    assert_eq!(repo.list_row(1), None);
    assert_eq!(repo.list_count(), 1);
    *fake.refuse.lock().unwrap() = Some(ModsError::NotSignedIn);
    repo.fetch(Instant::now());
    assert_eq!(
        repo.list_row(0).unwrap(),
        ("Sign in to Live first".into(), "Retry".into())
    );
    assert_eq!(repo.menu_text(), "Mods");
    // A press asks again.
    assert!(matches!(repo.press_list(0), Chosen::Stepped(_)));
    assert!(repo.wanted.load(Ordering::SeqCst));
    for (e, says) in [
        (ModsError::Offline, "Live is off"),
        (ModsError::Busy, "Live is busy"),
        (
            ModsError::Server("HTTP 500".into()),
            "Can't reach the server",
        ),
    ] {
        assert_eq!(failed_text(&e), says);
    }
    let (_, empty) = listed(Vec::new(), None);
    assert_eq!(empty.list_row(0).unwrap().0, "No mods yet");
    assert!(matches!(empty.press_list(0), Chosen::Stepped(_)));
}

#[test]
fn a_failed_refresh_keeps_the_list_shown() {
    let (fake, repo) = listed(three(), None);
    repo.fetch(Instant::now());
    let done = fake.lists.lock().unwrap().pop().unwrap();
    done(Err(ModsError::Server("HTTP 503".into())));
    assert_eq!(repo.list_row(0).unwrap().0, "Lockout Swords");
    assert_eq!(repo.list_count(), 3);
}

#[test]
fn the_reader_asks_on_demand_and_on_a_schedule_never_twice_at_once() {
    let fake = Arc::new(Fake::default());
    let repo = Repo::new(fake.clone(), None);
    let t0 = Instant::now();
    assert!(repo.due(t0), "never read");
    repo.fetch(t0);
    assert!(!repo.due(t0 + REFRESH_EVERY * 2), "one read at a time");
    let done = fake.lists.lock().unwrap().pop().unwrap();
    done(Ok(ModPage::default()));
    assert!(!repo.due(t0 + Duration::from_secs(1)));
    assert!(repo.due(t0 + REFRESH_EVERY));
    // The Live menu drawn: asked for, but not more often than MIN_GAP.
    repo.want_list();
    assert!(!repo.due(t0 + Duration::from_secs(5)));
    assert!(repo.due(t0 + MIN_GAP));
    // A failed read is tried again sooner than the schedule.
    *fake.refuse.lock().unwrap() = Some(ModsError::Busy);
    repo.fetch(t0);
    assert!(!repo.due(t0 + Duration::from_secs(5)));
    assert!(repo.due(t0 + RETRY_AFTER));
}

#[test]
fn a_mod_downloads_from_its_page_with_progress_then_shows_installed() {
    let (fake, repo) = listed(three(), None);
    // Row 1 is Lockout Snipers: its page.
    assert_eq!(repo.press_list(1), Chosen::Open(DETAIL));
    assert_eq!(
        repo.texts.get(DETAIL_TITLE).as_deref(),
        Some("Lockout Snipers")
    );
    let rows: Vec<(String, String)> = (0..DETAIL_COUNT as i16)
        .map(|r| repo.detail_row(r).unwrap())
        .collect();
    let want: Vec<(String, String)> = [
        ("Download", "Press A"),
        ("Author", "Modder"),
        ("Map", "Lockout"),
        ("Size", "3.4 MB"),
        ("Published", "2026-11-18"),
    ]
    .into_iter()
    .map(|(l, v)| (l.to_string(), v.to_string()))
    .collect();
    assert_eq!(rows, want);
    assert_eq!(repo.detail_row(DETAIL_COUNT as i16), None);
    assert_eq!(repo.press_detail(ROW_AUTHOR), Chosen::Nothing);
    assert!(matches!(
        repo.press_detail(ROW_DOWNLOAD),
        Chosen::Stepped(_)
    ));
    let (got, mut progress, done) = fake.installs.lock().unwrap().pop().unwrap();
    assert_eq!(got, id(2));
    progress(50, 200);
    assert_eq!(repo.detail_row(ROW_DOWNLOAD).unwrap().1, "Downloading 25%");
    assert_eq!(repo.list_row(1).unwrap().1, "Downloading 25%");
    // Pressed again while it runs: no second download.
    assert!(matches!(
        repo.press_detail(ROW_DOWNLOAD),
        Chosen::Stepped(_)
    ));
    assert!(fake.installs.lock().unwrap().is_empty());
    done(Ok(PathBuf::from("/data/maps/live-0202020202020202")));
    assert_eq!(repo.detail_row(ROW_DOWNLOAD).unwrap().1, "Installed");
    assert_eq!(repo.list_row(1).unwrap().1, "Installed");
}

#[test]
fn a_mod_needing_a_disc_map_says_which() {
    let (fake, repo) = listed(three(), None);
    repo.press_list(0);
    repo.press_detail(ROW_DOWNLOAD);
    let (_, _, done) = fake.installs.lock().unwrap().pop().unwrap();
    done(Err(ModsError::Refused(Refusal::NoBase {
        name: "Lockout Swords".into(),
        map: "lockout".into(),
    })));
    assert_eq!(
        repo.detail_row(ROW_DOWNLOAD).unwrap().1,
        "Needs Lockout from your disc"
    );
    // A failed one can be tried again.
    assert!(matches!(
        repo.press_detail(ROW_DOWNLOAD),
        Chosen::Stepped(_)
    ));
    assert_eq!(fake.installs.lock().unwrap().len(), 1);
    for (e, says) in [
        (
            ModsError::Refused(Refusal::TakenDown("rules".into())),
            "Taken down",
        ),
        (
            ModsError::Refused(Refusal::Unpublished(id(1))),
            "Not on the server",
        ),
        (
            ModsError::Refused(Refusal::Download("HTTP 500".into())),
            "Failed; press A to retry",
        ),
        (ModsError::NotSignedIn, "Sign in to Live first"),
        // A Map Pack base: the map pack, by the map's game name.
        (
            ModsError::Refused(Refusal::NoMapPack {
                name: "Relic Rockets".into(),
                map: "dune".into(),
            }),
            "Needs the Map Pack's Relic",
        ),
        (
            ModsError::Refused(Refusal::NoBase {
                name: "Turf Swords".into(),
                map: "turf".into(),
            }),
            "Needs the Map Pack's Turf",
        ),
    ] {
        assert_eq!(failed_install_text(&e), says);
    }
}

#[test]
fn installed_maps_say_so_and_are_not_fetched_again() {
    let fake = Arc::new(Fake::default());
    fake.here.lock().unwrap().insert(id(1));
    let repo = Repo::new(fake.clone(), None);
    repo.fetch(Instant::now());
    let done = fake.lists.lock().unwrap().pop().unwrap();
    done(Ok(ModPage {
        items: three(),
        ..ModPage::default()
    }));
    repo.opened();
    assert_eq!(repo.list_row(2).unwrap().1, "Installed");
    repo.press_list(2);
    assert_eq!(repo.detail_row(ROW_DOWNLOAD).unwrap().1, "Installed");
    assert!(matches!(
        repo.press_detail(ROW_DOWNLOAD),
        Chosen::Stepped(_)
    ));
    assert!(fake.installs.lock().unwrap().is_empty());
}

#[test]
fn every_text_fits_and_has_an_id_of_the_family() {
    let mut items: Vec<ModEntry> = (0..50u8)
        .map(|n| {
            entry(
                n,
                "An Extremely Long Mod Name That Goes On",
                Some(u64::from(n)),
            )
        })
        .collect();
    items[0].downloads = Some(1234);
    let (_, repo) = listed(items, None);
    let rows = ListRows(Arc::clone(&repo));
    assert_eq!(rows.count(), MAX_ROWS, "at most MAX_ROWS listed");
    for r in 0..rows.count() as i16 {
        let texts = rows.texts(r);
        assert_eq!(texts.len(), 2, "row {r}");
        assert!(texts.iter().all(|(_, id)| TEXTS.owns(*id)));
    }
    assert!(rows.texts(MAX_ROWS as i16).is_empty());
    assert!(rows.texts(-1).is_empty());
    let label = repo.list_row(0).unwrap().0;
    assert_eq!(label.chars().count(), NAME_CHARS);
    assert!(label.ends_with("..."));
    repo.press_list(0);
    assert_eq!(
        repo.detail_row(ROW_PUBLISHED).unwrap(),
        ("Downloads".into(), "1234".into())
    );
    let detail = DetailRows(Arc::clone(&repo));
    assert!(TEXTS.owns(detail.title()) && TEXTS.owns(rows.title()));
    for r in 0..DETAIL_COUNT as i16 {
        assert!(detail.texts(r).iter().all(|(_, id)| TEXTS.owns(*id)));
    }
    assert!(
        rows.describe(3)
            .unwrap()
            .starts_with("title \"Mods\"; rows: [0]")
    );
    // The slots stay inside the family.
    assert!(LIST_ROWS + 2 * MAX_ROWS as u32 <= DETAIL_ROWS);
    assert!(DETAIL_ROWS + 2 * DETAIL_COUNT as u32 <= TEXTS.len);
}

#[test]
fn slots_are_served_once_placed() {
    let s = Slots::default();
    let id = s.set(LIST_TITLE, "Mods");
    assert_eq!(s.address(id), None, "no memory yet");
    s.base.store(0x0080_0000, Ordering::SeqCst);
    assert_eq!(s.address(id), Some(0x0080_0000 + LIST_TITLE * SLOT_LEN));
    assert_eq!(s.address(TEXTS.base + TEXTS.len), None, "not ours");
    let long = "x".repeat(300);
    s.set(DETAIL_TITLE, &long);
    assert_eq!(s.get(DETAIL_TITLE).unwrap().len(), SLOT_LEN as usize - 1);
}

#[test]
fn the_footers_say_what_a_does() {
    assert_eq!(prompts(SCREEN)[0].text, "Select");
    assert_eq!(prompts(DETAIL)[0].text, "Download");
}

#[test]
fn the_seen_file_is_lenient() {
    let dir = temp("lenient");
    let f = dir.join(SEEN_FILE);
    assert_eq!(read_seen(&f), 0, "none");
    std::fs::write(&f, "not toml [").unwrap();
    assert_eq!(read_seen(&f), 0);
    std::fs::write(&f, "seen = -4\n").unwrap();
    assert_eq!(read_seen(&f), 0);
    write_seen(&f, 7).unwrap();
    assert_eq!(read_seen(&f), 7);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_snapshot_waits_for_downloads_and_carries_the_open_page() {
    let (fake, repo) = listed(three(), None);
    assert_eq!(repo.snapshot_live(), None);
    assert_eq!(repo.press_list(1), Chosen::Open(DETAIL));
    let bytes = repo.save_state();
    assert!(matches!(
        repo.press_detail(ROW_DOWNLOAD),
        Chosen::Stepped(_)
    ));
    assert!(repo.snapshot_live().is_some(), "a download is under way");
    let (_, _, done) = fake.installs.lock().unwrap().pop().unwrap();
    done(Ok(PathBuf::from("/data/maps/live-0202020202020202")));
    assert_eq!(repo.snapshot_live(), None);

    // Another run on another page loads it: the saved page is back.
    let (_, other) = listed(three(), None);
    assert_eq!(other.press_list(0), Chosen::Open(DETAIL));
    other.load_state(&bytes).unwrap();
    assert_eq!(other.selected().map(|e| e.id), Some(id(2)));
    assert_eq!(
        other.texts.get(DETAIL_TITLE).as_deref(),
        Some("Lockout Snipers")
    );
    assert_eq!(other.save_state(), bytes, "the same state again");
    assert!(other.load_state(&bytes[1..]).is_err());
    assert_eq!(other.save_state(), bytes, "damaged bytes change nothing");
}
