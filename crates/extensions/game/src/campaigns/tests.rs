use super::*;
use content::campaign_list::{FIXTURE, HALO1};
use guest_layout::memory::GuestMemory;
use guest_layout::testing::FakeGuest;

/// The fake guest as the boot's memory trait.
struct G(FakeGuest);
impl Gm for G {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.0.read(addr, out).is_ok()
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.0.write(addr, data).is_ok()
    }
}

const MATG: u32 = 0x8000_0000;
const START_EL: u32 = 0x8000_1000;
const START_LIST: u32 = 0x8000_2000;
const UI_EL: u32 = 0x8001_0000;
const UI_CAMPAIGN_LIST: u32 = 0x8001_1000;
const UI_LEVEL_LIST: u32 = 0x8002_0000;
const REGION: Region = Region::OURS;
const BITM: u32 = 0x6269_746d;

/// Retail-shaped lists: `used` campaign-1 levels named `l0`.. with ids
/// `0x65 + i`, then `free` free slots; each level-select slot its own
/// picture `0xe000_0000 + i`. With `ui`, the level select's lists too.
fn globals(used: u32, free: u32, ui: bool) -> G {
    let mut g = FakeGuest::new();
    g.map(GLOBALS_TAG, 4).map(REGION.base, REGION.len);
    g.put_u32(GLOBALS_TAG, MATG);
    g.put_u32(MATG + MATG_START, 1)
        .put_u32(MATG + MATG_START + 4, START_EL);
    let n = used + free;
    g.put_u32(START_EL, n).put_u32(START_EL + 4, START_LIST);
    for i in 0..n {
        let at = START_LIST + i * START_LEN as u32;
        if i < used {
            let p = format!("scenarios\\solo\\l{i}\\l{i}");
            g.put_u32(at, 1)
                .put_u32(at + 4, 0x65 + i)
                .put(at + 8, p.as_bytes());
        } else {
            g.put_u32(at, FREE).put_u32(at + 4, FREE);
        }
    }
    if ui {
        g.put_u32(MATG + MATG_UI, 1)
            .put_u32(MATG + MATG_UI + 4, UI_EL);
        g.put_u32(UI_EL, 4).put_u32(UI_EL + 4, UI_CAMPAIGN_LIST);
        g.put_u32(UI_EL + 8, n).put_u32(UI_EL + 12, UI_LEVEL_LIST);
        g.put_u32(UI_EL + 16, 0).put_u32(UI_EL + 20, 0);
        for i in 0..4 {
            g.put_u32(
                UI_CAMPAIGN_LIST + i * UI_CAMPAIGN_LEN as u32,
                if i == 0 { 1 } else { FREE },
            );
        }
        g.map(UI_CAMPAIGN_LIST, 4 * UI_CAMPAIGN_LEN as u32);
        for i in 0..n {
            let at = UI_LEVEL_LIST + i * UI_LEVEL_LEN as u32;
            g.map(at, UI_LEVEL_LEN as u32);
            let (c, m) = if i < used {
                (1, 0x65 + i)
            } else {
                (FREE, FREE)
            };
            g.put_u32(at, c).put_u32(at + 4, m);
            g.put_u32(at + 8, BITM).put_u32(at + 12, 0xe000_0000 + i);
        }
    } else {
        g.put_u32(MATG + MATG_UI, 0).put_u32(MATG + MATG_UI + 4, 0);
    }
    G(g)
}

fn built(texts: &[&str]) -> Vec<Built> {
    let ms: Vec<CampaignManifest> = texts
        .iter()
        .map(|t| CampaignManifest::parse(t).unwrap())
        .collect();
    cl::build(&ms, &[1, FREE, FREE, FREE], &[0x65, 0x66, 0x67, 0x68]).unwrap()
}

/// A fixture whose levels are the synthetic retail ones.
const SMALL: &str = r#"format = 1
[campaign]
key = "test"
name = "Second"
[[level]]
map = "l2"
name = "Two again"
picture = "l2"
[[level]]
map = "x1"
name = "New"
"#;

// ---- the game's readers, transcribed (docs/halo1/campaign-picker.md) ----

fn read_list<M: Gm>(m: &M, at: u32, len: usize) -> Vec<Vec<u8>> {
    let b = block(m, at, len as u32).unwrap();
    elements(m, &b).unwrap()
}

fn start_list<M: Gm>(m: &M) -> Vec<(u32, u32, String)> {
    let matg = m.u32(GLOBALS_TAG).unwrap();
    let el = element0(m, matg, MATG_START).unwrap();
    read_list(m, el, START_LEN)
        .iter()
        .map(|e| cl::read_start(e).unwrap())
        .collect()
}

fn ui_levels<M: Gm>(m: &M) -> Vec<(u32, u32, [u32; 2], String)> {
    let matg = m.u32(GLOBALS_TAG).unwrap();
    let el = element0(m, matg, MATG_UI).unwrap();
    read_list(m, el + UI_LEVELS, UI_LEVEL_LEN)
        .iter()
        .map(|e| cl::read_ui_level(e).unwrap())
        .collect()
}

fn ui_campaigns<M: Gm>(m: &M) -> Vec<(u32, String)> {
    let matg = m.u32(GLOBALS_TAG).unwrap();
    let el = element0(m, matg, MATG_UI).unwrap();
    read_list(m, el + UI_CAMPAIGNS, UI_CAMPAIGN_LEN)
        .iter()
        .map(|e| cl::read_ui_campaign(e).unwrap())
        .collect()
}

/// `0x19c270(campaign, map)`: the first start record with both ids.
fn lookup(list: &[(u32, u32, String)], c: u32, m: u32) -> Option<&str> {
    list.iter()
        .find(|(lc, lm, _)| *lc == c && *lm == m)
        .map(|e| e.2.as_str())
}

/// `0x19c6d0(array, campaign)`: the campaign's map ids other than -1,
/// sorted rising; more than 20 would overrun its buffer.
fn level_select_ids(list: &[(u32, u32, String)], c: u32) -> Vec<u32> {
    let mut ids: Vec<u32> = list
        .iter()
        .filter(|e| e.0 == c && e.1 != FREE)
        .map(|e| e.1)
        .collect();
    assert!(ids.len() <= 20, "0x19c6d0's buffer holds 20");
    ids.sort();
    ids
}

/// `0x19c440(campaign, map)`: the smallest id above `map` in the campaign.
fn next_level(list: &[(u32, u32, String)], c: u32, m: u32) -> Option<u32> {
    list.iter()
        .filter(|e| e.0 == c && e.1 as i32 > m as i32)
        .map(|e| e.1)
        .min()
}

/// `0x19c1f0(map)`: the first level-select record with that map id.
fn ui_record(
    list: &[(u32, u32, [u32; 2], String)],
    m: u32,
) -> Option<&(u32, u32, [u32; 2], String)> {
    list.iter().find(|e| e.1 == m)
}

/// Every check the game's readers imply, over the loaded lists.
fn game_reads(g: &G, built: &[Built]) {
    let start = start_list(g);
    let ui = ui_levels(g);
    cl::check(&start.iter().map(|e| (e.0, e.1)).collect::<Vec<_>>()).unwrap();
    for b in built {
        cl::check_built(b).unwrap();
        let ids = level_select_ids(&start, b.campaign_id);
        assert_eq!(ids, b.levels.iter().map(|l| l.map_id).collect::<Vec<_>>());
        for (i, l) in b.levels.iter().enumerate() {
            assert_eq!(
                lookup(&start, b.campaign_id, l.map_id),
                Some(l.scenario.as_str())
            );
            let rec = ui_record(&ui, l.map_id).expect("a level-select record");
            assert_eq!((rec.0, &rec.3), (b.campaign_id, &l.names[0]));
            assert_eq!(rec.2[0], BITM, "a picture");
            let next = next_level(&start, b.campaign_id, l.map_id);
            assert_eq!(next, b.levels.get(i + 1).map(|n| n.map_id));
        }
    }
}

#[test]
fn a_small_campaign_goes_into_the_free_slots() {
    let mut g = globals(4, 3, true);
    let b = built(&[SMALL]);
    let r = extend(&mut g, &b, &REGION).unwrap();
    assert_eq!(
        (r.in_place, r.appended),
        ([2, 1, 2], [0, 0, 0]),
        "{}",
        r.line()
    );
    // Nothing moved: the blocks still point at the tag's own lists.
    assert_eq!(g.u32(START_EL + 4), Some(START_LIST));
    assert_eq!(g.u32(UI_EL + 12), Some(UI_LEVEL_LIST));
    let start = start_list(&g);
    assert_eq!(start.len(), 7);
    assert_eq!(start[4], (2, 0x1001, "scenarios\\solo\\l2\\l2".into()));
    assert_eq!(start[5], (2, 0x1002, "scenarios\\solo\\x1\\x1".into()));
    assert_eq!(start[6].0, FREE);
    // The retail records are untouched.
    assert_eq!(start[2], (1, 0x67, "scenarios\\solo\\l2\\l2".into()));
    let ui = ui_levels(&g);
    // `picture = "l2"`: retail l2's picture; the other keeps its slot's.
    assert_eq!(ui[4].2, [BITM, 0xe000_0002]);
    assert_eq!(ui[5].2, [BITM, 0xe000_0005]);
    assert_eq!(ui_campaigns(&g)[1], (2, "Second".into()));
    game_reads(&g, &b);
    // Retail still reads as before.
    assert_eq!(level_select_ids(&start, 1), [0x65, 0x66, 0x67, 0x68]);
    // Again: nothing to do.
    let again = extend(&mut g, &b, &REGION).unwrap();
    assert!(again.already, "{}", again.line());
    assert_eq!(start_list(&g), start);
}

#[test]
fn a_level_with_a_card_shows_it() {
    let mut g = globals(4, 3, true);
    let b = built(&[SMALL]);
    let cards = BTreeMap::from([("x1".to_owned(), 0xe123_0456)]);
    extend_with_cards(&mut g, &b, &REGION, &cards).unwrap();
    let ui = ui_levels(&g);
    // l2 has no card: retail l2's picture, as without cards.
    assert_eq!(ui[4].2, [BITM, 0xe000_0002]);
    assert_eq!(ui[5].2, [CARD_GROUP, 0xe123_0456]);
    assert_eq!(CARD_GROUP, BITM);
    game_reads(&g, &b);
}

#[test]
fn a_campaign_past_the_free_slots_is_copied_into_our_memory() {
    let mut g = globals(4, 2, true);
    let b = built(&[HALO1]);
    let r = extend(&mut g, &b, &REGION).unwrap();
    assert_eq!(
        (r.in_place, r.appended),
        ([2, 1, 2], [8, 0, 8]),
        "{}",
        r.line()
    );
    let at = g.u32(START_EL + 4).unwrap();
    assert!((REGION.base..REGION.base + REGION.len).contains(&at));
    assert_eq!(g.u32(START_EL), Some(14));
    assert_eq!(g.u32(UI_EL + 8), Some(14));
    assert_eq!(g.u32(UI_EL), Some(4), "the campaigns fit");
    let start = start_list(&g);
    // Retail first, as it was; then ours in the free slots, then after.
    assert_eq!(start[0], (1, 0x65, "scenarios\\solo\\l0\\l0".into()));
    assert_eq!(start[4].2, "scenarios\\solo\\a10\\a10");
    assert_eq!(start[13].2, "scenarios\\solo\\d40\\d40");
    let ui = ui_levels(&g);
    // No picture named: free slots keep their own, the rest the list's first.
    assert_eq!(ui[4].2, [BITM, 0xe000_0004]);
    assert_eq!(ui[13].2, [BITM, 0xe000_0000]);
    game_reads(&g, &b);
    assert!(extend(&mut g, &b, &REGION).unwrap().already);
}

#[test]
fn two_campaigns_keep_apart() {
    let mut g = globals(4, 3, true);
    let mut other = CampaignManifest::parse(FIXTURE).unwrap();
    other.levels.truncate(1);
    other.levels[0].map = "l1".into();
    other.levels[0].picture = Some("l1".into());
    let text = toml::to_string(&other).unwrap();
    let b = built(&[HALO1, &text]);
    extend(&mut g, &b, &REGION).unwrap();
    game_reads(&g, &b);
    let c: Vec<u32> = ui_campaigns(&g).iter().map(|c| c.0).collect();
    assert_eq!(c, [1, 2, 3, FREE]);
}

#[test]
fn a_level_map_without_the_level_select_gets_its_start_list_only() {
    let mut g = globals(4, 3, false);
    let b = built(&[SMALL]);
    let r = extend(&mut g, &b, &REGION).unwrap();
    assert_eq!((r.in_place, r.appended), ([2, 0, 0], [0, 0, 0]));
    let start = start_list(&g);
    assert_eq!(next_level(&start, 2, 0x1001), Some(0x1002));
}

#[test]
fn no_globals_or_no_list_is_said_and_left() {
    let mut g = G(FakeGuest::new());
    g.0.map(GLOBALS_TAG, 4);
    assert!(extend(&mut g, &built(&[SMALL]), &REGION).unwrap().no_lists);
    let mut g = globals(1, 1, true);
    g.0.put_u32(MATG + MATG_START, 0);
    assert!(extend(&mut g, &built(&[SMALL]), &REGION).unwrap().no_lists);
}

#[test]
fn a_full_region_is_an_error_not_a_partial_list() {
    let mut g = globals(4, 0, true);
    let small = Region {
        base: REGION.base,
        len: 0x100,
    };
    assert!(extend(&mut g, &built(&[HALO1]), &small).is_err());
    assert_eq!(
        g.u32(START_EL + 4),
        Some(START_LIST),
        "the block is not repointed"
    );
}

#[test]
fn the_start_takes_the_campaign_of_the_selected_level() {
    let b = built(&[SMALL]);
    let state = State {
        built: b,
        selection: Selection::default(),
        progress: Progress::memory(),
        cards: Default::default(),
    };
    let mut g = G(FakeGuest::new());
    g.0.map(SELECTED_CAMPAIGN, 8);
    g.0.put_u32(SELECTED_CAMPAIGN, 1)
        .put_u32(SELECTED_MAP, 0x1002);
    assert_eq!(state.fix_selection(&mut g), Some(2));
    assert_eq!(g.u32(SELECTED_CAMPAIGN), Some(2));
    // Back on a Halo 2 level, campaign 1 again.
    g.0.put_u32(SELECTED_MAP, 0x69);
    assert_eq!(state.fix_selection(&mut g), Some(1));
    assert_eq!(state.fix_selection(&mut g), None, "already right");
    // A campaign id that is neither retail nor ours is left alone.
    g.0.put_u32(SELECTED_CAMPAIGN, 9)
        .put_u32(SELECTED_MAP, 0x1001);
    assert_eq!(state.fix_selection(&mut g), None);
    assert_eq!(state.key_of(0x1001), Some("test"));
    assert_eq!(state.key_of(0x69), None);
    assert_eq!(state.key_of_campaign(2), Some("test"));
}

#[test]
fn progress_rises_per_difficulty_and_profile_and_persists() {
    let dir = std::env::temp_dir().join(format!("h2-campaigns-progress-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join(PROGRESS_FILE);
    let (p, why) = Progress::load(path.clone());
    assert!(why.is_none());
    let chief = vec!["Chief".to_string()];
    assert_eq!(
        p.max(&chief, "halo1"),
        -1,
        "nothing finished: the first level only"
    );
    assert!(p.finished("Chief", "halo1", 1, 0x1003));
    assert!(!p.finished("Chief", "halo1", 1, 0x1002), "never lowered");
    assert!(p.finished("Chief", "halo1", 3, 0x1001));
    let games = built(&[HALO1]);
    p.launched("Chief", "halo1", &games[0].levels[3], 2, false);
    p.checkpoint_saved("Chief", "halo1");
    assert!(p.finished("Arbiter", "halo1", 0, 0x1005));
    assert_eq!(p.max(&chief, "halo1"), 0x1003);
    assert_eq!(p.max(&["Chief".into(), "Arbiter".into()], "halo1"), 0x1005);
    assert_eq!(p.max(&chief, "test"), -1);
    let (again, _) = Progress::load(path.clone());
    assert_eq!(
        again.get("Chief", "halo1"),
        Some(CampaignProgress {
            finished: [-1, 0x1003, -1, 0x1001],
            last: 0x1004,
            last_map: Some("b30".into()),
            difficulty: Some(2),
            checkpoint: true,
        })
    );
    // An unreadable file is reported and never overwritten.
    std::fs::write(&path, "not = [toml").unwrap();
    let (bad, why) = Progress::load(path.clone());
    assert!(why.is_some());
    bad.finished("Chief", "halo1", 0, 0x1009);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not = [toml");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn installed_lists_campaigns_with_maps_and_says_why_not() {
    let dir = std::env::temp_dir().join(format!("h2-campaigns-installed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let none = installed(&dir, &|_| false);
    assert!(
        none.0.is_empty() && none.1.is_empty(),
        "no folder: nothing, silently"
    );
    let root = dir.join("campaigns");
    let fixture = root.join("test-only");
    std::fs::create_dir_all(&fixture).unwrap();
    std::fs::write(fixture.join(cl::MANIFEST_FILE), FIXTURE).unwrap();
    assert!(!has_installed_halo1(&dir));
    std::fs::remove_dir_all(fixture).unwrap();
    // Halo 1 with two of its maps converted.
    std::fs::create_dir_all(root.join("halo1")).unwrap();
    std::fs::write(root.join("halo1").join(cl::MANIFEST_FILE), HALO1).unwrap();
    assert!(!has_installed_halo1(&dir), "a manifest alone is not a campaign");
    std::fs::write(root.join("halo1/a10.map"), b"x").unwrap();
    assert!(!has_installed_halo1(&dir), "a loose map is ignored");
    store_fixture(&dir.join("tags"), "halo1_campaign_a10_h1", b"x");
    assert!(has_installed_halo1(&dir));
    store_fixture(&dir.join("tags"), "halo1_campaign_a30_h1", b"x");
    std::fs::write(
        root.join("halo1/import.stamp"),
        concat!(
            "converter=90\n",
            "sha256=0000000000000000000000000000000000000000000000000000000000000000\n",
            "size=1\nmtime=0\n",
            "map a10 ok\nmap a30 ok\nmap a50 failed conversion error\n",
        ),
    )
    .unwrap();
    // The fixture: retail maps, from the disc.
    std::fs::create_dir_all(root.join("test")).unwrap();
    std::fs::write(root.join("test").join(cl::MANIFEST_FILE), FIXTURE).unwrap();
    // A folder whose manifest names another key, and a broken one.
    std::fs::create_dir_all(root.join("wrong")).unwrap();
    std::fs::write(root.join("wrong").join(cl::MANIFEST_FILE), HALO1).unwrap();
    std::fs::create_dir_all(root.join("broken")).unwrap();
    std::fs::write(root.join("broken").join(cl::MANIFEST_FILE), "format = 7").unwrap();
    let disc = |m: &str| m.starts_with('0');
    let (ms, lines) = installed(&dir, &disc);
    let keys: Vec<&str> = ms.iter().map(|m| m.campaign.key.as_str()).collect();
    assert_eq!(keys, ["halo1", "test"]);
    assert_eq!(
        ms[0]
            .levels
            .iter()
            .map(|l| l.map.as_str())
            .collect::<Vec<_>>(),
        ["a10", "a30"]
    );
    assert_eq!(ms[1].levels.len(), 3);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("8 of 10 levels have no map")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("wrong") && l.contains("key")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("broken")), "{lines:?}");
    // No map at all: not listed.
    let (ms, lines) = installed(&dir, &|_| false);
    assert_eq!(ms.len(), 1);
    assert!(
        lines.iter().any(|l| l.contains("test: no level has a map")),
        "{lines:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_picker_lists_halo_1_then_halo_2_then_the_rest() {
    let b = built(&[HALO1, FIXTURE]);
    let p = Picker::new(&b);
    assert_eq!(
        p.rows,
        [
            (2, "Halo: Combat Evolved".to_string()),
            (1, "Halo 2".to_string()),
            (3, "Second Campaign (test)".to_string())
        ]
    );
    assert_eq!(p.campaign(0), Some(2));
    assert_eq!(p.campaign(1), Some(1));
    assert_eq!(p.campaign(2), Some(3));
    // No order, or one at or past Halo 2's: below Halo 2.
    let late = HALO1.replace("order = 10", &format!("order = {}", cl::HALO2_ORDER));
    assert_eq!(Picker::new(&built(&[&late])).rows[0].0, RETAIL_CAMPAIGN);
    let none = HALO1.replace("order = 10\n", "");
    assert_eq!(Picker::new(&built(&[&none])).rows[0].0, RETAIL_CAMPAIGN);
    assert_eq!(p.campaign(3), None);
    assert_eq!(p.campaign(-1), None);
}

/// The user's disc: our records go into `mainmenu.map`'s real lists (read
/// from the ISO, laid into a fake guest at their own addresses), and every
/// reader the game has for them finds what it should. The retail records
/// read as before. Writes nothing.
/// `H2_DISC=disc/halo2.iso cargo test -p boot --lib campaigns -- --ignored --nocapture`
#[test]
#[ignore = "needs the disc (H2_DISC)"]
fn real_disc_lists_take_both_campaigns() {
    let disc = PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
    let mut iso =
        xiso::Xiso::open(std::io::BufReader::new(std::fs::File::open(&disc).unwrap())).unwrap();
    let base = iso.partition_offset();
    let entries = iso.entries().unwrap().clone();
    let e = entries
        .iter()
        .find(|e| e.path.to_ascii_lowercase().ends_with("maps/mainmenu.map"))
        .unwrap();
    let mut m = map::Map::from_region(
        std::io::BufReader::new(std::fs::File::open(&disc).unwrap()),
        base + u64::from(e.sector) * xiso::SECTOR,
        e.size.into(),
    )
    .unwrap();
    let tag = m.globals().unwrap().clone();
    let matg = m.read_tag_bytes(tag.id).unwrap();
    // Lay the globals tag and the three lists into a fake guest.
    let mut g = FakeGuest::new();
    g.map(REGION.base, REGION.len);
    g.put_u32(GLOBALS_TAG, tag.index_address);
    g.put(tag.index_address, &matg);
    let mut copy = |g: &mut FakeGuest, addr: u32, len: u32| {
        let off = m.file_offset(addr, len).unwrap();
        let bytes = m.read_file_range(off, len.into()).unwrap();
        g.put(addr, &bytes);
        bytes
    };
    let le = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let start_el = le(&matg, MATG_START as usize + 4);
    let head = copy(&mut g, start_el, 8);
    copy(&mut g, le(&head, 4), le(&head, 0) * START_LEN as u32);
    let ui_el = le(&matg, MATG_UI as usize + 4);
    let head = copy(&mut g, ui_el, 0x18);
    copy(&mut g, le(&head, 4), le(&head, 0) * UI_CAMPAIGN_LEN as u32);
    copy(&mut g, le(&head, 12), le(&head, 8) * UI_LEVEL_LEN as u32);
    let mut g = G(g);

    let before = start_list(&g);
    let retail_ids: Vec<u32> = before.iter().map(|e| e.1).collect();
    let retail_campaigns: Vec<u32> = ui_campaigns(&g).iter().map(|c| c.0).collect();
    println!(
        "retail: {} start slots ({} used), {} campaign slots, {} level-select slots",
        before.len(),
        before.iter().filter(|e| e.0 != FREE).count(),
        retail_campaigns.len(),
        ui_levels(&g).len()
    );
    let ms = [
        CampaignManifest::parse(HALO1).unwrap(),
        CampaignManifest::parse(FIXTURE).unwrap(),
    ];
    let b = cl::build(&ms, &retail_campaigns, &retail_ids).unwrap();
    assert!(
        b.iter()
            .flat_map(|c| &c.levels)
            .all(|l| !retail_ids.contains(&l.map_id))
    );
    let r = extend(&mut g, &b, &REGION).unwrap();
    println!("{}", r.line());
    game_reads(&g, &b);
    // Halo 2 reads exactly as before.
    let after = start_list(&g);
    assert_eq!(level_select_ids(&after, 1), level_select_ids(&before, 1));
    for (c, m, p) in before.iter().filter(|e| e.0 == 1) {
        assert_eq!(lookup(&after, *c, *m), Some(p.as_str()));
        assert_eq!(next_level(&after, 1, *m), next_level(&before, 1, *m));
    }
    // The fixture's levels show their retail level's picture.
    let ui = ui_levels(&g);
    for l in &b[1].levels {
        let own = before
            .iter()
            .find(|e| e.0 == 1 && e.2.ends_with(&format!("\\{}", l.map)))
            .unwrap()
            .1;
        assert_eq!(
            ui_record(&ui, l.map_id).unwrap().2,
            ui_record(&ui, own).unwrap().2
        );
    }
}

/// `0x214db0`: the file the cache loader opens for a scenario path: a `t:\`
/// or `w:\` path as `<path>.map`, anything else as
/// `d:\maps\<basename>.map` (docs/halo1/campaign-picker.md §3).
fn map_file_of(scenario: &str) -> String {
    let lower = scenario.to_ascii_lowercase();
    if lower.starts_with("t:\\") || lower.starts_with("w:\\") {
        return format!("{scenario}.map");
    }
    let base = scenario.rsplit('\\').next().unwrap_or(scenario);
    format!("d:\\maps\\{base}.map")
}

/// What `h2 import halo1` leaves in `<data>/campaigns/halo1/`
/// (docs/halo1/campaign.md "Importing"), followed from the picker to the
/// file the game opens: the picker lists Halo 1 after Halo 2; its level
/// list holds the ten levels in play order with their names; each level's
/// start record (`0x19c270` by the campaign and map id the menu selects)
/// names a scenario path that `0x214db0` turns into `d:\maps\<level>.map`;
/// and `vfs::CampaignMapsBackend`, given [`host_maps`], serves exactly the
/// converted file there. The import's stamp and `source/` folder beside the
/// maps change nothing.
#[test]
fn a_halo1_import_is_picked_started_and_served() {
    use vfs::Backend;
    let dir = std::env::temp_dir().join(format!("h2-campaigns-h1chain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // The disc: retail maps only.
    let disc = dir.join("disc");
    std::fs::create_dir_all(disc.join("maps")).unwrap();
    std::fs::write(disc.join("maps/mainmenu.map"), b"retail mainmenu").unwrap();
    std::fs::write(disc.join("maps/01a_tutorial.map"), b"retail tutorial").unwrap();
    // The import's output.
    let data = dir.join("data");
    let h1 = data.join("campaigns/halo1");
    std::fs::create_dir_all(h1.join("source")).unwrap();
    std::fs::write(h1.join(cl::MANIFEST_FILE), HALO1).unwrap();
    let levels = [
        ("a10", "The Pillar of Autumn"),
        ("a30", "Halo"),
        ("a50", "The Truth and Reconciliation"),
        ("b30", "The Silent Cartographer"),
        ("b40", "Assault on the Control Room"),
        ("c10", "343 Guilty Spark"),
        ("c20", "The Library"),
        ("c40", "Two Betrayals"),
        ("d20", "Keyes"),
        ("d40", "The Maw"),
    ];
    for (l, _) in levels {
        store_fixture(&data.join("tags"), &format!("halo1_campaign_{l}_h1"), format!("converted {l}").as_bytes());
    }
    std::fs::write(h1.join("import.stamp"), "converter=1\n").unwrap();
    std::fs::write(h1.join("source/manifest.txt"), "x").unwrap();

    let (ms, lines) = installed(&data, &|m| m == "01a_tutorial" || m == "mainmenu");
    assert!(lines.is_empty(), "{lines:?}");
    let retail: Vec<u32> = (0..15).map(|i| 0x65 + i).collect();
    let built = cl::build(&ms, &[1], &retail).unwrap();
    assert_eq!(built.len(), 1);
    let b = &built[0];

    // The picker: Halo 1, then Halo 2.
    let picker = Picker::new(&built);
    assert_eq!(picker.rows[0], (b.campaign_id, "Halo: Combat Evolved".to_string()));
    assert_eq!(picker.rows[1].0, RETAIL_CAMPAIGN);
    assert_eq!(picker.campaign(0), Some(b.campaign_id));
    // Its levels, in play order, with names.
    let got: Vec<(&str, &str)> = b
        .levels
        .iter()
        .map(|l| (l.map.as_str(), l.names[0].as_str()))
        .collect();
    assert_eq!(got, levels);

    // The lists the game reads, extended (15 retail levels, 5 free slots).
    let mut g = globals(15, 5, true);
    extend(&mut g, &built, &REGION).unwrap();
    game_reads(&g, &built);
    let start = start_list(&g);
    assert_eq!(
        level_select_ids(&start, b.campaign_id),
        b.levels.iter().map(|l| l.map_id).collect::<Vec<_>>()
    );

    // The VFS as boot builds it: our maps over the disc.
    let state = State {
        built: built.clone(),
        selection: Selection::default(),
        progress: Progress::memory(),
        cards: Default::default(),
    };
    let maps = host_maps(&data, &state);
    assert_eq!(maps.len(), 10);
    let mut fs = vfs::CampaignMapsBackend::new(vfs::DirDiscBackend::open(&disc).unwrap(), maps);
    for l in &b.levels {
        // The menu's start: the selected map id, the campaign fixed to it.
        let scenario = lookup(&start, b.campaign_id, l.map_id).expect("a start record");
        let file = map_file_of(scenario);
        assert_eq!(file, format!("d:\\maps\\{}.map", l.map));
        let path = file.strip_prefix("d:\\").unwrap();
        let want = format!("converted {}", l.map);
        let meta = fs.stat(path).unwrap_or_else(|e| panic!("{path}: {e:?}"));
        assert_eq!(meta.end_of_file, want.len() as u64, "{path}");
        let mut buf = vec![0u8; want.len()];
        assert_eq!(fs.read_at(path, 0, &mut buf).unwrap(), want.len());
        assert_eq!(buf, want.as_bytes(), "{path}");
    }
    // The disc's own maps are still the disc's.
    let mut buf = [0u8; 15];
    fs.read_at("maps\\mainmenu.map", 0, &mut buf).unwrap();
    assert_eq!(&buf, b"retail mainmenu");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every 1.0 caller of the `(campaign, map)` lookups (`0x19c270`, the
/// start record; `0x19c440`, the next level), driven with a Halo 1 map id
/// and the campaign each passes. Several pass campaign 1 whatever the map:
/// without the hooks' [`State::lookup_campaign`] they find nothing, and the
/// level select's press (`0x2c5118`) reads the null record and faults (the
/// user's crash on picking Halo CE). With it, each finds Halo 1's record or
/// next level; a caller that already passes the right campaign is left
/// alone, and a retail map asked under one of our campaigns goes back to
/// Halo 2's.
#[test]
fn every_lookup_caller_finds_a_halo1_level() {
    let built = built(&[HALO1]);
    let h1 = built[0].campaign_id;
    let a10 = built[0].levels[0].map_id;
    let a30 = built[0].levels[1].map_id;
    let d40 = built[0].levels[9].map_id;
    let state = State {
        built: built.clone(),
        selection: Selection::default(),
        progress: Progress::memory(),
        cards: Default::default(),
    };
    let mut g = globals(4, 3, true);
    extend(&mut g, &built, &REGION).unwrap();
    let start = start_list(&g);
    let fixed = |c: u32, m: u32| state.lookup_campaign(c, m).unwrap_or(c);

    // 0x19c270(campaign, map) callers: (site, what, campaign passed, map).
    let record_calls = [
        (0x2c5118, "level select press: campaign 1 hard-coded", 1, a10),
        (0x199ab7, "Continue: campaign 1, the profile's progress", 1, a30),
        (0x199ac5, "Continue, second lookup", 1, a30),
        (0x148d6d, "selection globals 0x54e7c0/0x54e7c4, still 1", 1, a30),
        (0x148d6d, "selection globals, already ours", h1, a30),
        (0x18e9ec, "game won: the options' campaign", h1, a30),
        (0x163664, "0x163610: the level's own campaign", h1, d40),
        (0x19c98a, "0x19c960: its caller's campaign", 1, d40),
    ];
    for (site, what, c, m) in record_calls {
        let want = built[0].levels.iter().find(|l| l.map_id == m).unwrap();
        if c != h1 {
            assert_eq!(lookup(&start, c, m), None, "{site:#x} {what}: the fault without the hook");
        }
        assert_eq!(
            lookup(&start, fixed(c, m), m),
            Some(want.scenario.as_str()),
            "{site:#x} {what}"
        );
    }
    // 0x19c440(campaign, map) callers.
    let next_calls = [
        (0x24a9e9, "0x24a9xx: campaign 1 hard-coded", 1, a10, Some(a30)),
        (0x163658, "0x163610", h1, a10, Some(a30)),
        (0x18e9da, "game won", h1, d40, None),
    ];
    for (site, what, c, m, want) in next_calls {
        assert_eq!(next_level(&start, fixed(c, m), m), want, "{site:#x} {what}");
    }
    assert_eq!(next_level(&start, 1, a10), None, "without the hook");

    // Left alone: the right campaign already, retail under retail.
    assert_eq!(state.lookup_campaign(h1, a10), None);
    assert_eq!(state.lookup_campaign(1, 0x65), None);
    assert_eq!(state.lookup_campaign(7, 0x65), None, "a campaign not ours");
    // A retail map asked under ours: Halo 2's.
    assert_eq!(state.lookup_campaign(h1, 0x65), Some(1));
    assert_eq!(lookup(&start, fixed(h1, 0x65), 0x65), Some("scenarios\\solo\\l0\\l0"));
}

#[test]
fn snapshots_wait_while_the_level_select_is_built() {
    let s = Selection::default();
    assert_eq!(s.snapshot_live(), None);
    s.set(2);
    assert_eq!(s.snapshot_live(), None, "the campaign shown is menu state");
    s.set_building(true);
    assert!(s.snapshot_live().is_some());
    s.set_building(false);
    assert_eq!(s.snapshot_live(), None);
}

/// The picker's picture for a campaign: its first level's in the level
/// select's list, read from the loaded map's records.
#[test]
fn each_campaign_s_picture_is_its_first_level_s() {
    let mut g = globals(4, 3, true);
    let b = built(&[SMALL]);
    let cards = BTreeMap::from([("x1".to_owned(), 0xe123_0456)]);
    extend_with_cards(&mut g, &b, &REGION, &cards).unwrap();
    let id = b[0].campaign_id;
    assert_eq!(
        first_pictures(&g),
        BTreeMap::from([(1, 0xe000_0000), (id, 0xe000_0002)]),
        "Halo 2's first level; ours borrows retail l2's"
    );
    assert!(first_pictures(&globals(4, 3, false)).is_empty());
}
