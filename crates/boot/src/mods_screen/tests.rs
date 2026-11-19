//! The mod repository's state, screens and Live menu label over a fake
//! source: no guest, no server. Every entry is invented.

use super::*;

#[derive(Default)]
struct Fake {
    lists: Mutex<Vec<ListDone>>,
    installs: Mutex<Vec<(MapId, Progress, InstallDone)>>,
    here: Mutex<BTreeSet<MapId>>,
    refuse: Mutex<Option<ModsError>>,
    removed: Mutex<Vec<MapId>>,
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
    fn remove(&self, id: MapId) -> Result<(), String> {
        if !self.here.lock().unwrap().remove(&id) {
            return Err("not downloaded here".into());
        }
        self.removed.lock().unwrap().push(id);
        Ok(())
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
        credit: None,
        base: "lockout".into(),
        image: false,
        size: 3 * 1024 * 1024 + 400 * 1024,
        position,
        downloads: None,
        published_at: "2026-11-18T10:00:00Z".into(),
        features: Vec::new(),
        gametypes: Vec::new(),
        version: None,
        images: Vec::new(),
        contents: Vec::new(),
        pack: None,
        pack_info: None,
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
    let lines: Vec<String> = (0..7).map(|r| repo.detail_line(r).unwrap()).collect();
    assert_eq!(
        lines,
        [
            "Download",
            "Map: Lockout",
            "By: Modder",
            "Size: 3.4 MB",
            "Published: 2026-11-18",
            "For: any game type",
            "Changes: not listed",
        ]
    );
    assert_eq!(repo.detail_line(7).as_deref(), Some("[ ] Map Pack map"));
    assert_eq!(repo.detail_line(8).as_deref(), Some("[x] Joins get it"));
    assert_eq!(repo.detail_line(9), None);
    assert_eq!(repo.press_detail(2), Chosen::Nothing, "an info row");
    assert!(matches!(repo.press_detail(0), Chosen::Stepped(_)));
    let (got, mut progress, done) = fake.installs.lock().unwrap().pop().unwrap();
    assert_eq!(got, id(2));
    progress(50, 200);
    assert_eq!(repo.detail_line(0).unwrap(), "Downloading 25%");
    assert_eq!(repo.list_row(1).unwrap().1, "Downloading 25%");
    assert!(
        repo.detail_description(0)
            .unwrap()
            .starts_with("Downloading 25% of 3.4 MB")
    );
    // Pressed again while it runs (A, or X anywhere): no second download.
    assert!(matches!(repo.press_detail(0), Chosen::Stepped(_)));
    assert!(repo.act().contains("already"));
    assert!(fake.installs.lock().unwrap().is_empty());
    done(Ok(PathBuf::from("/data/maps/live-0202020202020202")));
    assert_eq!(repo.detail_line(0).unwrap(), "Remove");
    assert_eq!(repo.list_row(1).unwrap().1, "Installed");
    assert!(
        repo.texts.get(DETAIL_FOOTER).unwrap().contains("REMOVE"),
        "the footer says what X does now"
    );
}

#[test]
fn x_on_the_list_downloads_the_focused_map_without_its_page() {
    let (fake, repo) = listed(three(), None);
    assert!(
        repo.x_list(Some(2))
            .starts_with("mods: downloading Lockout Rockets")
    );
    let (got, _, done) = fake.installs.lock().unwrap().pop().unwrap();
    assert_eq!(got, id(1));
    assert_eq!(repo.selected(), None, "no page opened");
    assert_eq!(repo.list_line(2).unwrap(), "Lockout Rockets  0%");
    done(Ok(PathBuf::from("/data/maps/live-0101010101010101")));
    assert_eq!(repo.list_line(2).unwrap(), "Lockout Rockets  Installed");
    // Installed: X on the list leaves it alone; nothing focused: nothing.
    assert!(repo.x_list(Some(2)).contains("installed"));
    assert!(repo.x_list(None).contains("no map focused"));
    assert!(repo.x_list(Some(9)).contains("no map focused"));
    assert!(fake.installs.lock().unwrap().is_empty());
}

#[test]
fn an_installed_map_is_removed_from_its_page_on_a_second_press() {
    let fake = Arc::new(Fake::default());
    fake.here.lock().unwrap().insert(id(2));
    let repo = Repo::new(fake.clone(), None);
    repo.fetch(Instant::now());
    let done = fake.lists.lock().unwrap().pop().unwrap();
    done(Ok(ModPage {
        items: three(),
        ..ModPage::default()
    }));
    repo.press_list(1);
    assert_eq!(repo.detail_line(0).unwrap(), "Remove");
    assert!(repo.texts.get(DETAIL_FOOTER).unwrap().contains("REMOVE"));
    assert!(repo.act().contains("press again"));
    assert_eq!(repo.detail_line(0).unwrap(), "Remove? Press again");
    assert!(
        fake.removed.lock().unwrap().is_empty(),
        "one press removes nothing"
    );
    assert!(matches!(repo.press_detail(0), Chosen::Stepped(_)));
    assert_eq!(*fake.removed.lock().unwrap(), [id(2)]);
    assert_eq!(repo.detail_line(0).unwrap(), "Download");
    assert_eq!(repo.list_row(1).unwrap().1, "3.4 MB");
    assert!(repo.texts.get(DETAIL_FOOTER).unwrap().contains("DOWNLOAD"));
    // Leaving the page forgets a first press.
    repo.act();
    let (_, _, done) = fake.installs.lock().unwrap().pop().unwrap();
    done(Ok(PathBuf::from("/data/maps/live-0202020202020202")));
    fake.here.lock().unwrap().insert(id(2));
    repo.act();
    assert_eq!(repo.press_list(1), Chosen::Open(DETAIL));
    assert_eq!(repo.detail_line(0).unwrap(), "Remove");
    // A removal that fails says so.
    fake.here.lock().unwrap().clear();
    repo.act();
    assert!(repo.act().contains("not removed"));
    assert_eq!(repo.detail_line(0).unwrap(), "Remove failed");
    assert!(
        repo.detail_description(0)
            .unwrap()
            .contains("not downloaded here")
    );
}

/// An invented mod with everything its page can show.
fn full() -> ModEntry {
    let mut e = entry(5, "Relic Hogs", Some(50));
    e.base = "dune".into();
    e.description = Some("Warthogs everywhere. ".repeat(20));
    e.downloads = Some(12);
    e.version = Some("1.2".into());
    e.gametypes = vec!["ctf".into(), "slayer".into()];
    e.features = vec!["vehicles".into(), "scripts".into()];
    e
}

#[test]
fn a_maps_page_lists_its_info_and_ticks_what_it_changes() {
    let (_, repo) = listed(vec![full()], None);
    repo.press_list(0);
    let lines: Vec<String> = (0..repo.page().len() as i16)
        .map(|r| repo.detail_line(r).unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            "Download",
            "Map: Relic",
            "By: Modder",
            "Size: 3.4 MB",
            "Downloads: 12",
            "Published: 2026-11-18",
            "Version: 1.2",
            "For: CTF, Slayer",
            "[x] Vehicles",
            "[ ] Weapons",
            "[ ] Objects",
            "[ ] Textures",
            "[ ] Shaders",
            "[x] Scripted events",
            "[ ] Geometry",
            "[ ] AI",
            "[x] Map Pack map",
            "[x] Joins get it",
        ]
    );
    assert!(lines.iter().all(|l| l.chars().count() <= LINE_CHARS));
    let rows = page_rows(&full());
    let at = |r: PageRow| rows.iter().position(|x| *x == r).unwrap() as i16;
    let about = repo.detail_description(at(PageRow::Map)).unwrap();
    assert!(about.starts_with("Warthogs everywhere."), "{about}");
    assert!(about.ends_with("\nBy Modder."), "{about}");
    assert_eq!(
        repo.detail_description(at(PageRow::Feature(0))).unwrap(),
        "Vehicles: vehicles added, taken out or moved."
    );
    assert!(
        repo.detail_description(at(PageRow::Feature(1)))
            .unwrap()
            .starts_with("Weapons: not in this mod")
    );
    assert!(
        repo.detail_description(at(PageRow::MapPack))
            .unwrap()
            .contains("h2 import-mappack")
    );
    assert_eq!(
        repo.detail_description(at(PageRow::GameTypes)).unwrap(),
        "Made for CTF, Slayer."
    );
    assert!(
        repo.detail_description(0)
            .unwrap()
            .starts_with("A or X downloads it (3.4 MB). Then host a game on Relic")
    );
    for r in 0..rows.len() as i16 {
        let d = repo.detail_description(r).unwrap();
        assert!(d.len() < SLOT_LEN as usize, "row {r}: {} bytes", d.len());
    }
}

#[test]
fn the_list_describes_the_focused_map() {
    let mut items = three();
    items.insert(0, full());
    let (_, repo) = listed(items, None);
    assert_eq!(
        repo.list_description(0).unwrap(),
        "On Relic, by Modder\n3.4 MB, 12 downloads\nHas vehicles, scripted events\n\
         X downloads it, A opens its page"
    );
    assert_eq!(
        repo.list_description(1).unwrap().lines().nth(2),
        Some("What it changes isn't listed")
    );
    assert_eq!(repo.list_description(9), None);
    // The picture is the base map's slot's, once read.
    let rows = ListRows::new(Arc::clone(&repo));
    assert_eq!(rows.picture(0), None, "not read yet");
    repo.set_pictures(BTreeMap::from([
        ("dune".to_string(), 0xe100_0010),
        ("lockout".to_string(), 0xe100_0020),
    ]));
    assert_eq!(rows.picture(0), Some(0xe100_0010));
    assert_eq!(rows.picture(1), Some(0xe100_0020));
    assert_eq!(rows.picture(9), None);
    repo.press_list(0);
    let page = DetailRows::new(Arc::clone(&repo));
    assert_eq!(page.picture(5), Some(0xe100_0010), "every row: the map's");
    // Opening the screen again reads them again.
    repo.opened();
    assert_eq!(rows.picture(0), None);
}

#[test]
fn descriptions_are_set_again_when_something_changes() {
    let (fake, repo) = listed(three(), None);
    let rows = ListRows::new(Arc::clone(&repo));
    assert!(rows.live_text().redraw, "first draw");
    assert!(!rows.live_text().redraw, "nothing changed");
    assert!(repo.x_list(Some(0)).contains("downloading"));
    assert!(rows.live_text().redraw, "a download started");
    let (_, mut progress, _) = fake.installs.lock().unwrap().pop().unwrap();
    assert!(!rows.live_text().redraw);
    progress(1, 2);
    assert!(rows.live_text().redraw, "its progress");
    assert_eq!(
        repo.texts.get(LIST_DESCRIPTIONS).unwrap().lines().last(),
        Some("Downloading 50%")
    );
}

#[test]
fn a_mod_needing_a_disc_map_says_which() {
    let (fake, repo) = listed(three(), None);
    repo.press_list(0);
    repo.press_detail(0);
    let (_, _, done) = fake.installs.lock().unwrap().pop().unwrap();
    done(Err(ModsError::Refused(Refusal::NoBase {
        name: "Lockout Swords".into(),
        map: "lockout".into(),
    })));
    assert_eq!(repo.detail_line(0).unwrap(), "Needs Lockout from your disc");
    // A failed one can be tried again.
    assert!(matches!(repo.press_detail(0), Chosen::Stepped(_)));
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
    assert_eq!(repo.detail_line(0).unwrap(), "Remove");
    assert!(matches!(repo.press_detail(0), Chosen::Stepped(_)));
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
    let rows = ListRows::new(Arc::clone(&repo));
    assert_eq!(rows.count(), MAX_ROWS, "at most MAX_ROWS listed");
    for r in 0..rows.count() as i16 {
        let texts = rows.texts(r);
        assert_eq!(
            texts.len(),
            1,
            "row {r}: one line on the level select's skin"
        );
        assert!(texts.iter().all(|(_, id)| TEXTS.owns(*id)));
        assert!(TEXTS.owns(rows.description(r).unwrap()));
        let line = repo.list_line(r).unwrap();
        assert!(line.chars().count() <= LINE_CHARS, "{line:?}");
        let d = repo.list_description(r).unwrap();
        assert!(
            d.len() < SLOT_LEN as usize && d.lines().count() <= 5,
            "{d:?}"
        );
    }
    assert!(rows.texts(MAX_ROWS as i16).is_empty());
    assert!(rows.texts(-1).is_empty());
    assert_eq!(rows.description(MAX_ROWS as i16), None);
    // Row 0 was published at position 0: seen. Row 1 is new.
    assert_eq!(repo.list_line(0).unwrap(), "An Extremely Long Mod Nam...");
    assert_eq!(repo.list_line(1).unwrap(), "An Extremely Long Mo...  New");
    repo.press_list(0);
    assert_eq!(repo.detail_line(4).as_deref(), Some("Downloads: 1234"));
    let detail = DetailRows::new(Arc::clone(&repo));
    assert!(TEXTS.owns(detail.title()) && TEXTS.owns(rows.title()));
    assert_eq!(detail.count(), repo.page().len());
    for r in 0..detail.count() as i16 {
        assert!(detail.texts(r).iter().all(|(_, id)| TEXTS.owns(*id)));
        assert!(TEXTS.owns(detail.description(r).unwrap()));
    }
    assert!(
        rows.describe(3)
            .unwrap()
            .starts_with("title \"Mods\"; rows: [0]")
    );
    // The most rows a page can have fit its slots.
    let mut most = entry(1, "M", None);
    most.features = vec!["ai".into()];
    most.version = Some("1".into());
    most.downloads = Some(1);
    assert!(page_rows(&most).len() <= MAX_PAGE_ROWS);
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
fn the_footers_say_what_a_and_x_do() {
    let texts = |p: Vec<Prompt>| p.into_iter().map(|p| p.text).collect::<Vec<_>>();
    assert_eq!(
        texts(prompts(SCREEN, false)),
        ["Details", "Download", "Back"]
    );
    assert_eq!(
        texts(prompts(DETAIL, false)),
        ["Select", "Download", "Back"]
    );
    assert_eq!(texts(prompts(DETAIL, true)), ["Select", "Remove", "Back"]);
    // X is the button the class's action takes (event button word 2).
    assert_eq!(prompts(SCREEN, false)[1].glyphs, [Glyph::X]);
    assert_eq!(X_ONLY, [crate::list_class::X_BUTTON]);
    assert_eq!(crate::list_class::X_BUTTON, 2);
    let classes = classes(&Repo::new(Arc::new(Fake::default()), None));
    for c in &classes {
        let b = c.behaviour();
        assert!(b.action && b.action_buttons == [crate::list_class::X_BUTTON]);
    }
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
    assert!(matches!(repo.press_detail(0), Chosen::Stepped(_)));
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

#[test]
fn by_is_the_credited_maker_and_the_uploader_keeps_a_row() {
    let mut credited = entry(2, "PPF Noobs Lockout", Some(20));
    credited.author = "afr".into();
    credited.credit = Some("l PPF l NeoN, The PPF Noobs & Friends".into());
    let uncredited = entry(1, "Canyon", Some(10));
    assert_eq!(made_by(&credited), "l PPF l NeoN, The PPF Noobs & Friends");
    assert_eq!(made_by(&uncredited), "Modder", "no credit: the uploader");
    assert!(uploader_apart(&credited) && !uploader_apart(&uncredited));
    let mut same = entry(3, "Mine", None);
    same.credit = Some("modder".into());
    assert!(!uploader_apart(&same), "the uploader made it: one row");
    assert_eq!(page_rows(&credited)[2..4], [PageRow::By, PageRow::Uploader]);
    assert_eq!(page_rows(&uncredited)[2..4], [PageRow::By, PageRow::Size]);

    let (_, repo) = listed(vec![credited, uncredited], None);
    repo.press_list(0);
    let by = repo.detail_line(2).unwrap();
    assert!(
        by.starts_with("By: l PPF l NeoN") && by.ends_with("..."),
        "{by}"
    );
    assert_eq!(by.chars().count(), LINE_CHARS);
    assert_eq!(repo.detail_line(3).as_deref(), Some("Uploaded By: afr"));
    assert!(
        repo.detail_description(3)
            .unwrap()
            .starts_with("afr shared it")
    );
    repo.press_list(1);
    assert_eq!(repo.detail_line(2).as_deref(), Some("By: Modder"));
    assert_eq!(repo.detail_line(3).as_deref(), Some("Size: 3.4 MB"));
}
