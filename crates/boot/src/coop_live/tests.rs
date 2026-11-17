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
    use engine::memory::GuestMemory as _;
    use engine::testing::FakeGuest;
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
    assert!(
        matches!(friends.press(1), Chosen::Stepped(m) if m.contains("offline")),
        "an offline friend is not invited"
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
        },
        since: String::new(),
        last_activity: 0,
        state: "open".into(),
        players: Default::default(),
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
fn the_live_menu_has_six_rows_with_co_op_above_content_download() {
    use crate::custom_games::*;
    assert_eq!(list_count_with(LIST_NAME, VANILLA_ROWS, true), 6);
    assert_eq!(list_count_with(LIST_NAME, VANILLA_ROWS, false), 5);
    assert_eq!(list_count_with(LIST_NAME, 6, true), 6, "not twice");
    let kinds: Vec<Row> = (0..7).map(|r| row_kind_with(r, true)).collect();
    assert_eq!(
        kinds,
        [
            Row::Retail,
            Row::Retail,
            Row::Retail,
            Row::Browser,
            Row::Coop,
            Row::Download,
            Row::Retail
        ]
    );
    assert_eq!(
        row_kind_with(4, false),
        Row::Download,
        "co-op off: as before"
    );
    assert_eq!(pane_of_with(COOP_ROW, 6, true), Pane::Show(COOP_PANE));
    assert_eq!(pane_of_with(ROW, 6, true), Pane::Show(BROWSER_PANE));
    assert_eq!(pane_of_with(DOWNLOAD_ROW, 6, true), Pane::Show(3));
    assert_eq!(pane_of_with(COOP_ROW, 5, true), Pane::Keep, "no co-op pane");
    assert_eq!(pane_of_with(COOP_ROW, 4, true), Pane::Keep);
    assert_eq!(extended_panes(true), 6);
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
    let mut r = feature::Registry::new(feature::Switches::defaults());
    let arm = coop_probe::install_live(&rt, &mut r, Path::new("/nonexistent"))
        .unwrap()
        .unwrap();
    assert!(!arm.active() && !arm.is_forced());
    let text = r.build(&feature::Base).unwrap().text();
    assert!(text.contains(coop_probe::LIVE_ID), "{text}");
    for p in coop_probe::HOOKED {
        assert!(text.contains(p.name), "{} missing:\n{text}", p.name);
    }
    assert!(text.contains("host lobby setup"), "{text}");
    // H2_FEATURES=-coop-live turns them off.
    let mut r = feature::Registry::new(feature::Switches::parse("-coop-live").unwrap());
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
    let mut r = feature::Registry::new(feature::Switches::defaults());
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
    let plan = r.build(&feature::Base).unwrap();
    let text = plan.text();
    assert!(
        text.contains(FEATURE) && text.contains("open the co-op join screen"),
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
