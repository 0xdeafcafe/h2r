//! The event fields that can cross from a private machine tape to a web range.
//! Identity-bearing originals stay in a bounded private frame adjacent to TICK.
mod pose;

use super::{Summary, events, stream};
use crate::game;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

pub const PRIVATE_TAG: [u8; 4] = *b"IPRV";

pub struct Projection {
    pub public: Vec<u8>,
    pub private: Option<Vec<u8>>,
}

pub fn public_summary(summary: &mut Summary) {
    for player in &mut summary.players {
        player.xuid = None;
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Lifecycle {
    Join { player: DisplaySeat },
    Leave { seat: u8 },
    Quit { seat: u8 },
    Team { seat: u8, team: i32 },
    HostMigration {},
    Pause { paused: bool },
}

#[derive(Serialize, Deserialize)]
struct DisplaySeat {
    seat: u8,
    name: String,
    team: i32,
    kind: DisplayKind,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DisplayKind {
    Local,
    Remote,
    Phantom,
    Bot,
}

struct RecordView<'a> {
    public: Option<Cow<'a, [u8]>>,
    // Include this record in IPRV whenever the block needs one.
    private: bool,
}

impl<'a> RecordView<'a> {
    fn private() -> Self {
        Self {
            public: None,
            private: true,
        }
    }
}

fn json_event<T: Serialize>(subtype: u8, value: &T) -> Option<Vec<u8>> {
    let mut bytes = vec![subtype];
    serde_json::to_writer(&mut bytes, value).ok()?;
    Some(bytes)
}

// A new field on the private identity model must not become public just
// because serde learned its name. Copy only the game and display fields.
fn display_player(player: game::Player) -> game::Player {
    game::Player {
        index: player.index,
        name: player.name,
        team: player.team,
        bot: player.bot,
        local: player.local,
        appearance: player.appearance.map(|a| game::Appearance {
            primary: a.primary,
            secondary: a.secondary,
            tertiary: a.tertiary,
            quaternary: a.quaternary,
            emblem_foreground: a.emblem_foreground,
            emblem_background: a.emblem_background,
        }),
        xuid: None,
    }
}

fn display_game(game: game::NewGame) -> game::NewGame {
    game::NewGame {
        map: game.map,
        mode: game.mode,
        variant: game.variant,
        team_play: game.team_play,
        score_limit: game.score_limit,
        time_limit_s: game.time_limit_s,
        tick: game.tick,
        players: game.players.into_iter().map(display_player).collect(),
        kind: game.kind,
        playlist: game.playlist,
        playlist_id: game.playlist_id,
        difficulty: game.difficulty,
        online: game.online,
        map_id: game.map_id,
        skulls: game.skulls,
        ..Default::default()
    }
}

fn view<'a>(r: stream::Record<'a>, packer: Option<&mut crate::pack::Decoder>) -> RecordView<'a> {
    let keep = || RecordView {
        public: Some(Cow::Borrowed(r.payload)),
        private: false,
    };
    match r.tag {
        stream::EVENT => {
            let Some((&subtype, json)) = r.payload.split_first() else {
                return RecordView::private();
            };
            match subtype {
                stream::LIFECYCLE_EVENT | stream::CLOSING_EVENT => {
                    let Ok(events) = serde_json::from_slice::<Vec<Lifecycle>>(json) else {
                        return RecordView::private();
                    };
                    RecordView {
                        public: json_event(subtype, &events).map(Cow::Owned),
                        private: true,
                    }
                }
                stream::STATS_EVENT => {
                    let Ok(record) = events::Record::decode(json) else {
                        return RecordView::private();
                    };
                    use events::Event::*;
                    let event = match record.event {
                        GameStarted(start) => GameStarted(game::GameStarted {
                            game_id: start.game_id,
                            host: String::new(),
                            dedicated: Default::default(),
                            game: display_game(start.game),
                        }),
                        PlayerJoined(join) => PlayerJoined(game::PlayerJoined {
                            player: display_player(join.player),
                        }),
                        event => event,
                    };
                    let record = events::Record {
                        tick: record.tick,
                        event,
                    };
                    let import = match &record.event {
                        GameStarted(_) | PlayerJoined(_) | PlayerLeft(_) | PlayerKilled(_)
                        | ScoreChanged(_) | GameEnded(_) => true,
                        Assist { .. }
                        | Medal { .. }
                        | WeaponFire { .. }
                        | Damage(_)
                        | ActorKilled(_)
                        | Impact(_)
                        | ProjectileEnded(_)
                        | Pickup { .. }
                        | VehicleEntered { .. }
                        | VehicleExited { .. }
                        | ObjectiveScore { .. }
                        | ObjectiveState(_)
                        | PowerupTaken { .. } => false,
                    };
                    RecordView {
                        public: json_event(subtype, &record).map(Cow::Owned),
                        private: import,
                    }
                }
                _ => RecordView::private(),
            }
        }
        stream::INPUT if input_shape(r.payload) => keep(),
        stream::POSE if pose::check(r.payload, packer) => keep(),
        stream::HASH if r.payload.len() == 8 => keep(),
        stream::CLOCK if stream::decode_clock(r.payload).is_ok() => keep(),
        stream::MARKER if std::str::from_utf8(r.payload).is_ok() => keep(),
        _ => RecordView::private(),
    }
}

// Only gameplay masks and replacement bytes occur in an input record.
fn input_shape(mut bytes: &[u8]) -> bool {
    let Ok(changed) = stream::read_varint(&mut bytes) else {
        return false;
    };
    let Ok(changed) = u16::try_from(changed) else {
        return false;
    };
    for _ in 0..changed.count_ones() {
        let Some((&flags, tail)) = bytes.split_first() else {
            return false;
        };
        bytes = tail;
        if flags > 3 {
            return false;
        }
        for (bit, width) in [(1, 18usize), (2, 92)] {
            if flags & bit == 0 {
                continue;
            }
            let Some((mask, tail)) = bytes.split_at_checked(width.div_ceil(8)) else {
                return false;
            };
            if mask[mask.len() - 1] >> (width % 8) != 0 {
                return false;
            }
            let n: usize = mask.iter().map(|b| b.count_ones() as usize).sum();
            let Some(tail) = tail.get(n..) else {
                return false;
            };
            bytes = tail;
        }
    }
    bytes.is_empty()
}

/// Re-encode recognised JSON through the public schemas, discarding extension
/// fields by default. A private frame, when needed, contains the complete
/// lifecycle and stats-import sequences for this block in observation order.
pub fn project(raw: &[u8]) -> Result<Projection, String> {
    let chunk = stream::Chunk::parse(raw)?;
    let mut public = stream::ChunkWriter::new(chunk.first_tick, chunk.tick_count)?;
    let mut private = stream::ChunkWriter::new(chunk.first_tick, chunk.tick_count)?;
    let mut changed = false;
    for r in chunk.records() {
        let r = r?;
        let view = view(r, None);
        let differs = view.public.as_deref() != Some(r.payload);
        if view.private || differs {
            private.record(r.tick, r.tag, r.payload)?;
        }
        if let Some(payload) = view.public {
            public.record(r.tick, r.tag, &payload)?;
        }
        changed |= differs;
    }
    Ok(Projection {
        public: public.finish(),
        private: changed.then(|| private.finish()),
    })
}

/// Older writers could emit arbitrary JSON extensions in otherwise known
/// events. Only bytes produced by a recognised public schema are addressable.
pub fn is_public_tick(raw: &[u8]) -> bool {
    let mut packer = crate::pack::Decoder::new();
    stream::Chunk::parse(raw).is_ok_and(|chunk| {
        chunk.records().all(|r| {
            r.is_ok_and(|r| view(r, Some(&mut packer)).public.as_deref() == Some(r.payload))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{game::Frame, pack};

    fn block(records: &[&[u8]], tag: u8) -> Vec<u8> {
        let mut chunk = stream::ChunkWriter::new(1, 1).unwrap();
        for payload in records {
            chunk.record(1, tag, payload).unwrap();
        }
        chunk.finish()
    }

    #[test]
    fn unrecognised_fields_never_pass_through_a_known_event() {
        let mut join = vec![stream::LIFECYCLE_EVENT];
        join.extend(br#"[{"kind":"join","player":{"seat":1,"team":2,"kind":"remote","name":"Guest","future":{"endpoint":"198.51.100.17"}}}]"#);
        let mut stats = vec![stream::STATS_EVENT];
        stats.extend(
            br#"{"kind":"PlayerLeft","tick":1,"data":{"index":1,"address":"198.51.100.17"}}"#,
        );
        let raw = block(&[&join, &stats, b"\x7ffuture identity"], stream::EVENT);
        assert!(!is_public_tick(&raw));
        let result = project(&raw).unwrap();
        assert!(is_public_tick(&result.public));
        assert_eq!(result.private.unwrap(), raw);
        assert!(!result.public.windows(13).any(|s| s == b"198.51.100.17"));
        assert!(!result.public.windows(6).any(|s| s == b"future"));
    }

    #[test]
    fn extra_bytes_after_a_pose_are_never_a_public_range() {
        let baseline = [0, 0, 0, 0];
        assert!(is_public_tick(&block(&[&baseline], stream::POSE)));
        let mut trailing = baseline.to_vec();
        trailing.extend(b"secret host address");
        assert!(!is_public_tick(&block(&[&trailing], stream::POSE)));
        let mut enc = pack::Encoder::with_version(0, u64::MAX, 8).unwrap();
        let mut f = Frame {
            game_id: "demo".into(),
            tick: 1,
            ..Default::default()
        };
        let (k, _) = enc.encode(&f).unwrap();
        f.tick += 1;
        let (d, _) = enc.encode(&f).unwrap();
        let k = [vec![2], k].concat();
        let mut d = [vec![2], d].concat();
        assert!(is_public_tick(&block(&[&k, &d], stream::POSE)));
        assert!(!is_public_tick(&block(&[&d], stream::POSE)));
        d.extend(b"secret host address");
        assert!(!is_public_tick(&block(&[&k, &d], stream::POSE)));
    }
}
