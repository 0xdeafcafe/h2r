//! The mod repository's state, screens and Live menu label over a fake
//! source: no guest, no server. Every entry is invented.

use super::*;

#[derive(Default)]
struct Fake {
    signed_out: AtomicBool,
    lists: Mutex<Vec<ListDone>>,
    installs: Mutex<Vec<(MapId, Progress, InstallDone)>>,
    here: Mutex<BTreeSet<MapId>>,
    refuse: Mutex<Option<ModsError>>,
    removed: Mutex<Vec<MapId>>,
}

impl Source for Fake {
    fn check_list(&self) -> Result<(), ModsError> {
        if self.signed_out.load(Ordering::SeqCst) {
            Err(ModsError::NotSignedIn)
        } else {
            Ok(())
        }
    }
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
        pack_description: None,
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
    assert_eq!(repo.list_line(0).as_deref(), Some("Loading..."));
    assert_eq!(repo.list_row(1), None);
    assert_eq!(repo.list_count(), 1);
    *fake.refuse.lock().unwrap() = Some(ModsError::NotSignedIn);
    repo.fetch(Instant::now());
    assert_eq!(
        repo.list_row(0).unwrap(),
        ("Sign in to Live first".into(), "Retry".into())
    );
    // The one row is what A does; why is in its description.
    assert_eq!(repo.list_line(0).as_deref(), Some("Try Again"));
    assert_eq!(
        repo.list_description(0).as_deref(),
        Some("Sign in to Live first.\nA reads the list again.")
    );
    assert_eq!(repo.list_description(1), None);
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
    assert_eq!(empty.list_line(0).as_deref(), Some("Refresh"));
    assert!(empty
        .list_description(0)
        .unwrap()
        .starts_with("No mods on your Live server yet."));
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
fn the_reader_waits_for_live_sign_in_then_fetches_without_a_retry_delay() {
    let fake = Arc::new(Fake::default());
    fake.signed_out.store(true, Ordering::SeqCst);
    let repo = Repo::new(fake.clone(), None);
    let t0 = Instant::now();
    let sign_in = t0 + REFRESH_EVERY + Duration::from_secs(1);
    for now in [t0, t0 + RETRY_AFTER, t0 + MIN_GAP, t0 + REFRESH_EVERY] {
        repo.want_list();
        repo.read_if_due(now);
        assert!(fake.lists.lock().unwrap().is_empty());
    }
    assert_eq!(repo.list_state(), List::Failed(ModsError::NotSignedIn));

    fake.signed_out.store(false, Ordering::SeqCst);
    repo.read_if_due(sign_in);
    repo.read_if_due(sign_in + Duration::from_secs(1));
    let mut lists = fake.lists.lock().unwrap();
    assert_eq!(lists.len(), 1);
    let done = lists.pop().unwrap();
    drop(lists);
    done(Ok(ModPage {
        items: three(),
        ..Default::default()
    }));
    assert_eq!(repo.menu_text(), "Mods (3 new)");

    fake.signed_out.store(true, Ordering::SeqCst);
    repo.read_if_due(sign_in + Duration::from_secs(2));
    assert!(fake.lists.lock().unwrap().is_empty());
    assert_eq!(repo.menu_text(), "Mods (3 new)");

    fake.signed_out.store(false, Ordering::SeqCst);
    repo.read_if_due(sign_in + Duration::from_secs(3));
    assert_eq!(fake.lists.lock().unwrap().len(), 1);
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
    // One row, the action: everything else is on the page, not in a list.
    assert_eq!(repo.detail_line(0).as_deref(), Some("Download"));
    assert_eq!(repo.detail_line(1), None);
    assert_eq!(DetailRows::new(Arc::clone(&repo)).count(), 1);
    assert_eq!(repo.press_detail(1), Chosen::Nothing, "no other row");
    assert_eq!(
        repo.detail_description(0).as_deref(),
        Some("A or X downloads it: 3.4 MB.")
    );
    assert!(matches!(repo.press_detail(0), Chosen::Stepped(_)));
    let (got, mut progress, done) = fake.installs.lock().unwrap().pop().unwrap();
    assert_eq!(got, id(2));
    progress(50, 200);
    assert_eq!(repo.detail_line(0).unwrap(), "Downloading 25%");
    assert_eq!(repo.list_row(1).unwrap().1, "Downloading 25%");
    assert_eq!(
        repo.detail_description(0).as_deref(),
        Some("25% of 3.4 MB. You can leave.")
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
    assert_eq!(
        repo.detail_description(0).as_deref(),
        Some("Installed. Change Map > Mods.")
    );
}

#[test]
fn x_on_the_list_downloads_the_focused_map_without_its_page() {
    let (fake, repo) = listed(three(), None);
    assert!(repo
        .x_list(Some(2))
        .starts_with("mods: downloading Lockout Rockets"));
    let (got, _, done) = fake.installs.lock().unwrap().pop().unwrap();
    assert_eq!(got, id(1));
    assert_eq!(repo.selected(), None, "no page opened");
    assert_eq!(repo.list_line(2).unwrap(), "Lockout Rockets  \u{2193} 0%");
    done(Ok(PathBuf::from("/data/maps/live-0101010101010101")));
    assert_eq!(repo.list_line(2).unwrap(), "Lockout Rockets  \u{2713}");
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
    assert_eq!(repo.detail_line(0).unwrap(), "Retry removal");
    assert_eq!(
        repo.detail_description(0).as_deref(),
        Some("Not removed: not downloaded here")
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
    let e = full();
    assert_eq!(
        facts(&e),
        [
            ("Map", "Relic".to_string()),
            ("By", "Modder".to_string()),
            ("Size", "3.4 MB".to_string()),
            ("Downloads", "12".to_string()),
            ("Published", "2026-11-18".to_string()),
            ("Version", "1.2".to_string()),
            ("Modes", "CTF, Slayer".to_string()),
            ("From", "Map Pack".to_string()),
            ("Joiners", "Auto-install".to_string()),
        ]
    );
    assert_eq!(
        checklist(&e),
        [
            "[x] Vehicles",
            "[ ] Weapons",
            "[ ] Objects",
            "[ ] Textures",
            "[ ] Shaders",
            "[x] Scripts",
            "[ ] Geometry",
            "[ ] AI",
        ]
    );
    let (_, repo) = listed(vec![e], None);
    repo.press_list(0);
    // The panel's slots hold the page: body, facts, checklist.
    let t = repo.panel();
    assert_eq!(t.len(), page_layout::TEXTS);
    assert!(t[page_layout::BODY_TEXT].starts_with("Warthogs everywhere."));
    assert!(t[page_layout::BODY_TEXT].ends_with("..."));
    assert_eq!(t[page_layout::BODY_TEXT].lines().count(), 3);
    assert_eq!(t[page_layout::fact_label(0)], "Map");
    assert_eq!(t[page_layout::fact_value(0)], "Relic");
    assert_eq!(t[page_layout::fact_label(8)], "Joiners");
    assert_eq!(t[page_layout::fact_label(9)], "", "no tenth fact");
    assert_eq!(t[page_layout::CHECK_HEADING], "Changes");
    assert_eq!(t[page_layout::check_cell(0, 0)], "[x] Vehicles");
    assert_eq!(t[page_layout::check_cell(0, 1)], "[ ] Weapons");
    assert_eq!(t[page_layout::check_cell(3, 1)], "[ ] AI");
    for (i, text) in t.iter().enumerate() {
        assert_eq!(
            repo.texts.get(PANEL_TEXTS + i as u32).as_deref(),
            Some(text.as_str()),
            "slot of panel text {i}"
        );
    }
    let page = DetailRows::new(Arc::clone(&repo));
    for i in 0..page_layout::TEXTS {
        assert_eq!(page.panel_text(i), Some(Slots::id(PANEL_TEXTS + i as u32)));
    }
    assert_eq!(page.panel_text(page_layout::TEXTS), None);
    // Facts fit their row, label and value; a mod with no checklist and no
    // description says so.
    let mut bare = entry(6, "Bare", None);
    bare.published_at = String::new();
    bare.gametypes = vec!["koth".into(), "juggernaut".into(), "slayer".into()];
    for (label, value) in facts(&bare).iter().chain(facts(&full()).iter()) {
        assert!(
            text_fit::width(value) <= i32::from(page_layout::TABLE_RIGHT - page_layout::VALUE_LEFT),
            "{label} {value}"
        );
    }
    assert_eq!(facts(&bare)[3].0, "Modes");
    assert!(facts(&bare)[3].1.ends_with("..."));
    assert_eq!(checklist(&bare), ["Not listed"]);
    assert_eq!(about(&bare), "No description. Built on Lockout.");
}

/// Every status line the page's one row can have fits its one line.
#[test]
fn the_status_line_fits_in_every_state() {
    let (fake, repo) = listed(vec![full()], None);
    repo.press_list(0);
    let mut seen = vec![repo.detail_description(0).unwrap()];
    repo.act();
    let (_, mut progress, done) = fake.installs.lock().unwrap().pop().unwrap();
    progress(1, 3);
    seen.push(repo.detail_description(0).unwrap());
    done(Err(ModsError::Refused(Refusal::NoMapPack {
        name: "Relic Hogs".into(),
        map: "dune".into(),
    })));
    seen.push(repo.detail_description(0).unwrap());
    assert_eq!(
        seen.last().map(String::as_str),
        Some("Needs the Map Pack's Relic")
    );
    fake.here.lock().unwrap().insert(id(5));
    repo.install_done(id(5), Ok(()));
    seen.push(repo.detail_description(0).unwrap());
    repo.act();
    seen.push(repo.detail_description(0).unwrap());
    for s in &seen {
        assert!(text_fit::width(s) <= 303, "{s:?}");
    }
    assert_eq!(seen[4], "Press A or X again to remove it.");
}

#[test]
fn the_list_describes_the_focused_map() {
    let mut items = three();
    items.insert(0, full());
    let (_, repo) = listed(items, None);
    assert_eq!(
        repo.list_description(0).unwrap(),
        "On Relic, by Modder\n3.4 MB, 12 downloads\nHas vehicles, scripts\n\
         New map"
    );
    assert_eq!(
        repo.list_description(1).unwrap().lines().nth(2),
        Some("Changes not listed")
    );
    assert_eq!(repo.list_description(9), None);
    // The picture is the base map's slot's, once read.
    let rows = ListRows::new(Arc::clone(&repo));
    assert_eq!(rows.picture(0), None, "not read yet");
    assert!(rows.restore_missing_picture());
    assert_eq!(LIST_PICTURE.screen_id, page_layout::LOBBY_SCREEN_ID);
    assert_eq!(LIST_PICTURE.bitmap, page_layout::PICTURE.bitmap);
    assert_eq!(LIST_PICTURE.at, crate::campaign_menu::PICTURE.at);
    repo.set_pictures(BTreeMap::from([
        ("dune".to_string(), 0xe100_0010),
        ("lockout".to_string(), 0xe100_0020),
    ]));
    assert_eq!(rows.picture(0), Some(0xe100_0010));
    assert_eq!(rows.picture(1), Some(0xe100_0020));
    assert_eq!(rows.picture(9), None);
    repo.press_list(0);
    let page = DetailRows::new(Arc::clone(&repo));
    assert_eq!(page.picture(0), Some(0xe100_0010), "the map's");
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
    assert_eq!(repo.detail_line(0).unwrap(), "Retry download");
    assert_eq!(
        repo.detail_description(0).unwrap(),
        "Needs Lockout from your disc"
    );
    assert_eq!(repo.list_row(0).unwrap().1, "Failed");
    assert_eq!(repo.prompts(SCREEN, Some(0))[1].text, "Retry");
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
            "Download failed",
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
        assert!(text_fit::width(&line) <= text_fit::ROW, "{line:?}");
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
    assert!(repo.list_line(0).unwrap().ends_with("..."));
    assert!(repo.list_line(1).unwrap().ends_with("...  \u{2605}"));
    repo.press_list(0);
    assert!(facts(&repo.selected().unwrap()).contains(&("Downloads", "1234".to_string())));
    let detail = DetailRows::new(Arc::clone(&repo));
    assert!(TEXTS.owns(detail.title()) && TEXTS.owns(rows.title()));
    assert_eq!(detail.count(), MAX_PAGE_ROWS);
    for r in 0..detail.count() as i16 {
        assert!(detail.texts(r).iter().all(|(_, id)| TEXTS.owns(*id)));
        assert!(TEXTS.owns(detail.description(r).unwrap()));
    }
    for i in 0..page_layout::TEXTS {
        assert!(TEXTS.owns(detail.panel_text(i).unwrap()));
    }
    assert!(rows
        .describe(3)
        .unwrap()
        .starts_with("title \"Mods\"; rows: [0]"));
    // The most facts a page can have fit its table.
    let mut most = entry(1, "M", None);
    most.features = vec!["ai".into()];
    most.version = Some("1".into());
    most.downloads = Some(1);
    most.credit = Some("Someone Else".into());
    assert_eq!(facts(&most).len(), page_layout::FACT_ROWS);
    assert!(checklist(&most).len() <= 2 * page_layout::CHECK_ROWS);
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
    let (_, repo) = listed(with_pack(), None);
    let commands = |screen, row| texts(repo.prompts(screen, Some(row)));
    assert_eq!(commands(SCREEN, 0), ["Details", "Download", "Back"]);
    assert_eq!(commands(SCREEN, 1), ["Open", "Download All", "Back"]);
    repo.press_list(1);
    assert_eq!(commands(PACK, 0), ["or", "Download All", "Back"]);
    repo.press_pack(2);
    assert_eq!(commands(DETAIL, 0), ["or", "Download", "Back"]);
    repo.act();
    assert_eq!(commands(DETAIL, 0), ["Back"]);
    repo.install_done(id(3), Ok(()));
    assert_eq!(commands(PACK, 2), ["Details", "Back"]);
    assert_eq!(commands(DETAIL, 0), ["or", "Remove", "Back"]);
    repo.act();
    assert_eq!(commands(DETAIL, 0), ["or", "Confirm", "Back"]);
    let rows = DetailRows::new(Arc::clone(&repo));
    assert_eq!(rows.focused_footer(Some(0)), Some(Slots::id(DETAIL_FOOTER)));
    assert!(repo.texts.get(DETAIL_FOOTER).unwrap().contains("CONFIRM"));
    let mut spec = spec::pack_spec(&repo);
    spec.focus = 2;
    assert_eq!(
        spec.footer_text(),
        Some(footer_legend(&repo.prompts(PACK, Some(2))))
    );
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
    let credit = facts(&credited);
    assert_eq!(credit[1].0, "By");
    assert!(credit[1].1.starts_with("l PPF") && credit[1].1.ends_with("..."));
    assert_eq!(credit[2], ("Shared By", "afr".to_string()));
    assert_eq!(
        facts(&uncredited)[1..3],
        [("By", "Modder".to_string()), ("Size", "3.4 MB".to_string())]
    );
    let (_, repo) = listed(vec![credited, uncredited], None);
    repo.press_list(0);
    assert_eq!(repo.panel()[page_layout::fact_value(2)], "afr");
}

/// The list's states are marks drawn into the fonts, not words.
#[test]
fn the_list_marks_new_installed_and_downloading() {
    assert_eq!(mark("New").as_deref(), Some("\u{2605}"));
    assert_eq!(mark("Installed").as_deref(), Some("\u{2713}"));
    assert_eq!(mark("Downloading 45%").as_deref(), Some("\u{2193} 45%"));
    assert_eq!(mark("3.4 MB"), None);
    assert_eq!(mark("120 KB"), None);
    assert_eq!(mark(""), None);
    for unit in font::mod_marks::MARKS {
        let c = char::from_u32(u32::from(unit)).unwrap();
        assert!(
            ["New", "Installed", "Downloading 1%"]
                .iter()
                .any(|s| mark(s).unwrap().starts_with(c)),
            "U+{unit:04X} is used"
        );
    }
}

/// Two maps of "Phantom Mappack" between two maps of their own, and one
/// pack with a single map listed.
fn with_pack() -> Vec<ModEntry> {
    let in_pack = |n: u8, name: &str, at: u64, maker: &str| {
        let mut e = entry(n, name, Some(at));
        e.pack = Some("Phantom Mappack".into());
        e.credit = Some(maker.into());
        e.base = "ascension".into();
        e
    };
    let mut first = in_pack(5, "Phantom Ascension", 50, "Phantom");
    first.pack_description = Some("Classic maps,   remade\nfor small teams.".into());
    let mut lone = entry(1, "Lone Lockout", Some(10));
    lone.pack = Some("Hockey Fans".into());
    vec![
        entry(6, "Lockout Swords", Some(60)),
        first,
        entry(4, "Lockout Snipers", Some(40)),
        in_pack(3, "Phantom Ivory", 30, "Ghost"),
        in_pack(2, "Phantom Colossus", 20, "Phantom"),
        lone,
    ]
}

#[test]
fn a_pack_is_one_row_at_its_newest_maps_place() {
    let items = with_pack();
    let rows = tops(&items);
    let names: Vec<String> = rows
        .iter()
        .map(|t| match t {
            Top::Map(e) => e.name.clone(),
            Top::Pack(name, maps) => format!("{name} ({})", maps.len()),
        })
        .collect();
    assert_eq!(
        names,
        [
            "Lockout Swords",
            "Phantom Mappack (3)",
            "Lockout Snipers",
            "Lone Lockout"
        ],
        "a pack of one is its map's row"
    );
    let (_, repo) = listed(items, None);
    repo.opened();
    assert_eq!(repo.list_count(), 4);
    assert_eq!(repo.list_line(1).unwrap(), "Phantom Mappack  \u{2605}");
    assert_eq!(
        repo.list_description(1).unwrap(),
        "3 maps, by Phantom, Ghost\nClassic maps, remade for\nsmall teams.\n0/3 installed, 10.2 MB\nNew maps"
    );
    // Seen before: no star, how many maps.
    let mut old = with_pack();
    for e in &mut old {
        e.position = Some(1);
    }
    let (_, seen) = listed(old, None);
    seen.opened();
    seen.opened();
    assert!(seen.list_line(1).unwrap().ends_with("  3 maps"));
}

#[test]
fn a_packs_list_has_download_all_then_its_maps_each_opening_its_page() {
    let (fake, repo) = listed(with_pack(), None);
    assert_eq!(repo.press_list(1), Chosen::Open(PACK));
    assert_eq!(
        repo.texts.get(PACK_TITLE).as_deref(),
        Some("Phantom Mappack")
    );
    let rows = PackRows::new(Arc::clone(&repo));
    assert_eq!(rows.count(), 4);
    assert_eq!(repo.pack_line(0).unwrap(), "Download All (3)");
    assert_eq!(repo.pack_line(1).unwrap(), "Phantom Ascension  \u{2605}");
    assert_eq!(repo.pack_line(3).unwrap(), "Phantom Colossus  \u{2605}");
    assert_eq!(repo.pack_line(4), None);
    assert!(repo.pack_description(0).unwrap().ends_with("3 to download"));
    assert!(
        repo.pack_description(2)
            .unwrap()
            .starts_with("On Ascension")
    );
    // A map's row opens its page, Back returns to the pack.
    assert_eq!(repo.press_pack(2), Chosen::Open(DETAIL));
    assert_eq!(repo.selected().map(|e| e.id), Some(id(3)));
    // X on a map downloads it alone.
    assert!(
        repo.x_pack(Some(2))
            .starts_with("mods: downloading Phantom Ivory")
    );
    assert_eq!(repo.pack_line(0).as_deref(), Some("Download All (2)"));
    assert!(repo.pack_description(0).unwrap().contains("Downloading 0%"));
    let (got, _, done) = fake.installs.lock().unwrap().pop().unwrap();
    assert_eq!(got, id(3));
    done(Ok(PathBuf::from("/data/maps/live-03")));
    assert_eq!(repo.pack_line(0).unwrap(), "Download All (2)");
    // Download All starts the two not here, together on the rows.
    assert!(matches!(repo.press_pack(0), Chosen::Stepped(_)));
    let mut started: Vec<_> = fake.installs.lock().unwrap().drain(..).collect();
    let ids: BTreeSet<MapId> = started.iter().map(|(i, ..)| *i).collect();
    assert_eq!(ids, BTreeSet::from([id(5), id(2)]));
    for (_, progress, _) in &mut started {
        progress(50, 100);
    }
    assert_eq!(repo.pack_line(0).unwrap(), "Downloading 66%");
    assert!(repo.list_line(1).unwrap().ends_with("  \u{2193} 66%"));
    assert!(
        repo.x_pack(Some(0))
            .contains("every map is installed or downloading")
    );
    let (_, _, done) = started.pop().unwrap();
    done(Ok(PathBuf::from("/data/maps/live")));
    assert_eq!(repo.pack_line(0).unwrap(), "Downloading 83%");
    let (_, _, done) = started.pop().unwrap();
    done(Ok(PathBuf::from("/data/maps/live")));
    assert_eq!(repo.pack_line(0).unwrap(), "All Installed");
    assert_eq!(repo.list_line(1).unwrap(), "Phantom Mappack  \u{2713}");
    assert!(
        repo.list_description(1)
            .unwrap()
            .ends_with("3/3 installed, 10.2 MB\nChange Map > Mods")
    );
}

#[test]
fn x_on_a_packs_row_downloads_every_map_of_it() {
    let (fake, repo) = listed(with_pack(), None);
    assert_eq!(
        repo.x_list(Some(1)),
        "mods: Phantom Mappack: downloading 3 maps"
    );
    assert_eq!(fake.installs.lock().unwrap().len(), 3);
    assert_eq!(repo.selected(), None, "nothing opened");
}

#[test]
fn a_pack_without_words_says_what_it_is_built_on_and_the_open_pack_survives_a_snapshot() {
    let mut items = with_pack();
    for e in &mut items {
        e.pack_description = None;
    }
    let (_, repo) = listed(items.clone(), None);
    assert_eq!(
        repo.list_description(1).unwrap().lines().nth(1),
        Some("On Ascension")
    );
    repo.press_list(1);
    let rows = PackRows::new(Arc::clone(&repo));
    repo.set_pictures(BTreeMap::from([("ascension".to_string(), 0xe100_0030)]));
    assert_eq!(rows.picture(0), Some(0xe100_0030), "its newest map's");
    assert_eq!(
        ListRows::new(Arc::clone(&repo)).picture(1),
        Some(0xe100_0030)
    );
    let bytes = repo.save_state();
    let (_, other) = listed(items, None);
    other.load_state(&bytes).unwrap();
    assert_eq!(
        other.pack().map(|(n, m)| (n, m.len())),
        Some(("Phantom Mappack".into(), 3))
    );
    assert_eq!(
        other.texts.get(PACK_TITLE).as_deref(),
        Some("Phantom Mappack")
    );
    // A snapshot from before packs (the page only) still loads.
    other.load_state(&bytes[..33]).unwrap();
    assert_eq!(other.pack(), None);
}

#[test]
fn a_big_packs_texts_fit_and_have_ids_of_the_family() {
    let items: Vec<ModEntry> = (0..50u8)
        .map(|n| {
            let mut e = entry(
                n,
                "An Extremely Long Mod Name That Goes On",
                Some(u64::from(n)),
            );
            e.pack = Some("A Pack Whose Name Is Longer Than Any Line".into());
            e.pack_description = Some("words ".repeat(60));
            e.credit = Some(format!("Maker Number {n}"));
            e
        })
        .collect();
    let (_, repo) = listed(items, None);
    assert_eq!(repo.list_count(), 1, "fifty maps, one row");
    let line = repo.list_line(0).unwrap();
    assert!(text_fit::width(&line) <= text_fit::ROW, "{line:?}");
    let d = repo.list_description(0).unwrap();
    assert!(
        d.len() < SLOT_LEN as usize && d.lines().count() <= 5,
        "{d:?}"
    );
    assert_eq!(repo.press_list(0), Chosen::Open(PACK));
    let rows = PackRows::new(Arc::clone(&repo));
    assert_eq!(rows.count(), MAX_PACK_ROWS, "Download All and all 40 maps");
    assert!(TEXTS.owns(rows.title()));
    for r in 0..rows.count() as i16 {
        let texts = rows.texts(r);
        assert_eq!(texts.len(), 1, "row {r}");
        assert!(texts.iter().all(|(_, id)| TEXTS.owns(*id)));
        assert!(TEXTS.owns(rows.description(r).unwrap()));
        let line = repo.pack_line(r).unwrap();
        assert!(text_fit::width(&line) <= text_fit::ROW, "{line:?}");
        let d = repo.pack_description(r).unwrap();
        assert!(
            d.len() < SLOT_LEN as usize && d.lines().count() <= 5,
            "{d:?}"
        );
    }
    assert!(rows.texts(MAX_PACK_ROWS as i16).is_empty());
    assert_eq!(repo.pack_line(0).unwrap(), "Download All (40)");
    assert!(rows.describe(2).unwrap().contains("...\"; rows: [0]"));
}

#[test]
fn a_cold_visit_can_retry_and_browse_every_row_without_changing_focus_on_refresh() {
    let fake = Arc::new(Fake::default());
    let repo = Repo::new(fake, None);
    let rows = ListRows::new(Arc::clone(&repo));
    repo.opened();
    let now = Instant::now();
    repo.fetch(now);
    repo.listed(Err(ModsError::Offline));
    assert!(matches!(rows.press(0), Chosen::Stepped(_)));
    assert_eq!(repo.list_state(), List::Loading);
    assert!(repo.due(now));
    assert_eq!(rows.count(), 1);
    assert_eq!(rows.growing_capacity(), Some(MAX_ROWS));
    repo.listed(Ok(three()));
    assert_eq!(rows.count(), 3);
    assert_eq!(repo.menu_text(), "Mods");
    assert_eq!(rows.press(2), Chosen::Open(DETAIL));
    repo.listed(Ok(vec![entry(4, "New arrival", Some(40))]));
    assert_eq!(repo.menu_text(), "Mods (1 new)");
    assert_eq!(rows.count(), 3);
    assert_eq!(repo.selected().unwrap().id, id(1));
    repo.closed();
    repo.opened();
    assert_eq!(rows.count(), 1);
    assert_eq!(repo.list_row(0).unwrap().0, "New arrival");
}

#[test]
fn bulk_download_requires_focus_on_download_all_in_a_listed_pack() {
    let (fake, repo) = listed(with_pack(), None);
    repo.press_list(1);
    for row in [None, Some(-1), Some(4)] {
        repo.x_pack(row);
    }
    assert!(fake.installs.lock().unwrap().is_empty());
    repo.listed(Ok(Vec::new()));
    assert_eq!(repo.pack_count(), 1);
    assert_eq!(repo.pack_line(0).as_deref(), Some("Pack unavailable"));
    assert!(repo.pack_description(0).unwrap().contains("Press B"));
    assert_eq!(repo.press_pack(0), Chosen::Nothing);
    repo.x_pack(Some(0));
    assert!(fake.installs.lock().unwrap().is_empty());
}

#[test]
fn wide_published_text_leaves_room_for_state_facts_and_status() {
    let mut map = full();
    map.name = "W".repeat(80);
    map.credit = Some("M".repeat(100));
    map.description = Some("W".repeat(200));
    map.features = FEATURES.iter().map(|s| s.to_string()).collect();
    let (_, repo) = listed(vec![map], None);
    let list = spec::list_spec(&repo);
    let row = &list.rows[0];
    assert!(row.label.ends_with("...  ★"));
    assert!(text_fit::width(&row.label) <= text_fit::ROW);
    let summary = row.description.as_deref().unwrap();
    assert!(
        summary
            .lines()
            .all(|l| text_fit::width(l) <= text_fit::SUMMARY)
    );
    assert_eq!(summary.lines().last(), Some("New map"));
    repo.press_list(0);
    let spec = spec::detail_spec(&repo);
    for cell in &spec.texts {
        let [_, left, _, right] = cell.rect;
        assert!(
            cell.text
                .lines()
                .all(|line| text_fit::width(line) <= i32::from(right - left)),
            "{}",
            cell.id
        );
    }
    assert!(spec.texts[page_layout::BODY_TEXT].text.ends_with("..."));
}

#[test]
fn pack_rows_drop_only_the_repeated_pack_name_and_keep_full_page_titles() {
    let repo = spec::sample_repo(spec::Sample::Pack);
    assert_eq!(repo.pack_line(3).as_deref(), Some("Beaver Creek  ★"));
    repo.press_pack(3);
    assert_eq!(
        repo.texts.get(DETAIL_TITLE).as_deref(),
        Some("Sample Pack Beaver Creek")
    );
    let (_, other) = listed(with_pack(), None);
    other.press_list(1);
    assert!(other.pack_line(1).unwrap().starts_with("Phantom Ascension"));
}

#[test]
fn a_restored_page_for_an_unlisted_map_has_a_way_back() {
    let repo = spec::sample_repo(spec::Sample::Page);
    let (_, empty) = listed(Vec::new(), None);
    empty.load_state(&repo.save_state()).unwrap();
    assert_eq!(
        empty.texts.get(DETAIL_TITLE).as_deref(),
        Some("Mod details")
    );
    assert_eq!(empty.detail_line(0).as_deref(), Some("Map unavailable"));
    assert_eq!(
        empty.detail_description(0).as_deref(),
        Some("Press B to go back.")
    );
    assert_eq!(
        empty.prompts(DETAIL, Some(0)),
        [Prompt::new(&[Glyph::B], "Back")]
    );
    assert_eq!(empty.press_detail(0), Chosen::Nothing);
}

#[test]
fn preview_presses_preserve_parent_focus_and_use_the_displayed_download_commands() {
    use crate::ui_kit::input::Input::{A, B, Down, X, Y};
    use crate::ui_kit::page::Live;
    let mut preview = spec::preview::Preview::new(spec::Sample::List);
    assert!(preview.go("focus-2"));
    preview.press(A);
    assert_eq!(preview.spec().name, PACK);
    preview.press(Down);
    preview.press(X);
    let spec = preview.spec();
    assert!(spec.rows[1].label.ends_with("↓ 0%"));
    let footer = spec.footer_text().unwrap();
    assert!(footer.contains("DETAILS") && !footer.contains("DOWNLOAD"));
    preview.press(A);
    assert_eq!(preview.spec().name, DETAIL);
    let before = preview.spec();
    preview.press(Y);
    assert_eq!(preview.spec(), before);
    preview.press(B);
    assert_eq!(
        (preview.spec().name.as_str(), preview.spec().focus),
        (PACK, 1)
    );
    preview.press(B);
    assert_eq!(
        (preview.spec().name.as_str(), preview.spec().focus),
        (SCREEN, 2)
    );
    assert!(preview.go("offline"));
    preview.press(A);
    assert_eq!(preview.spec().rows[0].label, "Loading...");
}

#[test]
fn every_named_mod_state_uses_its_rows_footer_and_fits_the_synthetic_layout() {
    use crate::ui_kit::{layout, page::Live, preview::From};
    for screen in [spec::Sample::List, spec::Sample::Pack, spec::Sample::Page] {
        let preview = spec::preview::Preview::new(screen);
        for (name, page) in preview.specs() {
            assert_eq!(
                page.footer_text(),
                page.rows[page.focus]
                    .prompts
                    .as_ref()
                    .map(|p| footer_legend(p))
            );
            let (view, _) = From::Synthetic.draw(&page, &name).unwrap();
            let report = layout::check(
                &view,
                &crate::ui_kit::preview::SyntheticSource,
                &layout::Rules::for_spec(&page),
            );
            assert!(
                report.issues.is_empty(),
                "{}/{name}: {:?}",
                page.name,
                report.issues
            );
        }
    }
}

#[test]
fn a_refresh_keeps_installed_state_for_maps_still_visible_on_this_visit() {
    let (fake, repo) = listed(three(), None);
    fake.here.lock().unwrap().insert(id(2));
    repo.listed(Ok(three()));
    repo.opened();
    repo.press_list(1);
    repo.listed(Ok(Vec::new()));
    assert_eq!(repo.detail_line(0).as_deref(), Some("Remove"));
    repo.closed();
    repo.listed(Err(ModsError::Offline));
    assert_eq!(repo.list_line(0).as_deref(), Some("Try Again"));
}

#[test]
fn loading_a_snapshot_requires_a_fresh_removal_confirmation() {
    let (fake, repo) = listed(three(), None);
    repo.press_list(1);
    repo.install_done(id(2), Ok(()));
    repo.act();
    let bytes = repo.save_state();
    repo.load_state(&bytes).unwrap();
    assert_eq!(repo.detail_line(0).as_deref(), Some("Remove"));
    repo.act();
    assert!(fake.removed.lock().unwrap().is_empty());
}
