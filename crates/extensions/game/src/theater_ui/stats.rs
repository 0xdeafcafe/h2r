//! Theater reads match results and kill locations from the public stats stream
//! beside the recording. Camera subjects come from the film's separate roster.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{ErrorKind, Read, Seek};
use std::path::Path;

use anyhow::{Result, ensure};
use live_proto::film::{events, stream};
use live_proto::game;

use super::adapter::FilmMeta;

fn open(recording: &Path) -> Result<Option<File>> {
    match File::open(recording.with_extension("h2stats")) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The browser only needs the bounded front matter for its scoreboard.
pub fn read_summary(recording: &Path, row: &mut FilmMeta) -> Result<()> {
    let Some(mut file) = open(recording)? else {
        return Ok(());
    };
    let front = film::range::read_front(&mut file)?;
    ensure!(front.id == row.id, "stats file belongs to another film");
    row.apply_summary(front.summary);
    Ok(())
}

/// The playback window reads the event chunks once when it opens. Ticks are
/// from the envelope's film clock, including after a revert in campaign.
pub fn read_playback(recording: &Path, row: &mut FilmMeta) -> Result<()> {
    let Some(file) = open(recording)? else {
        return Ok(());
    };
    let mut reader = film::range::RangeReader::new(file)?;
    ensure!(
        reader.front.id == row.id,
        "stats file belongs to another film"
    );
    let observations = Observations::scan(&mut reader)?;
    row.apply_summary(reader.front.summary);
    row.kill_ticks = observations.kills;
    for p in &mut row.scoreboard {
        if let Some(seat) = p.seat {
            if let Some(s) = observations.scores.get(&u32::from(seat)) {
                p.kills = Some(s.kills);
                p.deaths = Some(s.deaths);
            }
            p.emblem_colour = observations
                .colours
                .get(&(u32::from(seat), p.name.clone()))
                .copied();
        }
    }
    for p in &mut row.roster {
        if let Some(seat) = p.seat {
            p.emblem_colour = observations
                .colours
                .get(&(u32::from(seat), p.name.clone()))
                .copied();
        }
    }
    Ok(())
}

// The same fallback palette the web viewer uses when art includes no palette.
const EMBLEM_COLOURS: [[u8; 3]; 18] = [
    [236, 236, 236],
    [110, 110, 110],
    [28, 28, 28],
    [190, 40, 34],
    [226, 106, 30],
    [240, 190, 40],
    [126, 150, 60],
    [40, 130, 60],
    [246, 150, 170],
    [130, 60, 150],
    [40, 150, 160],
    [110, 180, 230],
    [36, 70, 160],
    [130, 90, 200],
    [120, 80, 44],
    [200, 160, 110],
    [150, 30, 70],
    [70, 90, 70],
];

#[derive(Default)]
struct Observations {
    kills: Vec<u64>,
    scores: BTreeMap<u32, game::PlayerScore>,
    colours: BTreeMap<(u32, String), [u8; 3]>,
}

impl Observations {
    fn scan<R: Read + Seek>(reader: &mut film::range::RangeReader<R>) -> Result<Self> {
        let mut observations = Self::default();
        for n in 0..reader.directory.chunks.len() {
            observations.chunk(&reader.chunk(n)?)?;
        }
        observations.kills.sort_unstable();
        observations.kills.dedup();
        Ok(observations)
    }

    fn chunk(&mut self, bytes: &[u8]) -> Result<()> {
        let chunk = stream::Chunk::parse(bytes).map_err(anyhow::Error::msg)?;
        for row in chunk.records() {
            let row = row.map_err(anyhow::Error::msg)?;
            if row.tag != stream::EVENT {
                continue;
            }
            let Some((&stream::STATS_EVENT, payload)) = row.payload.split_first() else {
                continue;
            };
            let Some((_, kind, value)) =
                events::Record::decode_telemetry(payload).map_err(anyhow::Error::msg)?
            else {
                continue;
            };
            match kind {
                game::PLAYER_KILLED => self.kills.push(row.tick),
                game::PLAYER_JOINED => {
                    let join: game::PlayerJoined = serde_json::from_value(value)?;
                    self.seat_colour(join.player);
                }
                game::GAME_STARTED => {
                    let start: game::GameStarted = serde_json::from_value(value)?;
                    for player in start.game.players {
                        self.seat_colour(player);
                    }
                }
                game::GAME_ENDED => {
                    let end: game::GameEnded = serde_json::from_value(value)?;
                    self.scores = end.players.into_iter().map(|p| (p.index, p)).collect();
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn seat_colour(&mut self, player: game::Player) {
        if let Some(colour) = player
            .appearance
            .and_then(|a| a.tertiary)
            .and_then(|colour| EMBLEM_COLOURS.get(usize::from(colour)))
        {
            self.colours.insert((player.index, player.name), *colour);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_stats_file_has_no_scoreboard_or_kills() {
        let dir =
            std::env::temp_dir().join(format!("h2stats-test-{}", live_proto::sign::random_id()));
        let mut row = FilmMeta {
            id: "synthetic".into(),
            ..Default::default()
        };
        let recording = dir.join("synthetic.h2film");
        read_summary(&recording, &mut row).unwrap();
        read_playback(&recording, &mut row).unwrap();
        assert!(row.scoreboard.is_empty() && row.kill_ticks.is_empty());
    }

    #[test]
    fn only_stats_kill_events_mark_the_timeline_in_the_envelope_clock() {
        let kill = serde_json::json!({"tick": 7, "kind": "PlayerKilled", "data": {
            "tick": 7, "victim": 2, "kind": "kill"
        }});
        let mut payload = vec![stream::STATS_EVENT];
        payload.extend(serde_json::to_vec(&kill).unwrap());
        let mut chunk = stream::ChunkWriter::new(1000, 50).unwrap();
        chunk.record(1001, stream::POSE, &payload).unwrap();
        chunk.record(1010, stream::EVENT, &payload).unwrap();
        chunk.record(1030, stream::MARKER, &payload).unwrap();
        let mut observed = Observations::default();
        observed.chunk(&chunk.finish()).unwrap();
        assert_eq!(observed.kills, [1010]);
    }
}
