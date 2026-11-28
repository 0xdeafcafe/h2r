//! Match facts, read from the host's active game options and player datums.
//! The slot in a tick is absolute. A participant in the results is salted:
//! another player can take the same slot without inheriting scores or medals.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, Weak};

use anyhow::{Context, Result, ensure};
use bots::phantom::session::at as net;
use guest_layout::view::players;
use film::blam::{Event, MapIdentity, MatchHeader, Mode, PlayerSummary, Seat, SeatKind, Summary};
use runtime::Guest;
use serde_json::{Value, json};

use crate::{
    level_boot,
    telemetry::{self, layout},
};

/// A game's name for its stats container's header
/// (docs/game/film-format.md, "Recording every game").
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Described {
    pub(super) map: String,
    pub(super) variant: String,
    pub(super) players: Vec<film::Player>,
}

pub(super) struct Capture {
    descriptor: MatchHeader,
    map_digest: Option<std::thread::JoinHandle<Result<MapIdentity>>>,
    variant_name: String,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    live: Weak<live_hle::Hle>,
    sim_bots: Vec<sim_bench::config::Bot>,
    active: BTreeMap<u8, u32>,
    results: BTreeMap<u32, PlayerSummary>,
    quitting: BTreeSet<u32>,
    result_order: Vec<u32>,
    host_machine: Option<String>,
    lease_mode: Option<Mode>,
    /// A default recording records each game into a film of its own
    /// ([`super::autorecord`]): it describes one game after another.
    games: bool,
    /// The last game ended and its film is sealed; the next game starts
    /// once the guest leaves it (a map unload, or a new game's globals).
    between_games: bool,
    /// A game left in play (a campaign mission, a multiplayer game): the
    /// recorder seals its film at the next slice's check, so the next game
    /// describes itself instead of inheriting this one's start and map.
    left_game: bool,
    /// The described game was read off a mounted map. Only a game the
    /// guest was seen in can be left.
    on_map: bool,
}

impl Capture {
    pub(super) fn new(header: &film::Header) -> Self {
        let games = header.player_features;
        let lease_mode = header.lease_id.as_ref().and_then(|id| {
            let json = std::env::var("H2_HOST_LEASE_JSON").ok()?;
            let lease: live_proto::hosts::HostLease = serde_json::from_str(&json).ok()?;
            if &lease.id != id {
                return None;
            }
            if lease.assignment_id.starts_with("sim-") || lease.assignment_id.starts_with("custom-")
            {
                Some(Mode::Custom)
            } else {
                Some(Mode::Matchmaking)
            }
        });
        Self {
            descriptor: MatchHeader::default(),
            map_digest: None,
            lease_mode,
            variant_name: String::new(),
            active: BTreeMap::new(),
            results: BTreeMap::new(),
            quitting: BTreeSet::new(),
            result_order: Vec::new(),
            host_machine: None,
            vfs: None,
            live: Weak::new(),
            sim_bots: vec![],
            games,
            between_games: false,
            left_game: false,
            on_map: false,
        }
    }

    pub(super) fn descriptor(&self) -> &MatchHeader {
        &self.descriptor
    }

    /// The mounted maps the session was given (`Session::set_match_sources`):
    /// a film watched in this session checks its map against them.
    pub(super) fn vfs(&self) -> Option<Arc<Mutex<vfs::Vfs>>> {
        self.vfs.clone()
    }

    /// Forget the game just sealed (or left before its end) and describe
    /// the next one from its start. The match sources stay.
    pub(super) fn next_game(&mut self) {
        self.descriptor = MatchHeader::default();
        self.map_digest = None;
        self.variant_name.clear();
        self.active.clear();
        self.results.clear();
        self.quitting.clear();
        self.result_order.clear();
        self.host_machine = None;
        self.between_games = true;
        self.on_map = false;
    }

    /// The game's first update is film tick `tick`: a game described
    /// before it (between its activation and that update) starts there.
    pub(super) fn begin_at(&mut self, tick: u64) {
        if self.descriptor.match_start_tick.is_some() {
            self.descriptor.match_start_tick = Some(tick);
        }
    }

    /// The guest still shows the game whose film was just sealed.
    pub(super) fn between_games(&self) -> bool {
        self.between_games
    }

    /// Whether a game was left in play since the last call: its film is
    /// sealed as it stands, a campaign mission and a multiplayer game alike.
    pub(super) fn take_left(&mut self) -> bool {
        std::mem::take(&mut self.left_game)
    }

    #[cfg(test)]
    pub(super) fn start_for_test(&mut self, tick: u64, level: &str, mode: Mode) {
        self.descriptor.match_start_tick = Some(tick);
        self.descriptor.map.name = level.into();
        self.descriptor.mode = mode;
        self.variant_name = "Slayer".into();
    }

    /// The fixture's stand-in for the game having been observed on a
    /// mounted map, without which leaving it cannot be seen.
    #[cfg(test)]
    pub(super) fn on_map_for_test(&mut self) {
        self.on_map = true;
    }

    pub(super) fn finish_map(&mut self) -> Result<()> {
        if let Some(job) = self.map_digest.take() {
            self.descriptor.map = job.join().map_err(|_| anyhow::anyhow!("film map digest worker panicked"))??;
        }
        Ok(())
    }

    /// The game being recorded as its stats container's header names it:
    /// its map, variant and the players seen so far (scores are left out;
    /// they are in the final header). None outside a game.
    pub(super) fn description(&self) -> Option<Described> {
        let map = &self.descriptor.map.name;
        if map.is_empty() {
            return None;
        }
        Some(Described {
            map: map.clone(),
            variant: self.variant_name.clone(),
            players: self
                .result_order
                .iter()
                .map(|id| {
                    let p = &self.results[id];
                    film::Player { name: p.name.clone(), team: p.team, score: 0 }
                })
                .collect(),
        })
    }

    pub(super) fn summary(&self, tick: u64) -> Summary {
        Summary {
            dropped_samples: Default::default(),
            level: self.descriptor.map.name.clone(),
            game_type: self.descriptor.game_type.clone(),
            mode: self.descriptor.mode,
            variant_name: self.variant_name.clone(),
            duration_ticks: self
                .descriptor
                .match_start_tick
                .map_or(0, |start| tick.saturating_sub(start) + 1),
            players: self
                .result_order
                .iter()
                .map(|id| self.results[id].clone())
                .collect(),
        }
    }

    pub(super) fn observe(
        &mut self,
        g: &Guest,
        tick: u64,
        mut dropped: impl FnMut(&'static str),
    ) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        let Some(options) = active_options(g) else {
            self.between_games = false;
            if self.games && self.on_map && self.descriptor.match_start_tick.is_some() {
                // A game left in play (the guest is out of it with no
                // game-over) keeps its descriptor for its seal: a left
                // multiplayer game is a film of what was played, as a left
                // mission is. Only a game seen on a mounted map can be
                // left, and only a default recording seals one so.
                self.left_game = true;
            }
            return Ok(events);
        };
        if self.between_games {
            if game_ended(g) {
                return Ok(events);
            }
            self.between_games = false;
        }
        self.on_map = true;
        let s = bots::lobby::game_session(g).ok().filter(|&s| {
            s != 0
                && g.u32(s.wrapping_add(net::STATE))
                    .is_ok_and(|v| (3..=8).contains(&v))
        });
        if self.descriptor.map.name.is_empty() {
            let Ok(info) = telemetry::game_info(g) else {
                return Ok(events);
            };
            if !info.is_match() || info.map.is_empty() || info.map == "mainmenu" {
                return Ok(events);
            }
            let vfs = self.vfs.as_ref().context("film map VFS is unavailable")?;
            let cache_path = crate::map_slots::current_source(g);
            let (identity, source) = prepare_map(vfs, &info.map, cache_path.as_deref(), None)?;
            self.descriptor.map = identity.clone();
            self.map_digest = Some(background::spawn("film-map-hash", move || hash_map(identity, source))?);
            self.descriptor
                .variant_blob
                .resize(level_boot::VARIANT_SIZE as usize, 0);
            g.read_bytes(
                options + level_boot::OPT_VARIANT,
                &mut self.descriptor.variant_blob,
                1,
            )?;
            self.descriptor.match_start_tick = Some(tick);
            self.descriptor.ticks_per_second = Some(info.tick_rate.max(1));

            self.descriptor.skulls = active_skulls(&info);
            self.descriptor.game_type = info.variant.unwrap_or_else(|| info.mode.clone());
            self.variant_name = variants::GameVariant::from_bytes(&self.descriptor.variant_blob)
                .ok()
                .map(|v| v.name())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| self.descriptor.game_type.clone());
            self.descriptor.difficulty = info.difficulty;
            self.descriptor.mode = if info.mode == "campaign" {
                let split = g.u8(options.wrapping_add(level_boot::OPT_SPLIT)).unwrap_or_else(|_| {
                    dropped("campaign_mode");
                    0
                });
                if split != 0
                    || s.is_some_and(|s| g.u32(s.wrapping_add(net::PLAYER_COUNT)).is_ok_and(|n| n > 1))
                {
                    Mode::CoopCampaign
                } else {
                    Mode::Campaign
                }
            } else if let Some(mode) = self.lease_mode {
                mode
            } else if bots::lobby::matchmaking_cycle(g)
                || self.live.upgrade().is_some_and(|h| h.matchmade().is_some())
            {
                Mode::Matchmaking
            } else if s.is_some_and(|s| {
                system_link(
                    g.u32(s.wrapping_add(net::PROTOCOL)).ok(),
                    g.u32(s.wrapping_add(crate::variant_ext::SESSION_NETWORK_CLASS)).ok(),
                )
            }) {
                Mode::SystemLink
            } else {
                Mode::Custom
            };
        }
        if let Some(machine) = s.and_then(|s| host_machine(g, s)) {
            if self
                .host_machine
                .as_ref()
                .is_some_and(|old| old != &machine)
            {
                events.push(Event::HostMigration {
                    host_id: machine.clone(),
                });
            }
            self.descriptor
                .host_id
                .get_or_insert_with(|| machine.clone());
            self.host_machine = Some(machine);
        }
        let rows = match players::array(g)
            .and_then(|array| array.map(|array| array.live(g, 16)).transpose())
        {
            Ok(Some(rows)) => rows,
            Ok(None) => return Ok(events),
            Err(_) => {
                dropped("roster");
                return Ok(events);
            }
        };
        let initial = self.results.is_empty();
        let mut active = BTreeMap::new();
        let mut joins = Vec::new();
        let stats = g.u32(layout::ENGINE_GLOBALS_PTR).and_then(|base| {
            if base == 0 {
                Ok(None)
            } else {
                g.u32(base.wrapping_add(layout::ENGINE_STATBORG))
                    .map(|p| (p != 0).then_some(base))
            }
        }).unwrap_or_else(|_| {
            dropped("scoreboard");
            None
        });
        for (handle, row) in rows {
            let seat = handle.index() as u8;
            let datum = handle.raw();
            // Commit this seat only after reading its identity. A stale row
            // must not manufacture a departure, team change or duplicate join.
            let describe = || -> Result<_> {
                if !row.listed(g).map_err(|e| anyhow::anyhow!("{e:?}"))? {
                    return Ok(None);
                }
                let team = i32::from(row.team(g).map_err(|e| anyhow::anyhow!("{e:?}"))?);
                let joining = if self.active.get(&seat) == Some(&datum) {
                    None
                } else {
                    Some(self.seat(g, row, seat, team, s)?)
                };
                Ok(Some((team, joining)))
            };
            let (team, joining) = match describe() {
                Ok(Some(seat)) => seat,
                Ok(None) => {
                    // Retail hides a quitting player before releasing its datum.
                    if self.active.get(&seat) == Some(&datum) {
                        active.insert(seat, datum);
                        if self.quitting.insert(datum) {
                            events.push(Event::Quit { seat });
                        }
                    }
                    continue;
                }
                Err(_) => {
                    dropped("roster_player");
                    if let Some(&previous) = self.active.get(&seat) {
                        active.insert(seat, previous);
                    }
                    continue;
                }
            };
            self.quitting.remove(&datum);
            active.insert(seat, datum);
            if let Some(player) = joining {
                if self.descriptor.host_id.is_none()
                    && player.kind == SeatKind::Local
                    && s.is_none()
                {
                    self.descriptor.host_id = player
                        .xuid
                        .map(|x| live_proto::Xuid(x).to_string())
                        .or_else(|| {
                            row.machine(g)
                                .ok()
                                .filter(|m| *m != [0; 6])
                                .map(|m| m.iter().map(|b| format!("{b:02x}")).collect())
                        });
                }
                if !self.results.contains_key(&datum) {
                    self.result_order.push(datum);
                }
                self.results.insert(
                    datum,
                    PlayerSummary {
                        seat,
                        team,
                        name: player.name.clone(),
                        xuid: player.xuid,
                        ..Default::default()
                    },
                );
                if initial {
                    self.descriptor.roster.push(player.clone());
                }
                joins.push(Event::Join { player });
            }
            if let Some(result) = self.results.get_mut(&datum) {
                if result.team != team {
                    events.push(Event::Team { seat, team });
                }
                result.team = team;
                if let Some(base) = stats {
                    let at = base
                        .wrapping_add(layout::ENGINE_PLAYER_STATS)
                        .wrapping_add(u32::from(seat) * layout::PLAYER_STAT_SIZE);
                    match g.u16(at.wrapping_add(layout::STAT_SCORE)) {
                        Ok(score) => result.score = i64::from(score as i16),
                        Err(_) => dropped("scoreboard"),
                    }
                }
            }
        }
        for (&seat, handle) in &self.active {
            if active.get(&seat) != Some(handle) {
                self.quitting.remove(handle);
                events.push(Event::Leave { seat });
            }
        }
        events.extend(joins);
        self.active = active;
        Ok(events)
    }

    fn seat(
        &self,
        g: &Guest,
        row: players::Player,
        seat: u8,
        team: i32,
        session: Option<u32>,
    ) -> Result<Seat> {
        let mut name = [0; players::NAME_UNITS];
        let n = row
            .name_units(g, &mut name)
            .map_err(|e| anyhow::anyhow!("film player name: {e:?}"))?;
        let name = String::from_utf16_lossy(live_proto::bots::unmarked_units(&name[..n]));
        let machine = row
            .machine(g)
            .map_err(|e| anyhow::anyhow!("film player machine: {e:?}"))?;
        let port = u8::try_from(g.u32(row.addr.wrapping_add(layout::PLAYER_CONTROLLER))?)
            .ok()
            .filter(|&v| v < 4);
        let phantom = bots::phantom::handle_of(machine);
        let kind = if phantom.is_some() {
            SeatKind::Phantom
        } else if row.local_user(g).is_ok_and(|u| (0..4).contains(&u)) {
            SeatKind::Local
        } else {
            SeatKind::Remote
        };
        let xuid = session
            .and_then(|s| session_xuid(g, s, machine, row.addr))
            .or_else(|| {
                phantom.and_then(|h| {
                    bots::phantom::list()
                        .into_iter()
                        .find(|p| p.handle == h)
                        .map(|p| p.identity.xuid)
                })
            })
            .or_else(|| port.and_then(|p| self.live.upgrade().map(|h| h.user(usize::from(p)))))
            .filter(|&id| id != 0);
        let bot_params = phantom.and_then(|h| {
            self.sim_bots.iter()
                .find(|b| xuid == Some(live_proto::bots::local_xuid(u32::from(b.seat) + 1).0))
                .map(|b| json!({"skill": format!("{:?}", b.skill), "seed": b.seed, "tuning": b.params}))
                .or_else(|| bots::phantom::global().brain_view(h).map(brain_parameters))
        });
        Ok(Seat {
            seat,
            name,
            xuid,
            team,
            kind,
            bot_params,
        })
    }
}

/// The retail game-over latch of the loaded game.
pub(super) fn game_ended(g: &Guest) -> bool {
    g.u32(layout::GAME_GLOBALS_PTR)
        .ok()
        .filter(|p| *p != 0)
        .and_then(|p| p.checked_add(layout::GAME_ENDED))
        .and_then(|p| g.u8(p).ok())
        .is_some_and(|v| v != 0)
}

pub(super) fn active_options(g: &Guest) -> Option<u32> {
    if !matches!(
        level_boot::active_mode(g),
        Some(level_boot::MODE_CAMPAIGN | level_boot::MODE_MULTIPLAYER)
    ) {
        return None;
    }
    g.u32(layout::GAME_GLOBALS_PTR)
        .ok()?
        .checked_add(layout::OPTIONS)
}

fn active_skulls(info: &telemetry::GameInfo) -> Vec<String> {
    let skulls = if info.mode == "campaign" {
        let mut on = info.skulls.clone().unwrap_or_default();
        on.extend(
            live_proto::skulls::OURS.filter(|&i| crate::skulls_custom::campaign().has(i as usize)),
        );
        on
    } else {
        live_proto::skulls::MULTIPLAYER
            .into_iter()
            .filter(|&i| crate::skulls_mp::shared().is_some_and(|mp| mp.active(i as usize)))
            .collect()
    };
    skulls
        .into_iter()
        .filter_map(|i| {
            crate::skulls::SKULLS
                .get(i as usize)
                .map(|d| d.key.to_owned())
        })
        .collect()
}

pub(super) fn verify_restored_map(
    expected: &MatchHeader,
    vfs: Option<&Arc<Mutex<vfs::Vfs>>>,
) -> Result<()> {
    verify_map(vfs.context("film map VFS unavailable")?, &expected.map)?;
    // The film's map is the package its header names: a Forge map's
    // document makes the Firefight log's genesis
    // (`crate::map_variant::begin_run_replica`).
    crate::map_variant::set_loaded(
        expected.map.package_id.as_deref().and_then(|id| id.parse().ok()),
    );
    Ok(())
}

pub(super) fn verify_start(
    g: &Guest,
    tick: u64,
    expected: &MatchHeader,
    vfs: Option<&Arc<Mutex<vfs::Vfs>>>,
) -> Result<()> {
    if expected.match_start_tick != Some(tick) {
        return Ok(());
    }
    let opts = active_options(g).context("film expected an active match")?;
    let info = telemetry::game_info(g)?;
    ensure!(
        info.map == expected.map.name,
        "loaded film map differs at tick {tick}"
    );
    if let Some(path) = &expected.map.cache_path {
        ensure!(
            crate::map_slots::current_source(g).is_some_and(|p| p.eq_ignore_ascii_case(path)),
            "loaded film cache slot differs at tick {tick}"
        );
    }
    let mut variant = [0; level_boot::VARIANT_SIZE as usize];
    g.read_bytes(opts + level_boot::OPT_VARIANT, &mut variant, 1)?;
    ensure!(
        expected
            .ticks_per_second
            .is_none_or(|hz| hz == info.tick_rate.max(1))
            && variant.as_slice() == expected.variant_blob.as_slice()
            && info.difficulty == expected.difficulty
            && active_skulls(&info) == expected.skulls,
        "film tick rate, variant, difficulty or skulls differ at tick {tick}"
    );
    verify_restored_map(expected, vfs)
}

fn brain_parameters(v: bots::BrainView) -> Value {
    let p = v.personality;
    json!({
        "skill": format!("{:?}", v.skill),
        "personality": {
            "aggression": p.aggression, "caution": p.caution, "teamwork": p.teamwork,
            "objective_focus": p.objective_focus, "composure": p.composure,
            "patience": p.patience, "showboating": p.showboating, "weapon_loyalty": p.weapon_loyalty,
            "chattiness": p.chattiness, "favorite_weapon": format!("{:?}", p.favorite_weapon)
        }
    })
}

fn host_machine(g: &Guest, s: u32) -> Option<String> {
    let host = g.u32(s.wrapping_add(net::HOST_PEER)).ok()?;
    if host >= g.u32(s.wrapping_add(net::PEER_COUNT)).ok()?.min(16) {
        return None;
    }
    let mut mac = [0; 6];
    g.read_bytes(
        s.wrapping_add(net::PEERS + host * net::PEER_SIZE + net::PEER_MAC),
        &mut mac,
        1,
    )
    .ok()?;
    (mac != [0; 6]).then(|| mac.iter().map(|b| format!("{b:02x}")).collect())
}

/// Match by originating machine and machine *user*, including split-screen peers.
fn session_xuid(g: &Guest, s: u32, machine: [u8; 6], player: u32) -> Option<u64> {
    let peer = bots::phantom::session::find_peer(g, s, machine).ok()??;
    let user = u32::from(g.u16(player.wrapping_add(0x1c)).ok()?);
    let mask = g.u32(s.wrapping_add(net::PLAYER_MASK)).ok()?;
    for i in 0..16 {
        let p = s.wrapping_add(net::PLAYERS + i * net::PLAYER_SIZE);
        if mask & (1 << i) != 0
            && g.u32(p.wrapping_add(net::P_PEER)).ok()? == peer
            && g.u32(p.wrapping_add(net::P_USER)).ok()? == user
        {
            let mut id = [0; 8];
            g.read_bytes(p.wrapping_add(net::P_ID), &mut id, 1).ok()?;
            let id = u64::from_le_bytes(id);
            return (id != 0).then_some(id);
        }
    }
    None
}

fn prepare_map(
    vfs: &Arc<Mutex<vfs::Vfs>>,
    name: &str,
    cache_path: Option<&str>,
    recorded: Option<&MapIdentity>,
) -> Result<(MapIdentity, vfs::FilmMap)> {
    ensure!(
        !name.is_empty()
            && name.len() <= 32
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b' ')),
        "invalid film map name"
    );
    let path = match cache_path {
        Some(path) => {
            let mounted = crate::map_slots::recorded_file(path).context("invalid film cache path")?;
            if mounted == path && vfs.lock().unwrap_or_else(|e| e.into_inner()).query_path(&mounted).is_err() {
                // A recorded title-content slot can be verified before the title links T:.
                crate::map_slots::slot_file(path).unwrap_or(mounted)
            } else {
                mounted
            }
        }
        None => vfs.lock().unwrap_or_else(|e| e.into_inner())
            .content_map_path(name)
            .unwrap_or_else(|| format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE)),
    };
    let source = vfs::FilmMap::open(vfs, &path, recorded.map(|r| r.ranges.as_deref()))
        .with_context(|| format!("opening mounted map {name}"))?;
    let ranges = source.ranges().map(Vec::from);
    let origin = if crate::h1_options_row::is_halo1_map(name)
        || crate::health::HALO1_LEVELS.contains(&name)
    {
        "converted_halo1"
    } else if cache_path.is_some_and(|p| p.to_ascii_lowercase().starts_with("t:"))
    {
        "mod"
    } else {
        "retail"
    };
    Ok((MapIdentity {
        name: name.into(),
        origin: origin.into(),
        sha256: String::new(),
        ranges,
        package_id: None,
        cache_path: cache_path.map(str::to_owned),
    }, source))
}

fn hash_map(mut id: MapIdentity, source: vfs::FilmMap) -> Result<MapIdentity> {
    let (digest, package) = source.digest(&agent_proto::cache_dir().join("film-map-digests"))
        .with_context(|| format!("hashing mounted map {}", id.name))?;
    if id.origin == "retail" && package.is_some() {
        id.origin = "mod".into();
    }
    id.package_id = package.map(|p| p.to_string());
    id.sha256 = digest.to_string();
    Ok(id)
}

fn map_identity(
    vfs: &Arc<Mutex<vfs::Vfs>>, name: &str, cache_path: Option<&str>, recorded: Option<&MapIdentity>,
) -> Result<MapIdentity> {
    let (identity, source) = prepare_map(vfs, name, cache_path, recorded)?;
    hash_map(identity, source)
}

pub(super) fn verify_map(vfs: &Arc<Mutex<vfs::Vfs>>, expected: &MapIdentity) -> Result<()> {
    let actual = map_identity(
        vfs,
        &expected.name,
        expected.cache_path.as_deref(),
        Some(expected),
    )?;
    ensure!(
        &actual == expected,
        "film map identity differs for {} (expected {}, observed {})",
        expected.name,
        expected.sha256,
        actual.sha256
    );
    Ok(())
}

impl super::Session {
    pub fn set_match_sources(
        &self,
        vfs: Option<Arc<Mutex<vfs::Vfs>>>,
        live: Option<Arc<live_hle::Hle>>,
        sim: Option<&sim_bench::config::Match>,
    ) {
        let mut state = self.lock();
        if let Some(play) = &mut state.play {
            play.map_vfs.clone_from(&vfs);
        }
        if let super::Mode::Record(rec) = &mut state.mode {
            rec.meta.vfs = vfs;
            rec.meta.live = live.as_ref().map(Arc::downgrade).unwrap_or_default();
            if let Some(m) = sim {
                rec.meta.sim_bots.clone_from(&m.bots);
                rec.meta.descriptor.seed = Some(m.config.seed);
            }
        }
    }
}

pub(super) fn install(session: &Arc<super::Session>, b: &mut hooks::Builder) {
    let s = Arc::clone(session);
    b.raw()
        .observe_before(&hooks::points::GAME_TICK, "initial film seed", move |o| {
            let mut state = s.lock();
            if let super::Mode::Record(rec) = &mut state.mode
                && rec.meta.descriptor.seed.is_none()
            {
                let g = o.guest();
                // After a game's film is sealed, the next game's seed, not the ended game's.
                if active_options(&g).is_some() && !(rec.meta.between_games() && game_ended(&g)) {
                    rec.meta.descriptor.seed = g
                        .u32(crate::sim_glue::RANDOM_SEED_PTR)
                        .ok()
                        .filter(|&p| p != 0)
                        .and_then(|p| g.u32(p).ok())
                        .map(u64::from);
                }
            }
        });
}

/// A networked game's session is System Link: a networked protocol (1 or
/// 2) and the network class 1 (`crate::variant_ext::SESSION_NETWORK_CLASS`;
/// a Live game's is 2, whichever Live service it runs on).
fn system_link(protocol: Option<u32>, class: Option<u32>) -> bool {
    matches!(protocol, Some(1 | 2)) && class == Some(crate::variant_ext::SYSTEM_LINK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_link_is_the_network_class_not_the_service() {
        assert!(system_link(Some(2), Some(1)));
        assert!(system_link(Some(1), Some(1)));
        assert!(!system_link(Some(2), Some(2)), "a Live game");
        assert!(!system_link(Some(0), Some(1)), "a local protocol");
        assert!(!system_link(Some(2), None), "an unreadable class");
    }

    #[test]
    fn checks_the_mounted_map_bytes_and_only_allows_guest_map_slots() {
        let dir =
            std::env::temp_dir().join(format!("film-match-map-{}", live_proto::sign::random_id()));
        std::fs::create_dir(&dir).unwrap();
        let iso = crate::map_override::tests::iso_at(&dir, b"film map on disc");
        let cfg = vfs::Config::new(dir.join("data"), 0x4d53_0064);
        let vfs = Arc::new(Mutex::new(vfs::Vfs::with_disc(cfg, &iso).unwrap()));
        let path = "d:\\maps\\level.map";
        let id = map_identity(&vfs, "level", Some(path), None).unwrap();
        assert_eq!(
            id.sha256,
            content::Sha256::of(b"film map on disc").to_string()
        );
        assert_eq!(id.origin, "retail");
        assert!(verify_map(&vfs, &id).is_ok());
        // A light host only has ranges. Re-rendering on a complete disc
        // must check precisely those bytes at the same offsets.
        let store = dir.join("stores");
        vfs::sim_store::build(
            &iso,
            "level",
            &[(0, 4), (9, 13)],
            &store.join("level.h2store"),
        )
        .unwrap();
        let mut config = vfs::Config::new(dir.join("sparse"), 0x4d53_0064);
        config.sim_store = Some(store.clone());
        let sparse = Arc::new(Mutex::new(
            crate::map_override::mount(
                config,
                &iso,
                None::<crate::map_override::tests::Upper>,
                None,
                &[],
            )
            .unwrap(),
        ));
        let sparse_id = map_identity(&sparse, "level", Some(path), None).unwrap();
        assert_eq!(sparse_id.ranges, Some(vec![(0, 4), (9, 4)]));
        assert_ne!(sparse_id.sha256, id.sha256);
        verify_map(&vfs, &sparse_id).unwrap();
        drop(sparse);

        // Replace just the store, leaving the original ISO and the logical
        // range table intact. A second dedicated host must read the new body.
        let file = store.join("level.h2store");
        let mut bytes = std::fs::read(&file).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        let replacement = store.join("replacement");
        std::fs::write(&replacement, bytes).unwrap();
        std::fs::rename(replacement, &file).unwrap();
        let mut config = vfs::Config::new(dir.join("sparse"), 0x4d53_0064);
        config.sim_store = Some(store);
        let updated = Arc::new(Mutex::new(crate::map_override::mount(
            config, &iso, None::<crate::map_override::tests::Upper>, None, &[],
        ).unwrap()));
        assert!(verify_map(&updated, &sparse_id).is_err());
        drop(updated);
        let mut wrong = id;
        wrong.sha256 = "00".repeat(32);
        assert!(verify_map(&vfs, &wrong).is_err());
        assert!(map_identity(&vfs, "level", Some("t:\\profile"), None).is_err());
        drop(vfs);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
