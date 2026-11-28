use super::*;
use live_proto::Xuid;
use std::collections::HashMap;

// ---- a fake HLE ----

/// What `set_coop` was called with.
type CoopCall = (Option<Campaign>, Option<Vec<u8>>);

#[derive(Default)]
struct FakeLive {
    coop: Mutex<Vec<CoopCall>>,
    friends: Vec<live_hle::Friend>,
    invites: Mutex<Vec<Xuid>>,
    joined: Mutex<Option<live_hle::Joined>>,
    /// What a ping to the host answers, and the pings made.
    echo: Option<u16>,
    pings: Mutex<Vec<(std::net::SocketAddrV4, Option<u16>)>>,
    /// The guest's report the host reads, and what it advertised.
    reported: Option<u16>,
    advertised: Mutex<Vec<Option<u16>>>,
    slots: Mutex<Vec<live_hle::SessionSlots>>,
}

impl Live for FakeLive {
    fn set_coop(&self, campaign: Option<Campaign>, skulls: Option<Vec<u8>>) {
        self.coop.lock().unwrap().push((campaign, skulls));
    }
    fn friends(&self) -> Vec<live_hle::Friend> {
        self.friends.clone()
    }
    fn invite(&self, to: Xuid) -> Result<String, u32> {
        self.invites.lock().unwrap().push(to);
        Ok("8000000000000001".into())
    }
    fn joined(&self) -> live_hle::Joined {
        self.joined
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(live_hle::Joined::NotJoined)
    }
    fn ping_host(&self, host: std::net::SocketAddrV4, report: Option<u16>) -> Option<u16> {
        self.pings.lock().unwrap().push((host, report));
        self.echo
    }
    fn guest_ping(&self) -> Option<u16> {
        self.reported
    }
    fn sync_squad_slots(&self, slots: live_hle::SessionSlots) {
        self.slots.lock().unwrap().push(slots);
    }
    fn set_coop_ping(&self, ms: Option<u16>) {
        self.advertised.lock().unwrap().push(ms);
    }
}

fn friend(x: u64, tag: &str, online: bool) -> live_hle::Friend {
    live_hle::Friend {
        xuid: Xuid(x),
        gamertag: tag.into(),
        online,
        playing: false,
        activity: None,
    }
}

/// Invented levels: two of campaign 1 (one unnamed), one of another
/// campaign, one without ids.
fn levels_fixture() -> Vec<Level> {
    levels(
        &[
            (1, 10, "scenarios\\solo\\01x_first\\01x_first".into()),
            (1, 11, "scenarios\\solo\\02x_second\\02x_second".into()),
            (7, 70, "scenarios\\solo\\other\\other".into()),
        ],
        &[(1, 10, "First Level".into()), (7, 70, "Other".into())],
    )
}

fn lobby_with(live: Arc<FakeLive>) -> (Arc<Lobby>, Arc<Arm>) {
    let arm = Arm::session();
    let skulls: HostSkulls = Box::new(|profiles| {
        if profiles.iter().any(|p| p == "Chief") {
            vec![0, 4]
        } else {
            Vec::new()
        }
    });
    let lobby = Arc::new(Lobby::new(
        levels_fixture(),
        Some(Arc::clone(&arm)),
        Some(live as Arc<dyn Live>),
        skulls,
    ));
    (lobby, arm)
}

fn session(peers: u32, players: u32) -> coop_probe::Session {
    coop_probe::Session {
        addr: 0x1000,
        state: 5,
        peers,
        players,
        local_peer: 0,
        local_peer_params: 0,
        host_peer: 0,
        class: 1,
        max_peers: 2,
        max_players: 2,
        difficulty: 0xffff,
        campaign: u32::MAX,
        map: u32::MAX,
        ..Default::default()
    }
}

// ---- levels ----

#[test]
fn levels_are_halo_2s_campaign_in_start_order_with_the_level_select_names() {
    let l = levels_fixture();
    assert_eq!(l.len(), 2, "another campaign's levels are not offered");
    assert_eq!(l[0].stem, "01x_first");
    assert_eq!(l[0].label(), "First Level");
    assert_eq!((l[0].campaign, l[0].map), (1, 10));
    assert_eq!(l[1].name, None);
    assert_eq!(l[1].label(), "02x_second", "no name: the map stem");
}

#[test]
fn level_select_names_are_read_from_the_globals_records() {
    // A synthetic globals tag: +0x178 names one element at 0x100, whose
    // block +8 is two 0xb50-byte records at 0x1000.
    let mut matg = vec![0u8; 0x180];
    matg[0x178..0x17c].copy_from_slice(&1u32.to_le_bytes());
    matg[0x17c..0x180].copy_from_slice(&0x100u32.to_le_bytes());
    let mut mem: HashMap<u32, Vec<u8>> = HashMap::new();
    let mut el = vec![0u8; 0x10];
    el[8..12].copy_from_slice(&2u32.to_le_bytes());
    el[12..16].copy_from_slice(&0x1000u32.to_le_bytes());
    mem.insert(0x100, el);
    let mut recs = vec![0u8; 2 * 0xb50];
    for (i, (c, m, name)) in [(1u32, 10u32, "Alpha Base"), (1, 11, "")]
        .iter()
        .enumerate()
    {
        let r = &mut recs[i * 0xb50..];
        r[0..4].copy_from_slice(&c.to_le_bytes());
        r[4..8].copy_from_slice(&m.to_le_bytes());
        for (k, u) in name.encode_utf16().enumerate() {
            r[0x10 + 2 * k..0x12 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
        // The second language's name must not leak into the first.
        r[0x50..0x52].copy_from_slice(&u16::from(b'X').to_le_bytes());
    }
    mem.insert(0x1000, recs);
    let mut read = |a: u32, n: u32| mem.get(&a).map(|b| b[..n as usize].to_vec());
    let names = level_names(&matg, &mut read);
    assert_eq!(
        names,
        vec![(1, 10, "Alpha Base".to_string()), (1, 11, String::new())]
    );
    // An empty name is no name.
    let l = levels(&[(1, 11, "a\\b\\02_x".into())], &names);
    assert_eq!(l[0].name, None);
    assert!(level_names(&[0; 8], &mut read).is_empty(), "a short tag");
}

/// A fresh directory under the system temp dir, removed on drop.
struct TempDisc(std::path::PathBuf);

impl TempDisc {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("coop-disc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempDisc(d)
    }
}

impl Drop for TempDisc {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_disc_directory_is_listed_not_opened_and_matched_without_case() {
    let d = TempDisc::new("lookup");
    std::fs::create_dir_all(d.0.join("MAPS")).unwrap();
    std::fs::write(d.0.join("MAPS").join("MainMenu.MAP"), b"x").unwrap();
    std::fs::create_dir_all(d.0.join("maps2").join("mainmenu.map")).unwrap();
    assert_eq!(
        dir_disc_file(&d.0, &["maps", "mainmenu.map"]),
        Some(d.0.join("MAPS").join("MainMenu.MAP"))
    );
    assert_eq!(dir_disc_file(&d.0, &["maps", "shared.map"]), None);
    // A directory is not the file, and a file is not a directory on the way.
    assert_eq!(dir_disc_file(&d.0, &["maps2", "mainmenu.map"]), None);
    assert_eq!(dir_disc_file(&d.0, &["maps", "mainmenu.map", "x"]), None);
}

/// The Xbox app's disc is a directory, and its container refuses opening
/// a directory (`opening ...\disc: Access is denied`): the levels come
/// from the directory's `maps/mainmenu.map` file instead.
#[test]
fn the_levels_of_a_disc_directory_come_from_its_mainmenu_file() {
    let d = TempDisc::new("dir");
    let e = format!("{:#}", disc_levels(&d.0).unwrap_err());
    assert!(e.contains("has no maps/mainmenu.map"), "{e}");
    std::fs::create_dir_all(d.0.join("Maps")).unwrap();
    std::fs::write(d.0.join("Maps").join("mainmenu.map"), vec![0u8; 4096]).unwrap();
    let e = format!("{:#}", disc_levels(&d.0).unwrap_err());
    assert!(
        e.contains("parsing mainmenu.map"),
        "read the file, not the directory: {e}"
    );
    assert!(
        !e.contains("opening") && !e.contains("reading the disc"),
        "{e}"
    );
}

/// Against your disc: the levels from a directory holding only the
/// image's `maps/mainmenu.map` match the image's.
#[test]
#[ignore = "reads $H2_DISC (the retail disc image)"]
fn a_disc_directory_gives_the_images_levels() {
    let disc = std::path::PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
    let mut iso =
        xiso::Xiso::open(std::io::BufReader::new(std::fs::File::open(&disc).unwrap())).unwrap();
    let e = iso.find("maps/mainmenu.map").unwrap();
    let bytes = iso.read(&e).unwrap();
    let d = TempDisc::new("image");
    std::fs::create_dir_all(d.0.join("maps")).unwrap();
    std::fs::write(d.0.join("maps").join("mainmenu.map"), bytes).unwrap();
    let from_dir = disc_levels(&d.0).unwrap();
    assert!(from_dir.len() >= 10, "{from_dir:?}");
    assert_eq!(from_dir, disc_levels(&disc).unwrap());
}

#[test]
#[ignore = "reads $H2_DISC (the retail disc image)"]
fn the_discs_co_op_levels_have_ids_and_names() {
    let disc = std::env::var("H2_DISC").expect("H2_DISC");
    let l = disc_levels(Path::new(&disc)).unwrap();
    assert!(l.len() >= 10, "{l:?}");
    assert!(l.iter().all(|l| l.campaign == 1 && l.map != u32::MAX));
    assert!(l.iter().filter(|l| l.name.is_some()).count() >= 10, "{l:?}");
    assert!(l.iter().any(|l| l.stem == "03a_oldmombasa"));
}

// ---- texts ----

#[test]
fn texts_are_slots_written_when_they_change() {
    use guest_layout::memory::GuestMemory as _;
    use guest_layout::testing::FakeGuest;
    let t = Texts::default();
    let id = t.set(3, "Waiting for Bravo");
    assert_eq!(id, TEXT_ID_BASE + 3);
    assert_eq!(t.address(id), None, "no guest memory yet");
    t.base.store(0x5000_0000, Ordering::SeqCst);
    assert_eq!(t.address(id), Some(0x5000_0000 + 3 * TEXT_SLOT_LEN));
    assert_eq!(t.address(TEXT_ID_BASE + TEXT_SLOTS), None);
    assert_eq!(t.address(0x0e00_01ec), None, "a retail id");
    let mut g = FakeGuest::new();
    g.map(0x5000_0000, TEXT_SLOTS * TEXT_SLOT_LEN);
    t.write(&mut g).unwrap();
    let at = 0x5000_0000 + 3 * TEXT_SLOT_LEN;
    let mut b = vec![0; 18];
    g.read(at, &mut b).unwrap();
    assert_eq!(&b, b"Waiting for Bravo\0");
    // Unchanged: not written again; changed: written.
    g.write(at, b"x").unwrap();
    t.set(3, "Waiting for Bravo");
    t.write(&mut g).unwrap();
    g.read(at, &mut b[..1]).unwrap();
    assert_eq!(b[0], b'x');
    t.set(3, "Bravo is in");
    t.write(&mut g).unwrap();
    g.read(at, &mut b[..12]).unwrap();
    assert_eq!(&b[..12], b"Bravo is in\0");
    // Long texts are cut on a character boundary, NUL included.
    let long = "é".repeat(100);
    t.set(4, &long);
    let got = t.get(4).unwrap();
    assert!(got.len() < TEXT_SLOT_LEN as usize && got.chars().all(|c| c == 'é'));
}

// ---- the host ----

#[test]
fn the_host_arms_co_op_and_advertises_its_campaign_and_skulls() {
    let live = Arc::new(FakeLive::default());
    let (lobby, arm) = lobby_with(Arc::clone(&live));
    assert!(!arm.active());
    assert!(!lobby.in_coop());
    lobby.set_profiles(vec!["Chief".into()]);
    assert_eq!(lobby.open_from_menu(), SCREEN);
    assert_eq!(lobby.role(), Role::Host);
    assert!(arm.active());
    let want = arm.want().unwrap();
    assert_eq!((want.campaign, want.map, want.difficulty), (1, 10, 0));
    let campaign = Campaign {
        level: "01x_first".into(),
        name: Some("First Level".into()),
        difficulty: 0,
    };
    assert_eq!(
        live.coop.lock().unwrap().last().cloned(),
        Some((Some(campaign), Some(vec![0, 4])))
    );
    // Level and difficulty follow the presses, to the HLE and the hooks.
    assert_eq!(lobby.step_level(), "coop-live: level 02x_second");
    assert_eq!(lobby.step_difficulty(), "coop-live: difficulty Normal");
    let last = live.coop.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.0.unwrap().difficulty, 1);
    assert_eq!(arm.want().unwrap().map, 11);
    assert_eq!(lobby.step_level(), "coop-live: level First Level", "wraps");
    for _ in 0..3 {
        lobby.step_difficulty();
    }
    assert_eq!(lobby.difficulty(), 0, "wraps after Legendary");
    // Another Live row: back to a custom game, hooks off.
    lobby.leave();
    assert_eq!(lobby.role(), Role::Idle);
    assert!(!arm.active());
    assert_eq!(
        live.coop.lock().unwrap().last().cloned(),
        Some((None, None))
    );
    // Leaving when not hosting does nothing.
    let n = live.coop.lock().unwrap().len();
    lobby.leave();
    assert_eq!(live.coop.lock().unwrap().len(), n);
}

#[test]
fn start_waits_for_two_consoles_and_the_status_says_who() {
    let live = Arc::new(FakeLive {
        friends: vec![friend(2, "Bravo", true), friend(3, "Charlie", false)],
        ..Default::default()
    });
    let (lobby, _) = lobby_with(Arc::clone(&live));
    lobby.open_from_menu();
    let rows = HostRows(Arc::clone(&lobby));
    assert_eq!(rows.count(), 6, "fits the list's six item widgets");
    assert_eq!(lobby.status(), "Invite a friend");
    assert_eq!(
        rows.press(ROW_START),
        Chosen::Stepped("coop-live: not ready: no lobby yet".into())
    );
    assert_eq!(rows.press(ROW_INVITE), Chosen::Open(FRIENDS));
    // The friends screen: online first, a press invites.
    let friends = FriendRows(Arc::clone(&lobby));
    assert_eq!(friends.count(), 2);
    assert_eq!(
        friends.row_texts(0),
        Some(("Bravo".into(), "Online".into()))
    );
    assert_eq!(
        friends.press(0),
        Chosen::Stepped("coop-live: invited Bravo to session 8000000000000001".into())
    );
    assert_eq!(*live.invites.lock().unwrap(), vec![Xuid(2)]);
    assert_eq!(friends.row_texts(0).unwrap().1, "Invited");
    // An offline friend's row has no A action, and the lobby refuses them too.
    assert_eq!(
        friends.press(1),
        Chosen::Nothing,
        "an offline friend is not invited"
    );
    assert_eq!(
        lobby.invite(1),
        "coop-live: Charlie is offline; not invited"
    );
    assert_eq!(live.invites.lock().unwrap().len(), 1);
    assert_eq!(lobby.status(), "Waiting for Bravo");
    lobby.observe(Some(session(1, 1)));
    assert_eq!(
        rows.press(ROW_START),
        Chosen::Stepped("coop-live: not ready: waiting for a second console".into())
    );
    assert_eq!(rows.row_texts(ROW_START).unwrap().1, "Not ready");
    lobby.observe(Some(session(2, 3)));
    assert_eq!(lobby.status(), "3 players; co-op takes 2");
    lobby.observe(Some(session(2, 2)));
    assert_eq!(lobby.status(), "Bravo is in; ready");
    assert_eq!(rows.press(ROW_START), Chosen::Act);
    assert_eq!(rows.row_texts(ROW_START).unwrap().1, "Ready");
    // The lobby holds the level once the setters took.
    let mut held = session(2, 2);
    held.campaign = 1;
    held.map = 10;
    held.difficulty = 0;
    lobby.observe(Some(held));
    assert!(lobby.checks().holds);
    assert_eq!(lobby.status(), "Bravo is in; the lobby holds the level");
    // A session outside the lobby/game states is no lobby.
    lobby.observe(Some(coop_probe::Session { state: 0, ..held }));
    assert_eq!(lobby.session(), None);
}

#[test]
fn the_host_rows_have_labels_and_values() {
    let (lobby, _) = lobby_with(Arc::default());
    let rows = HostRows(Arc::clone(&lobby));
    lobby.set_text_base(0x5000_0000);
    let title = rows.title();
    assert_eq!(
        lobby.texts.get(title - TEXT_ID_BASE).unwrap(),
        "Co-op Campaign"
    );
    let got: Vec<_> = (0..6).map(|r| rows.row_texts(r).unwrap()).collect();
    assert_eq!(got[0], ("Level".into(), "First Level".into()));
    assert_eq!(got[1], ("Difficulty".into(), "Easy".into()));
    assert_eq!(got[2].0, "Invite a Friend");
    assert_eq!(got[3], ("Status".into(), "Invite a friend".into()));
    assert_eq!(got[4], ("Ping".into(), "-".into()), "no guest yet");
    assert_eq!(got[5].0, "Start");
    assert_eq!(rows.row_texts(6), None);
    // Distinct slots per row and screen.
    let ids: Vec<u32> = (0..6)
        .flat_map(|r| rows.texts(r))
        .map(|(_, id)| id)
        .collect();
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len());
    assert!(ids.iter().all(|i| lobby.texts.address(*i).is_some()));
}

// ---- the guest ----

fn coop_session(skulls: Option<Vec<u8>>) -> live_proto::session::Session {
    use live_proto::session::{Endpoint, NewSession, Slots};
    live_proto::session::Session {
        session_id: "8000000000000001".into(),
        host: "synthetic".into(),
        host_xuid: Xuid(1),
        host_gamertag: "Alpha".into(),
        xnkid: "8000000000000001".into(),
        endpoint: Endpoint {
            addr: "192.168.1.2".parse().unwrap(),
            port: 3074,
            source: "declared".into(),
        },
        advertisement: NewSession {
            addr: None,
            port: 3074,
            host_address: vec![0; 36],
            slots: Slots::default(),
            attributes: Default::default(),
            flags: "coop".into(),
            hopper_id: None,
            map: None,
            name: None,
            language: None,
            visibility: Default::default(),
            map_id: None,
            skulls,
            campaign: Some(Campaign {
                level: "03a_oldmombasa".into(),
                name: Some("Outskirts".into()),
                difficulty: 2,
            }),
            ping_ms: None,
            variant_ext: None,
            rules: None,
            firefight: None,
            browser: None,
        },
        matchmaking: None,
        since: String::new(),
        last_activity: 0,
        state: "open".into(),
        players: Default::default(),
        lan: None,
    }
}

#[test]
fn a_guest_in_a_coop_session_arms_and_opens_the_join_screen_once() {
    let (lobby, arm) = lobby_with(Arc::default());
    // The engine's (0, 4) and ours (16): all of them carry.
    let j = live_hle::Joined::Known {
        session: Box::new(coop_session(Some(vec![0, 4, 16]))),
        refreshed: true,
    };
    assert!(lobby.joined(&j));
    assert!(arm.active());
    assert_eq!(arm.want(), None, "a guest sets no level");
    let g = lobby.guest().unwrap();
    assert_eq!(
        (g.host.as_str(), g.skulls.clone()),
        ("Alpha", vec![0, 4, 16])
    );
    assert_eq!(ours_of(&g.skulls), vec![16], "ours go to skulls_custom");
    assert!(lobby.in_coop());
    assert!(lobby.take_open_join());
    assert!(!lobby.take_open_join(), "once");
    assert!(!lobby.joined(&j), "the same session again is not news");
    assert!(!lobby.take_open_join());
    // The Co-op Campaign row opens the join screen for a guest.
    assert_eq!(lobby.open_from_menu(), JOIN);
    let rows = JoinRows(Arc::clone(&lobby));
    assert_eq!(rows.count(), 6);
    assert_eq!(rows.row_texts(0).unwrap().1, "Alpha");
    assert_eq!(rows.row_texts(1).unwrap().1, "Outskirts");
    assert_eq!(rows.row_texts(2).unwrap().1, "Heroic");
    assert_eq!(rows.row_texts(3).unwrap().1, "3 from the host");
    assert_eq!(rows.row_texts(4).unwrap().1, "No lobby");
    let mut s = session(2, 2);
    s.host_peer = 1;
    lobby.observe(Some(s));
    assert_eq!(lobby.handshake(), "Waiting for the host's campaign");
    s.difficulty = 2;
    s.campaign = 1;
    lobby.observe(Some(s));
    assert_eq!(lobby.handshake(), "OK: the lobby holds the host's campaign");
    s.difficulty = 1;
    lobby.observe(Some(s));
    assert_eq!(lobby.handshake(), "The lobby's difficulty differs");
    assert_eq!(rows.press(JOIN_READY), Chosen::Act);
    // Leaving the session: back to retail.
    assert!(!lobby.joined(&live_hle::Joined::NotJoined));
    assert_eq!(lobby.role(), Role::Idle);
    assert!(!arm.active());
}

#[test]
fn a_custom_game_or_the_host_itself_is_no_co_op_guest() {
    let (lobby, arm) = lobby_with(Arc::default());
    let mut custom = coop_session(None);
    custom.advertisement.flags = "custom".into();
    custom.advertisement.campaign = None;
    assert!(!lobby.joined(&live_hle::Joined::Known {
        session: Box::new(custom),
        refreshed: true
    }));
    assert!(!lobby.joined(&live_hle::Joined::Unknown {
        session: "x".into()
    }));
    assert_eq!(lobby.role(), Role::Idle);
    assert!(!arm.active());
    lobby.open_from_menu();
    let j = live_hle::Joined::Known {
        session: Box::new(coop_session(None)),
        refreshed: true,
    };
    assert!(!lobby.joined(&j), "a host is not its own guest");
    assert_eq!(lobby.role(), Role::Host);
}

// ---- the lobby line and the skulls ----

#[test]
fn the_retail_co_op_line_is_ours_only_in_a_co_op_session() {
    let (lobby, _) = lobby_with(Arc::default());
    lobby.set_text_base(0x5000_0000);
    assert_eq!(
        lobby.string(COOP_TAKES_TWO),
        None,
        "split screen: the game's text"
    );
    lobby.open_from_menu();
    let at = lobby.string(COOP_TAKES_TWO).unwrap();
    assert_eq!(at, lobby.texts.address(Texts::id(LINE_SLOT)).unwrap());
    assert_eq!(lobby.texts.get(LINE_SLOT).unwrap(), LOBBY_LINE);
    assert!(LOBBY_LINE.contains("one per console"));
    for line in LOBBY_LINE.split("\r\n") {
        assert!(line.len() <= 52, "{line:?}");
    }
    lobby.leave();
    assert_eq!(lobby.string(COOP_TAKES_TWO), None);
    assert_eq!(lobby.string(0x0e00_01ed), None, "another retail id");
}

/// Every line the lobby can show in place of the retail line (co-op
/// needs two players on one console) fits the lobby's 52
/// characters a line, and none says "single".
#[test]
fn every_co_op_lobby_line_fits_and_none_says_single() {
    let mut lines = vec![
        LOBBY_LINE.to_string(),
        INVITE_FIRST.into(),
        SOLO_LINE.into(),
        GUEST_LINE.into(),
        ONE_PER_CONSOLE.into(),
        lobby_line(4),
        lobby_line(16),
    ];
    lines.push(line_for(&Role::Host, Some(&session(5, 5)), 4));
    for text in &lines {
        assert!(text.len() < 128, "{text:?} fits a text slot");
        assert!(!text.to_lowercase().contains("single"), "{text:?}");
        for line in text.split("\r\n") {
            assert!(line.len() <= 52, "{line:?}");
        }
    }
}

#[test]
fn the_lobby_line_is_ours_from_the_co_op_row_and_says_invite_first_while_alone() {
    // open_retail asks for co-op mode (process-wide).
    let _turn = MODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (lobby, _) = lobby_with(Arc::default());
    lobby.set_text_base(0x5000_0000);
    // At the main menu, then in a split-screen lobby: the game's text.
    assert_eq!(lobby.string(COOP_TAKES_TWO), None);
    lobby.observe(Some(coop_probe::Session {
        class: 0,
        ..session(1, 2)
    }));
    assert_eq!(lobby.string(COOP_TAKES_TWO), None, "split screen");
    lobby.observe(None);
    // The Co-op Campaign row: ours at once, before any session is read.
    lobby.open_retail();
    let at = lobby.string(COOP_TAKES_TWO).unwrap();
    assert_eq!(at, lobby.texts.address(Texts::id(LINE_SLOT)).unwrap());
    assert_eq!(
        lobby.texts.get(LINE_SLOT).unwrap(),
        lobby_line(lobby.max_players())
    );
    // The squad session, alone: Start Game starts, friends join in
    // progress (13.4); without the solo start it would say invite first.
    lobby.observe(Some(session(1, 1)));
    lobby.refresh_line();
    assert!(lobby.solo_start());
    assert_eq!(lobby.texts.get(LINE_SLOT).unwrap(), SOLO_LINE);
    assert_eq!(
        host_line(&Role::Host, Some(&session(1, 1)), 2, false),
        INVITE_FIRST
    );
    // A friend in, two players on one console between them.
    lobby.observe(Some(session(2, 3)));
    lobby.refresh_line();
    assert_eq!(lobby.texts.get(LINE_SLOT).unwrap(), ONE_PER_CONSOLE);
    // Too many consoles for the limit.
    lobby.observe(Some(session(3, 3)));
    lobby.refresh_line();
    assert_eq!(
        lobby.texts.get(LINE_SLOT).unwrap(),
        "Co-op over Xbox Live takes up to 2 consoles;\r\n3 are in this lobby."
    );
    // Another Live row: back to the game's text once out of the session.
    lobby.leave();
    lobby.observe(None);
    assert_eq!(lobby.string(COOP_TAKES_TWO), None);
    drop_coop_mode();
}

#[test]
fn a_friend_in_the_hosts_lobby_reads_ours_before_it_knows_it_is_a_co_op_guest() {
    let (lobby, _) = lobby_with(Arc::default());
    lobby.set_text_base(0x5000_0000);
    assert_eq!(lobby.role(), Role::Idle);
    // The accepted invitation put this console in the host's lobby: a
    // networked session it does not host.
    lobby.observe(Some(coop_probe::Session {
        host_peer: 1,
        ..session(2, 2)
    }));
    assert!(lobby.string(COOP_TAKES_TWO).is_some());
    lobby.refresh_line();
    assert_eq!(lobby.texts.get(LINE_SLOT).unwrap(), GUEST_LINE);
    // Without Live (a system link lobby), the game's text.
    let arm = Arm::session();
    let offline = Lobby::new(levels_fixture(), Some(arm), None, Box::new(|_| Vec::new()));
    offline.set_text_base(0x5000_0000);
    offline.observe(Some(session(2, 2)));
    assert_eq!(offline.string(COOP_TAKES_TWO), None);
}

#[test]
fn a_co_op_guest_starts_with_exactly_the_hosts_skulls() {
    let (on, line) =
        crate::skulls::coop_start(Some(crate::skulls::MODE_CAMPAIGN), &[0, 4, 99]).unwrap();
    assert!(on[0] && on[4]);
    assert_eq!(
        on.iter().filter(|b| **b).count(),
        2,
        "an index past the array is ignored"
    );
    assert!(
        line.starts_with("skulls: co-op campaign start with the host's skulls: "),
        "{line}"
    );
    let (none, line) = crate::skulls::coop_start(Some(crate::skulls::MODE_CAMPAIGN), &[]).unwrap();
    assert!(none.iter().all(|b| !b));
    assert!(line.ends_with("none on"));
    assert!(
        crate::skulls::coop_start(Some(2), &[0]).is_none(),
        "multiplayer"
    );
}

#[test]
fn the_host_advertises_its_own_skulls_after_the_engines() {
    let mut on = [false; crate::skulls::COUNT];
    on[4] = true;
    on[0] = true;
    assert_eq!(
        host_campaign_skulls(Some(on), &[16, 20]),
        vec![0, 4, 16, 20]
    );
    assert_eq!(host_campaign_skulls(None, &[17]), vec![17], "ours alone");
    assert_eq!(host_campaign_skulls(None, &[]), Vec::<u8>::new());
    // The wire takes them: a co-op session may carry any skull.
    live_proto::skulls::validate(&host_campaign_skulls(Some(on), &[16, 20])).unwrap();
    // And a guest hands back exactly ours.
    assert_eq!(ours_of(&[0, 4, 16, 20, 99]), vec![16, 20]);
}

// ---- ping ----

#[test]
fn a_guest_pings_its_host_and_reports_its_last_round_trip() {
    let live = Arc::new(FakeLive {
        echo: Some(42),
        ..Default::default()
    });
    let (lobby, _) = lobby_with(Arc::clone(&live));
    lobby.ping_once();
    assert!(live.pings.lock().unwrap().is_empty(), "idle: no ping");
    assert_eq!(lobby.ping_text(), "-");
    let j = live_hle::Joined::Known {
        session: Box::new(coop_session(None)),
        refreshed: true,
    };
    assert!(lobby.joined(&j));
    let host = lobby.guest().unwrap().endpoint.unwrap();
    lobby.ping_once();
    lobby.ping_once();
    assert_eq!(
        *live.pings.lock().unwrap(),
        vec![(host, None), (host, Some(42))],
        "the second ping carries the first's round trip"
    );
    assert_eq!(lobby.ping(), Some(42));
    let rows = JoinRows(Arc::clone(&lobby));
    assert_eq!(rows.row_texts(0).unwrap().1, "Alpha, 42 ms");
    assert!(
        live.advertised.lock().unwrap().is_empty(),
        "a guest advertises nothing"
    );
}

#[test]
fn a_host_shows_and_advertises_its_guests_report() {
    let live = Arc::new(FakeLive {
        reported: Some(57),
        ..Default::default()
    });
    let (lobby, _) = lobby_with(Arc::clone(&live));
    lobby.open_from_menu();
    assert_eq!(lobby.role(), Role::Host);
    lobby.ping_once();
    assert_eq!(*live.advertised.lock().unwrap(), vec![Some(57)]);
    let rows = HostRows(Arc::clone(&lobby));
    assert_eq!(
        rows.row_texts(ROW_PING).unwrap(),
        ("Ping".to_owned(), "57 ms".to_owned())
    );
    assert!(live.pings.lock().unwrap().is_empty(), "a host pings nobody");
    lobby.leave();
    lobby.ping_once();
    assert_eq!(lobby.ping_text(), "-", "idle again");
}

// ---- the Live menu ----

#[test]
fn the_live_menu_puts_co_op_above_custom_games_and_mods_last() {
    use crate::custom_games::*;

    let with_coop = [
        (Row::Retail, Pane::Retail),
        (Row::Retail, Pane::Retail),
        (Row::Clankermatch, Pane::Show(CLANKERMATCH_PANE)),
        (Row::Coop, Pane::Show(COOP_PANE)),
        (Row::CustomGames, Pane::Show(CUSTOM_GAMES_PANE)),
        (Row::Forge, Pane::Show(FORGE_PANE)),
        (Row::Browser, Pane::Show(BROWSER_PANE)),
        (Row::Mods, Pane::Show(3)),
    ];
    let without_coop: Vec<_> = with_coop
        .iter()
        .copied()
        .filter(|(r, _)| *r != Row::Coop)
        .collect();

    for (coop, count, expected) in [
        (true, 8, with_coop.as_slice()),
        (false, 7, without_coop.as_slice()),
    ] {
        let menu = Menu { coop, ..Menu::ALL };
        assert_eq!(menu.rows(), count);
        assert_eq!(menu.count(LIST_NAME, VANILLA_ROWS), count);
        assert_eq!(menu.count(LIST_NAME, count), count, "not twice");
        assert_eq!(menu.visible(), count.min(VISIBLE_ITEMS), "up to eight show; more would scroll");
        for (row, &(kind, pane)) in expected.iter().enumerate() {
            assert_eq!(menu.row_kind(row as i16), kind, "co-op {coop}, row {row}");
            assert_eq!(
                menu.pane_of(row as i16, ALL_PANES),
                pane,
                "co-op {coop}, row {row}"
            );
        }
        assert_eq!(menu.row_kind(count as i16), Row::Retail, "past the last row");
    }

    let menu = Menu::ALL;
    assert_eq!(COOP_ROW + 1, CUSTOM_GAMES_ROW, "co-op directly above Custom Games");
    assert_eq!(menu.pane_of(COOP_ROW, 6), Pane::Keep, "no co-op pane");
    assert_eq!(menu.pane_of(COOP_ROW, 4), Pane::Keep);
    let ids: Vec<u32> = strings().iter().map(|(i, _)| *i).collect();
    assert!(ids.contains(&COOP_TEXT_ID) && ids.contains(&COOP_HELP_ID));
    assert!(
        COOP_TEXT.len() <= TEXT.len(),
        "fits where the browser's label fits"
    );
}

#[test]
fn the_co_op_hooks_are_on_the_plan_and_off_until_armed() {
    let rt = runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::dispatch::Dispatch::default(),
    );
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    let arm = coop_probe::install_live(&rt, &mut r, Path::new("/nonexistent"))
        .unwrap()
        .unwrap();
    assert!(!arm.active() && !arm.is_forced());
    let text = r.build(&hooks::Base).unwrap().text();
    assert!(text.contains(coop_probe::LIVE_ID), "{text}");
    for p in coop_probe::HOOKED {
        assert!(text.contains(p.name), "{} missing:\n{text}", p.name);
    }
    assert!(text.contains("host lobby setup"), "{text}");
    // H2_FEATURES=-coop-live turns them off.
    let mut r = hooks::Registry::new(hooks::Switches::parse("-coop-live").unwrap());
    assert!(
        coop_probe::install_live(&rt, &mut r, Path::new("/x"))
            .unwrap()
            .is_none()
    );
    // Armed and disarmed.
    arm.arm(None);
    assert!(arm.active() && arm.want().is_none());
    arm.disarm();
    assert!(!arm.active());
    // The probe's arm stays on.
    let forced = Arm::forced(None);
    forced.disarm();
    assert!(forced.active());
}

#[test]
fn the_screens_and_the_hooks_share_the_plan() {
    let rt = runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::dispatch::Dispatch::default(),
    );
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    let arm = coop_probe::install_live(&rt, &mut r, Path::new("/nonexistent"))
        .unwrap()
        .unwrap();
    let (lobby, _) = lobby_with(Arc::default());
    drop(arm);
    r.add(&CoopScreens {
        lobby: Arc::clone(&lobby),
    })
    .unwrap();
    for (id, screen, rows) in [
        (
            "coop-screen",
            SCREEN,
            Arc::new(HostRows(Arc::clone(&lobby))) as Arc<dyn Rows>,
        ),
        (
            "coop-friends",
            FRIENDS,
            Arc::new(FriendRows(Arc::clone(&lobby))),
        ),
        ("coop-join", JOIN, Arc::new(JoinRows(Arc::clone(&lobby)))),
    ] {
        r.add(&ListClass::with_rows(id, screen, rows)).unwrap();
    }
    r.add(&lockstep::TickObserver(lockstep::Recorder::new(None)))
        .unwrap();
    // Our skulls are on in every boot with settings, with a mutating hook
    // on the same tick.
    let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
    r.add(&crate::skulls_custom::SkullsCustom(Arc::new(
        crate::skulls_custom::Custom::new(store, None, None, false),
    )))
    .unwrap();
    let plan = r.build(&hooks::Base).unwrap();
    let text = plan.text();
    assert!(
        text.contains(FEATURE)
            && text.contains("co-op mode for the co-op row")
            && text.contains("co-op over 2 to N consoles"),
        "{text}"
    );
    // The join screen opens itself only with our list screens.
    assert_eq!(
        text.contains("open the co-op join screen"),
        list_ui(),
        "{text}"
    );
    assert!(
        lobby.texts.address(Texts::id(LINE_SLOT)).is_some(),
        "the texts have guest memory"
    );
    assert!(crate::list_class::loader_of(JOIN).is_some());
}

/// Opening the co-op screen showed the community variants' names: the
/// texts' ids shared their index (`0xffe0xx`), and their lookup ignored the
/// top byte then. The co-op ids stay clear on the low 24 bits too; every
/// family against every other is `crate::string_ids`.
#[test]
fn the_co_op_text_ids_are_nobody_elses() {
    use crate::{campaign_menu, community_variants, layout_glue, skulls_menu};
    let low = |id: u32| id & 0x00ff_ffff;
    // The other private ranges, each as wide as its owner could take.
    let others = [
        ("community variants", 0x00ff_e000..0x00ff_e100),
        (
            "layouts",
            layout_glue::ID_BASE..layout_glue::ID_BASE + 0x100,
        ),
        (
            "campaign picker",
            campaign_menu::ID_BASE..campaign_menu::ID_BASE + 0x100,
        ),
        (
            "skulls",
            skulls_menu::SKULLS_ID_BASE..skulls_menu::SKULLS_ID_BASE + 0x100,
        ),
    ];
    for slot in 0..TEXT_SLOTS {
        let id = Texts::id(slot);
        assert!(
            !community_variants::is_ours(id) && community_variants::text(id).is_none(),
            "{id:#x} is a community variant's"
        );
        assert!(
            !layout_glue::is_ours(id),
            "{id:#x} is in the layouts' range"
        );
        for (who, r) in &others {
            assert!(
                !r.contains(&id) && !(low(r.start)..=low(r.end - 1)).contains(&low(id)),
                "{id:#x} collides with the {who}"
            );
        }
    }
    // Every screen's texts resolve through our source, to our text.
    let (lobby, _) = lobby_with(Arc::default());
    lobby.set_text_base(0x00a0_0000);
    let title = HostRows(Arc::clone(&lobby)).title();
    assert_eq!(lobby.string(title), Some(0x00a0_0000));
    assert_eq!(
        lobby.texts.get(title - TEXT_ID_BASE).as_deref(),
        Some("Co-op Campaign")
    );
}

#[test]
fn an_opened_screen_describes_its_rows_for_the_log() {
    let (lobby, _) = lobby_with(Arc::default());
    let rows = HostRows(Arc::clone(&lobby));
    let d = rows.describe(rows.count()).unwrap();
    assert!(
        d.starts_with("title \"Co-op Campaign\" (0x20ffd000); rows: [0] "),
        "{d}"
    );
    assert_eq!(d.matches(" = ").count(), HOST_ROWS, "{d}");
}

// ---- the retail lobby ----

/// The pending co-op mode is one per run: its tests take turns.
static MODE_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn the_co_op_rows_press_turns_the_next_custom_game_mode_into_co_op_once() {
    let _turn = MODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    drop_coop_mode();
    assert_eq!(lobby_mode(MODE_CUSTOM, now), MODE_CUSTOM, "nothing pending");
    want_coop_mode();
    assert_eq!(
        lobby_mode(MODE_COOP, now),
        MODE_COOP,
        "another mode leaves it pending"
    );
    assert_eq!(lobby_mode(MODE_CUSTOM, now), MODE_COOP);
    assert_eq!(lobby_mode(MODE_CUSTOM, now), MODE_CUSTOM, "once");
    // A press whose session never came: a later custom game stays one.
    want_coop_mode();
    let late = now + COOP_MODE_WINDOW + std::time::Duration::from_secs(1);
    assert_eq!(lobby_mode(MODE_CUSTOM, late), MODE_CUSTOM);
    // Another row's press drops it.
    want_coop_mode();
    drop_coop_mode();
    assert_eq!(lobby_mode(MODE_CUSTOM, now), MODE_CUSTOM);
}

#[test]
fn the_lobbys_co_op_check_is_lifted_only_for_two_consoles_in_a_networked_campaign() {
    let mut s = session(2, 2);
    s.difficulty = 1;
    s.class = 2;
    assert!(lift_coop_check(START_COOP_OR_TEAMS, Some(&s), 2));
    assert!(!lift_coop_check(START_READY, Some(&s), 2), "only state 7");
    assert!(!lift_coop_check(START_COOP_OR_TEAMS, None, 2));
    let mut local = s;
    local.class = 0;
    assert!(
        !lift_coop_check(START_COOP_OR_TEAMS, Some(&local), 2),
        "split screen stays retail"
    );
    let mut mp = s;
    mp.difficulty = coop_probe::NO_DIFFICULTY;
    assert!(
        !lift_coop_check(START_COOP_OR_TEAMS, Some(&mp), 2),
        "teams in multiplayer"
    );
    let three = session(3, 3);
    assert!(!lift_coop_check(START_COOP_OR_TEAMS, Some(&three), 2));
    // The retail tail after a passing check.
    assert_eq!(tail_state(0, false), START_COUNTDOWN);
    assert_eq!(tail_state(-1, true), START_READY);
    assert_eq!(tail_state(-1, false), START_NOT_LOADED);
}

#[test]
fn the_lobby_check_stays_retail_for_a_playerless_multiplayer_lobby() {
    // Four phantoms and a host with no player: 5 peers, 4 players, custom game.
    let mut s = session(5, 4);
    s.class = 1;
    s.max_peers = 16;
    s.max_players = 4;
    for state in [START_COOP_OR_TEAMS, START_READY, START_NOT_LOADED] {
        for max in [coop_probe::MAX_PLAYERS, 4, coop_probe::CEILING] {
            assert!(!lift_coop_check(state, Some(&s), max), "state {state}, max {max}");
        }
    }
}

#[test]
fn a_larger_co_op_lifts_the_lobby_check_for_one_player_per_console_up_to_its_limit() {
    let net = |peers, players| {
        let mut s = session(peers, players);
        s.difficulty = 1;
        s.class = 2;
        s
    };
    for n in 2..=4 {
        assert!(
            lift_coop_check(START_COOP_OR_TEAMS, Some(&net(n, n)), 4),
            "{n} consoles"
        );
    }
    assert!(
        !lift_coop_check(START_COOP_OR_TEAMS, Some(&net(5, 5)), 4),
        "past the limit"
    );
    assert!(lift_coop_check(
        START_COOP_OR_TEAMS,
        Some(&net(16, 16)),
        coop_probe::CEILING
    ));
    assert!(
        !lift_coop_check(START_COOP_OR_TEAMS, Some(&net(3, 4)), 4),
        "two players on one console: not stage 1"
    );
    assert!(
        !lift_coop_check(START_COOP_OR_TEAMS, Some(&net(1, 1)), 4),
        "one console"
    );
    assert!(
        !lift_coop_check(START_COOP_OR_TEAMS, Some(&net(3, 3)), 2),
        "the default stays at two"
    );
}

#[test]
fn a_larger_co_op_host_waits_for_up_to_its_limit_and_says_so() {
    let live = Arc::new(FakeLive::default());
    let (lobby, arm) = lobby_with(live);
    arm.set_max_players(4);
    lobby.open_from_menu();
    lobby.observe(Some(session(3, 3)));
    assert_eq!(lobby.checks().blocker(), None, "three of four");
    lobby.observe(Some(session(4, 4)));
    assert_eq!(lobby.checks().blocker(), None);
    lobby.observe(Some(session(5, 5)));
    assert_eq!(
        lobby.checks().blocker().as_deref(),
        Some("5 consoles in the lobby; co-op takes up to 4")
    );
    lobby.observe(Some(session(4, 5)));
    assert_eq!(
        lobby.checks().blocker().as_deref(),
        Some("5 players; co-op takes up to 4")
    );
}

#[test]
fn the_start_state_line_names_the_consoles_once() {
    let (lobby, _) = lobby_with(Arc::default());
    assert_eq!(
        lobby
            .start_state_line(START_COOP_OR_TEAMS, START_READY, 4)
            .as_deref(),
        Some("coop-live: lobby start state 7 (co-op wants one console) answered 9 for 4 consoles")
    );
    assert_eq!(
        lobby.start_state_line(START_COOP_OR_TEAMS, START_READY, 4),
        None,
        "once"
    );
    assert_eq!(
        lobby
            .start_state_line(START_NOT_LOADED, START_NOT_LOADED, 4)
            .as_deref(),
        Some("coop-live: lobby start state 8")
    );
}

/// docs/live/live-coop-join.md 13.4: a Live co-op host alone in a
/// networked campaign lobby starts; nothing else that one console does.
#[test]
fn a_host_alone_lifts_the_co_op_check_with_the_solo_start() {
    let net = |peers, players| {
        let mut s = session(peers, players);
        s.difficulty = 1;
        s.class = 2;
        s
    };
    let alone = net(1, 1);
    assert!(lift_solo_start(START_COOP_OR_TEAMS, Some(&alone), true));
    assert!(
        !lift_solo_start(START_COOP_OR_TEAMS, Some(&alone), false),
        "join in progress off: invite first"
    );
    assert!(!lift_solo_start(START_READY, Some(&alone), true), "only state 7");
    assert!(!lift_solo_start(START_COOP_OR_TEAMS, None, true));
    let mut local = alone;
    local.class = 0;
    assert!(!lift_solo_start(START_COOP_OR_TEAMS, Some(&local), true), "split screen");
    let mut mp = alone;
    mp.difficulty = coop_probe::NO_DIFFICULTY;
    assert!(!lift_solo_start(START_COOP_OR_TEAMS, Some(&mp), true), "multiplayer");
    let mut guest = alone;
    guest.local_peer = 1;
    assert!(!lift_solo_start(START_COOP_OR_TEAMS, Some(&guest), true), "not the host");
    assert!(
        !lift_solo_start(START_COOP_OR_TEAMS, Some(&net(1, 2)), true),
        "two local players: retail's own check"
    );
    assert!(
        !lift_solo_start(START_COOP_OR_TEAMS, Some(&net(2, 2)), true),
        "two consoles: lift_coop_check"
    );
    // The line, and the lobby's answer for one console.
    let (lobby, _) = lobby_with(Arc::default());
    assert!(lobby.solo_start());
    assert_eq!(
        lobby
            .start_state_line(START_COOP_OR_TEAMS, START_READY, 1)
            .as_deref(),
        Some(
            "coop-live: lobby start state 7 (co-op wants one console) answered 9 for 1 console (alone; friends join in progress)"
        )
    );
    // The host alone reads the solo line; a guest, or two consoles, not.
    assert_eq!(host_line(&Role::Host, Some(&net(1, 1)), 2, true), SOLO_LINE);
    assert_eq!(host_line(&Role::Idle, Some(&guest), 2, true), GUEST_LINE);
    assert_eq!(host_line(&Role::Host, Some(&net(2, 2)), 2, true), LOBBY_LINE);
    // Without hooks, no solo start.
    let none: HostSkulls = Box::new(|_| Vec::new());
    let bare = Lobby::new(levels_fixture(), None, None, none);
    assert!(!bare.solo_start());
}

#[test]
fn the_lobby_line_says_the_limit_and_fits() {
    assert_eq!(lobby_line(2), LOBBY_LINE);
    for n in [3, 4, 8, 16] {
        let line = lobby_line(n);
        assert!(line.contains(&format!("up to {n} players")), "{line}");
        for l in line.split("\r\n") {
            assert!(l.len() <= 52, "{l:?}");
        }
    }
    let live = Arc::new(FakeLive::default());
    let (lobby, arm) = lobby_with(live);
    arm.set_max_players(8);
    lobby.set_text_base(0x2000_0000);
    assert_eq!(lobby.texts.get(LINE_SLOT).unwrap(), lobby_line(8));
}

#[test]
fn the_retail_host_advertises_what_the_lobby_holds() {
    let _turn = MODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let live = Arc::new(FakeLive::default());
    let (lobby, arm) = lobby_with(Arc::clone(&live));
    lobby.open_retail();
    drop_coop_mode();
    assert_eq!(lobby.role(), Role::Host);
    assert!(
        arm.active() && arm.want().is_none(),
        "armed; the lobby sets the level"
    );
    // No session yet: nothing advertised.
    lobby.sync_retail();
    assert!(live.coop.lock().unwrap().is_empty());
    let level = lobby.levels()[1].clone();
    let mut s = session(1, 1);
    s.campaign = level.campaign;
    s.map = level.map;
    s.difficulty = 2;
    lobby.observe(Some(s));
    lobby.sync_retail();
    lobby.sync_retail();
    let calls = live.coop.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "once per change: {calls:?}");
    let c = calls[0].0.clone().unwrap();
    assert_eq!((c.level.as_str(), c.difficulty), (level.stem.as_str(), 2));
    // The lobby's difficulty screen moves it.
    s.difficulty = 3;
    lobby.observe(Some(s));
    lobby.sync_retail();
    assert_eq!(live.coop.lock().unwrap().len(), 2);
}

/// The game's own co-op lobby on Live, drawn from the disc (pane 1 of the
/// pregame lobby is its co-op pane): the host with a Live guest, and the
/// guest's view, plus the lobby's level and difficulty screens. The game's
/// code fills the formats (`\u{e416}` a gamertag, `\u{e426}` the level and
/// so on); the texts here stand in for those. Skipped without a disc;
/// drawn into `H2_UI_PREVIEW_DIR` when it is set.
#[test]
fn the_retail_co_op_lobby_on_live_drawn_from_the_disc() {
    let iso = std::env::var_os("H2_DISC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso")
        });
    if !iso.exists() {
        eprintln!("no disc at {}; skipped", iso.display());
        return;
    }
    use crate::ui_kit::preview::{Disc, Retail};
    let disc = Disc::open(&iso).unwrap();
    const LOBBY: &str = r"ui\screens\game_shell\pregame_lobby\pregame_lobby";
    let players = ["Alpha".to_owned(), "Bravo".to_owned()];
    // The co-op pane's texts as the game fills them for a Live co-op lobby.
    let common = |id: u32| -> Option<String> {
        Some(match id {
            0x0c00_181a => "Outskirts".into(),
            0x1100_181b => "on Heroic".into(),
            0x1d00_181c => "Fight your way through the city to the beachhead.".into(),
            0x0d00_1815 => "2 player(s) in party".into(),
            0x0c00_1814 => "Alpha is the party leader".into(),
            0x0f00_1818 | 0x0d00_1816 => String::new(),
            _ => return None,
        })
    };
    let host = |id: u32| -> Option<String> {
        match id {
            // Start state 9 for the leader (the co-op check lifted).
            0x1a00_1813 => Some("Waiting for you to start the game".into()),
            _ => common(id),
        }
    };
    let guest = |id: u32| -> Option<String> {
        match id {
            0x1a00_1813 => Some("Waiting for Alpha to start the game".into()),
            _ => common(id),
        }
    };
    let none = |_: u32| -> Option<String> { None };
    let shots: [(&str, Retail<'_>); 4] = [
        (
            "coop-lobby-host.png",
            Retail {
                tag: LOBBY,
                pane: 1,
                text: &host,
                cells: &players,
                footer: Some("\u{e100} SELECT  \u{e101} BACK".into()),
                rows: None,
                sample: None,
            },
        ),
        (
            "coop-lobby-guest.png",
            Retail {
                tag: LOBBY,
                pane: 1,
                text: &guest,
                cells: &players,
                footer: Some("\u{e101} BACK".into()),
                rows: None,
                sample: None,
            },
        ),
        (
            "coop-lobby-level.png",
            Retail {
                tag: r"ui\screens\game_shell\load_level\load_level_lobby",
                pane: 0,
                text: &none,
                cells: &[],
                footer: Some("\u{e100} SELECT  \u{e101} BACK".into()),
                rows: None,
                sample: None,
            },
        ),
        (
            "coop-lobby-difficulty.png",
            Retail {
                tag: r"ui\screens\game_shell\difficulty\difficulty_lobby",
                pane: 0,
                text: &none,
                cells: &[],
                footer: Some("\u{e100} SELECT  \u{e101} BACK".into()),
                rows: None,
                sample: None,
            },
        ),
    ];
    let out = std::env::var_os("H2_UI_PREVIEW_DIR").map(std::path::PathBuf::from);
    for (file, r) in &shots {
        let c = disc.render_retail(r).unwrap();
        if let Some(dir) = &out {
            std::fs::create_dir_all(dir).unwrap();
            c.write_png(&dir.join(file)).unwrap();
            eprintln!("drew {}", dir.join(file).display());
        }
    }
}

/// docs/runtime/snapshots.md: the screens block a snapshot while this
/// console hosts or joined a co-op campaign, while the join screen is due
/// and while the hooks are armed; idle they don't.
#[test]
fn the_co_op_screens_block_snapshots_while_in_a_session() {
    let (lobby, arm) = lobby_with(Arc::default());
    assert_eq!(lobby.snapshot_live(), None, "idle");
    lobby.open_from_menu();
    assert_eq!(
        lobby.snapshot_live().as_deref(),
        Some("hosting a co-op campaign")
    );
    lobby.leave();
    assert_eq!(lobby.snapshot_live(), None, "left");

    let j = live_hle::Joined::Known {
        session: Box::new(coop_session(None)),
        refreshed: true,
    };
    assert!(lobby.joined(&j));
    assert_eq!(
        lobby.snapshot_live().as_deref(),
        Some("joined Alpha's co-op campaign")
    );
    assert!(!lobby.joined(&live_hle::Joined::NotJoined));
    assert_eq!(
        lobby.snapshot_live().as_deref(),
        Some("the co-op join screen is due"),
        "not opened yet"
    );
    assert!(lobby.take_open_join());
    assert_eq!(lobby.snapshot_live(), None);

    // Armed by someone else (the probe's arm is shared).
    arm.arm(None);
    assert_eq!(
        lobby.snapshot_live().as_deref(),
        Some("a co-op session has armed the co-op hooks")
    );
    arm.disarm();
    assert_eq!(lobby.snapshot_live(), None);
}

#[test]
fn the_co_op_rows_lobby_mode_waits_only_inside_its_window() {
    let now = std::time::Instant::now();
    assert!(!coop_mode_waits(None, now));
    assert!(coop_mode_waits(Some(now + COOP_MODE_WINDOW), now));
    assert!(coop_mode_waits(Some(now), now));
    let late = now + COOP_MODE_WINDOW + std::time::Duration::from_secs(1);
    assert!(!coop_mode_waits(Some(now + COOP_MODE_WINDOW), late));
}

#[test]
fn advertised_squad_room_respects_both_peer_and_player_limits() {
    let mut squad = coop_probe::Session {
        state: 5,
        class: 2,
        players: 2,
        peers: 1,
        max_players: 8,
        max_peers: 4,
        ..Default::default()
    };
    let slots = squad_slots(squad, None).unwrap();
    assert_eq!((slots.public_filled, slots.public_open), (2, 3));
    assert_eq!(squad_slots(squad, Some(2)).unwrap().public_open, 0);
    squad.local_peer = 4;
    squad.host_peer = 4;
    assert!(squad_slots(squad, None).is_some(), "sparse peer indices");
    squad.peers = squad.max_peers;
    assert_eq!(squad_slots(squad, None).unwrap().public_open, 0);
    squad.local_peer = 2;
    assert!(squad_slots(squad, None).is_none());
}

#[test]
fn only_the_observed_live_host_supplies_slots() {
    let live = Arc::new(FakeLive::default());
    let (lobby, _) = lobby_with(Arc::clone(&live));
    for (squad, live_network, open) in [
        (Some(session(1, 1)), true, 1),
        (Some(session(1, 1)), false, 0),
        (
            Some(coop_probe::Session {
                local_peer: 3,
                ..session(2, 2)
            }),
            true,
            0,
        ),
        (None, true, 0),
    ] {
        lobby.observe_network(squad, live_network);
        lobby.ping_once();
        assert_eq!(live.slots.lock().unwrap().last().unwrap().public_open, open);
    }
}
