//! Match facts, read from the host's active game options and player datums.
//! The slot in a tick is absolute. A participant in the results is salted:
//! another player can take the same slot without inheriting scores or medals.
use live_proto::film::events::medals;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, Weak};

use anyhow::{Context, Result, ensure};
use bots::phantom::session::at as net;
use engine::view::players;
use film::blam::{Event, MapIdentity, MatchHeader, Mode, PlayerSummary, Seat, SeatKind, Summary};
use runtime::Guest;
use serde_json::{Value, json};

use crate::{
    level_boot,
    telemetry::{self, layout},
};

pub(super) struct Capture {
    descriptor: MatchHeader,
    variant_name: String,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    live: Weak<live_hle::Hle>,
    sim_bots: Vec<sim::config::Bot>,
    active: BTreeMap<u8, u32>,
    results: BTreeMap<u32, PlayerSummary>,
    quitting: BTreeSet<u32>,
    medals: medals::Tally,
    host_machine: Option<String>,
    live_service: bool,
    lease_mode: Option<Mode>,
    team_play: bool,
}

impl Capture {
    pub(super) fn new(header: &film::Header) -> Self {
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
            lease_mode,
            variant_name: String::new(),
            active: BTreeMap::new(),
            results: BTreeMap::new(),
            quitting: BTreeSet::new(),
            medals: Default::default(),
            host_machine: None,
            vfs: None,
            live: Weak::new(),
            sim_bots: vec![],
            live_service: header.live_service,
            team_play: false,
        }
    }

    pub(super) fn descriptor(&self) -> &MatchHeader {
        &self.descriptor
    }

    pub(super) fn summary(&self, tick: u64) -> Summary {
        Summary {
            level: self.descriptor.map.name.clone(),
            game_type: self.descriptor.game_type.clone(),
            mode: self.descriptor.mode,
            variant_name: self.variant_name.clone(),
            duration_ticks: self
                .descriptor
                .match_start_tick
                .map_or(0, |start| tick.saturating_sub(start) + 1),
            players: self
                .results
                .iter()
                .map(|(&handle, p)| {
                    let mut p = p.clone();
                    p.medals = self.medals.counts(handle);
                    p
                })
                .collect(),
        }
    }

    pub(super) fn observe(&mut self, g: &Guest, tick: u64) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        let Some(options) = active_options(g) else {
            return Ok(events);
        };
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
            self.descriptor.map = map_identity(vfs, &info.map, cache_path.as_deref(), None)?;
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
            self.team_play = info.team_play;
            self.descriptor.difficulty = info.difficulty;
            self.descriptor.mode = if info.mode == "campaign" {
                if g.u8(options + level_boot::OPT_SPLIT)? != 0
                    || s.is_some_and(|s| g.u32(s + net::PLAYER_COUNT).is_ok_and(|n| n > 1))
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
            } else if !self.live_service
                && s.is_some_and(|s| g.u32(s + net::PROTOCOL).is_ok_and(|p| matches!(p, 1 | 2)))
            {
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
        let Some(array) =
            players::array(g).map_err(|e| anyhow::anyhow!("film players array: {e:?}"))?
        else {
            return Ok(events);
        };
        let rows = array
            .live(g, 16)
            .map_err(|e| anyhow::anyhow!("film players: {e:?}"))?;
        let initial = self.results.is_empty();
        let mut active = BTreeMap::new();
        let mut joins = Vec::new();
        let stats = g
            .u32(layout::ENGINE_GLOBALS_PTR)
            .ok()
            .filter(|&p| p != 0 && g.u32(p + layout::ENGINE_STATBORG).is_ok_and(|v| v != 0));
        for (handle, row) in rows {
            let seat = handle.index() as u8;
            let datum = handle.raw();
            if !row
                .listed(g)
                .map_err(|e| anyhow::anyhow!("film player flags: {e:?}"))?
            {
                // The retail iterator hides a quitting player before the engine
                // releases its datum. Emit both transitions at their own ticks.
                if self.active.get(&seat) == Some(&datum) {
                    active.insert(seat, datum);
                    if self.quitting.insert(datum) {
                        events.push(Event::Quit { seat });
                    }
                }
                continue;
            }
            self.quitting.remove(&datum);
            active.insert(seat, datum);
            let team = i32::from(
                row.team(g)
                    .map_err(|e| anyhow::anyhow!("film player team: {e:?}"))?,
            );
            if self.active.get(&seat) != Some(&datum) {
                let player = self.seat(g, row, seat, team, s)?;
                if self.descriptor.host_id.is_none()
                    && matches!(player.kind, SeatKind::Local | SeatKind::Bot)
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
                        + layout::ENGINE_PLAYER_STATS
                        + u32::from(seat) * layout::PLAYER_STAT_SIZE;
                    result.score = i64::from(g.u16(at + layout::STAT_SCORE)? as i16);
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
        let port = u8::try_from(g.u32(row.addr + layout::PLAYER_CONTROLLER)?)
            .ok()
            .filter(|&v| v < 4);
        let bot = port.and_then(|port| {
            bots::local::list()
                .into_iter()
                .find(|b| b.port == Some(port))
        });
        let phantom = bots::phantom::handle_of(machine);
        let kind = if phantom.is_some() {
            SeatKind::Phantom
        } else if bot.is_some() {
            SeatKind::Bot
        } else if row.local_user(g).is_ok_and(|u| (0..4).contains(&u)) {
            SeatKind::Local
        } else {
            SeatKind::Remote
        };
        let xuid = session
            .and_then(|s| session_xuid(g, s, machine, row.addr))
            .or_else(|| {
                bot.as_ref()
                    .and_then(|b| bots::local::global().account_of(b.handle).map(|a| a.xuid))
            })
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
        let bot_params = if matches!(kind, SeatKind::Bot | SeatKind::Phantom) {
            self.sim_bots.iter().find(|b| match b.port {
                Some(p) => Some(p) == port,
                None => xuid == Some(live_proto::bots::local_xuid(u32::from(b.seat)+1).0),
            }).map(|b| json!({"skill": format!("{:?}", b.skill), "seed": b.seed, "tuning": b.params}))
            .or_else(|| bot.and_then(|b| bots::local::global().brain_view(b.handle))
                .or_else(|| phantom.and_then(|h| bots::phantom::global().brain_view(h))).map(brain_parameters))
        } else {
            None
        };
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

fn active_options(g: &Guest) -> Option<u32> {
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
    vfs: Option<&Mutex<vfs::Vfs>>,
) -> Result<()> {
    verify_map(vfs.context("film map VFS unavailable")?, &expected.map)
}

pub(super) fn verify_start(
    g: &Guest,
    tick: u64,
    expected: &MatchHeader,
    vfs: Option<&Mutex<vfs::Vfs>>,
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
        expected.ticks_per_second.is_none_or(|hz| hz == info.tick_rate.max(1))
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
    let host = g.u32(s + net::HOST_PEER).ok()?;
    if host >= g.u32(s + net::PEER_COUNT).ok()?.min(16) {
        return None;
    }
    let mut mac = [0; 6];
    g.read_bytes(
        s + net::PEERS + host * net::PEER_SIZE + net::PEER_MAC,
        &mut mac,
        1,
    )
    .ok()?;
    (mac != [0; 6]).then(|| mac.iter().map(|b| format!("{b:02x}")).collect())
}

/// Match by originating machine and machine *user*, including split-screen peers.
fn session_xuid(g: &Guest, s: u32, machine: [u8; 6], player: u32) -> Option<u64> {
    let peer = bots::phantom::session::find_peer(g, s, machine).ok()??;
    let user = u32::from(g.u16(player + 0x1c).ok()?);
    let mask = g.u32(s + net::PLAYER_MASK).ok()?;
    for i in 0..16 {
        let p = s + net::PLAYERS + i * net::PLAYER_SIZE;
        if mask & (1 << i) != 0
            && g.u32(p + net::P_PEER).ok()? == peer
            && g.u32(p + net::P_USER).ok()? == user
        {
            let mut id = [0; 8];
            g.read_bytes(p + net::P_ID, &mut id, 1).ok()?;
            let id = u64::from_le_bytes(id);
            return (id != 0).then_some(id);
        }
    }
    None
}

fn map_identity(
    vfs: &Mutex<vfs::Vfs>,
    name: &str,
    cache_path: Option<&str>,
    recorded: Option<&MapIdentity>,
) -> Result<MapIdentity> {
    ensure!(
        !name.is_empty()
            && name.len() <= 32
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b' ')),
        "invalid film map name"
    );
    let mut vfs = vfs.lock().unwrap_or_else(|e| e.into_inner());
    let path = match cache_path {
        Some(path) => crate::map_slots::slot_file(path).context("invalid film cache path")?,
        None => vfs
            .content_map_path(name)
            .unwrap_or_else(|| format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE)),
    };
    let package_id = vfs.map_id(&path).map(|id| id.to_string());
    let ranges = match recorded {
        Some(id) => id.ranges.clone(),
        None => vfs.map_ranges(&path)?,
    };
    let sha256 = vfs
        .file_sha256(&path, ranges.as_deref())
        .with_context(|| format!("hashing mounted map {name}"))?
        .to_string();
    let origin = if crate::h1_options_row::is_halo1_map(name)
        || crate::health::HALO1_LEVELS.contains(&name)
    {
        "converted_halo1"
    } else if package_id.is_some()
        || cache_path.is_some_and(|p| p.to_ascii_lowercase().starts_with("t:"))
    {
        "mod"
    } else {
        "retail"
    };
    Ok(MapIdentity {
        name: name.into(),
        origin: origin.into(),
        sha256,
        ranges,
        package_id,
        cache_path: cache_path.map(str::to_owned),
    })
}

pub(super) fn verify_map(vfs: &Mutex<vfs::Vfs>, expected: &MapIdentity) -> Result<()> {
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
    pub(crate) fn set_match_sources(
        &self,
        vfs: Option<Arc<Mutex<vfs::Vfs>>>,
        live: Option<Arc<live_hle::Hle>>,
        sim: Option<&sim::config::Match>,
    ) {
        let mut state = self.lock();
        if let Some(engine) = &mut state.engine {
            engine.map_vfs.clone_from(&vfs);
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

pub(super) fn install(session: &Arc<super::Session>, b: &mut feature::Builder) {
    let s = Arc::clone(session);
    b.raw()
        .observe_before(&feature::points::GAME_TICK, "initial film seed", move |o| {
            let mut state = s.lock();
            if let super::Mode::Record(rec) = &mut state.mode
                && rec.meta.descriptor.seed.is_none()
            {
                let g = o.guest();
                if active_options(&g).is_some() {
                    rec.meta.descriptor.seed = g
                        .u32(crate::sim_glue::RANDOM_SEED_PTR)
                        .ok()
                        .filter(|&p| p != 0)
                        .and_then(|p| g.u32(p).ok())
                        .map(u64::from);
                }
            }
        });
    let s = Arc::clone(session);
    b.raw()
        .observe_before(&feature::points::PLAYER_KILLED, "film medals", move |o| {
            let g = o.guest();
            let Ok(victim) = g.u32(o.cpu.esp.wrapping_add(8)) else {
                return;
            };
            let Ok(attacker) = g.u32(o.cpu.esp.wrapping_add(4)) else {
                return;
            };
            let mut state = s.lock();
            let tick = state.tick;
            if let super::Mode::Record(rec) = &mut state.mode {
                if rec.meta.descriptor.game_type.is_empty() || victim == u32::MAX {
                    return;
                }
                let scoring = attacker != u32::MAX
                    && attacker != victim
                    && rec.meta.results.get(&attacker).is_some_and(|a| {
                        rec.meta
                            .results
                            .get(&victim)
                            .is_some_and(|v| !rec.meta.team_play || a.team < 0 || a.team != v.team)
                    });
                rec.meta
                    .medals
                    .kill(tick, scoring.then_some(attacker), victim);
            }
        })
        .optional();
}

/// Verify the final transition affecting each seat on this tick. A replacement
/// can leave and join the same slot before the next engine update.
pub(super) fn verify_events(g: &Guest, events: &[Event]) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let mut slots = BTreeMap::<u8, (Option<&Seat>, &Event)>::new();
    let mut host = None;
    for event in events {
        match event {
            Event::HostMigration { host_id } => host = Some(host_id),
            Event::Pause { .. } => {}
            Event::Join { player } => {
                slots.insert(player.seat, (Some(player), event));
            }
            Event::Leave { seat } | Event::Quit { seat } => {
                slots.insert(*seat, (None, event));
            }
            Event::Team { seat, .. } => {
                slots
                    .entry(*seat)
                    .and_modify(|(_, last)| *last = event)
                    .or_insert((None, event));
            }
        }
    }
    if let Some(host) = host {
        let machine = bots::lobby::game_session(g)
            .ok()
            .and_then(|s| host_machine(g, s));
        ensure!(
            machine.as_ref() == Some(host),
            "film host migration differs"
        );
    }
    if slots.is_empty() {
        return Ok(());
    }
    let rows = players::array(g)
        .map_err(|e| anyhow::anyhow!("film players: {e:?}"))?
        .map(|a| a.live(g, 16))
        .transpose()
        .map_err(|e| anyhow::anyhow!("film players: {e:?}"))?
        .unwrap_or_default();
    for (seat, (joined, event)) in slots {
        let row = rows
            .iter()
            .find(|(h, _)| h.index() == u16::from(seat))
            .map(|(_, p)| *p);
        if let Some(player) = joined {
            let row = row.context("film player did not join")?;
            let mut name = [0; players::NAME_UNITS];
            let n = row
                .name_units(g, &mut name)
                .map_err(|e| anyhow::anyhow!("film player name: {e:?}"))?;
            ensure!(
                player
                    .name
                    .encode_utf16()
                    .eq(live_proto::bots::unmarked_units(&name[..n]).iter().copied()),
                "film player joining seat {seat} differs"
            );
        }
        match event {
            Event::Leave { .. } => ensure!(row.is_none(), "film seat {seat} did not leave"),
            Event::Quit { .. } => ensure!(
                row.is_some_and(|p| p.flags(g).is_ok_and(|v| v & 2 != 0)),
                "film seat {seat} did not quit"
            ),
            Event::Team { team, .. } => ensure!(
                row.and_then(|p| p.team(g).ok()).map(i32::from) == Some(*team),
                "film seat {seat} team differs"
            ),
            Event::Join { player } => ensure!(
                row.and_then(|p| p.team(g).ok()).map(i32::from) == Some(player.team),
                "film seat {seat} join team differs"
            ),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_the_mounted_map_bytes_and_only_allows_guest_map_slots() {
        let dir =
            std::env::temp_dir().join(format!("film-match-map-{}", live_proto::sign::random_id()));
        std::fs::create_dir(&dir).unwrap();
        let iso = crate::map_override::tests::iso_at(&dir, b"film map on disc");
        let cfg = vfs::Config::new(dir.join("data"), 0x4d53_0064);
        let vfs = Mutex::new(vfs::Vfs::with_disc(cfg, &iso).unwrap());
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
        config.sim_store = Some(store);
        let sparse = Mutex::new(
            crate::map_override::mount(
                config,
                &iso,
                None::<crate::map_override::tests::Upper>,
                None,
                &[],
            )
            .unwrap(),
        );
        let sparse_id = map_identity(&sparse, "level", Some(path), None).unwrap();
        assert_eq!(sparse_id.ranges, Some(vec![(0, 4), (9, 4)]));
        assert_ne!(sparse_id.sha256, id.sha256);
        verify_map(&vfs, &sparse_id).unwrap();
        drop(sparse);
        let mut wrong = id;
        wrong.sha256 = "00".repeat(32);
        assert!(verify_map(&vfs, &wrong).is_err());
        assert!(map_identity(&vfs, "level", Some("t:\\profile"), None).is_err());
        drop(vfs);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
