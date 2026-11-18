use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use live_proto::game::Frame;
use serde::Serialize;
use serde_json::json;

use crate::event::GameEvent;

/// Output transport. Calls are made only by the pipeline's worker, never by
/// the guest thread. `flush` batches events where the transport supports it.
pub trait Sink: Send + 'static {
    fn event(&mut self, event: GameEvent) -> Result<(), String>;
    fn frame(&mut self, frame: Frame) -> Result<(), String>;
    fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }
}

pub struct JsonlSink {
    writer: BufWriter<File>,
}

impl JsonlSink {
    pub fn create(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    fn write(&mut self, value: impl Serialize) -> Result<(), String> {
        serde_json::to_writer(&mut self.writer, &value).map_err(|e| e.to_string())?;
        self.writer.write_all(b"\n").map_err(|e| e.to_string())
    }
}

impl Sink for JsonlSink {
    fn event(&mut self, event: GameEvent) -> Result<(), String> {
        self.write(json!({
            "type": event.kind,
            "game_id": event.game_id,
            "data": event.data,
        }))
    }

    fn frame(&mut self, frame: Frame) -> Result<(), String> {
        self.write(json!({ "type": "GameFrame", "data": frame }))
    }

    fn flush(&mut self) -> Result<(), String> {
        self.writer.flush().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{GameInfo, PlayerState};
    use crate::session::Session;
    use live_proto::game::FramePlayer;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "h2-telemetry-{name}-{}",
            live_proto::sign::random_id()
        ))
    }

    #[test]
    fn jsonl_writes_events_and_frames() {
        let path = temp("capture.jsonl");
        let mut sink = JsonlSink::create(&path).unwrap();
        let mut session = Session::new("test", None);
        session.start(
            &GameInfo {
                map: "lockout".into(),
                mode: "multiplayer".into(),
                difficulty: None,
                skulls: None,
                variant: Some("Slayer".into()),
                team_play: false,
                score_limit: Some(25),
                time_limit_s: Some(600),
                tick_rate: 30,
                tick: 12,
            },
            vec![PlayerState {
                datum: 0x80010000,
                player: live_proto::game::Player {
                    index: 0,
                    name: "A".into(),
                    xuid: None,
                    team: 0,
                    local: Some(true),
                    appearance: None,
                    bot: None,
                },
                appearance: crate::event::Appearance::default(),
                local_user: None,
                unit: u32::MAX,
                stats: crate::event::PlayerStats::ZERO,
            }],
        );
        for output in session.take_output() {
            match output {
                crate::event::Output::Event(event) => sink.event(event).unwrap(),
                crate::event::Output::Frame(frame) => sink.frame(frame).unwrap(),
            }
        }
        sink.frame(Frame {
            game_id: "g".into(),
            tick: 14,
            players: vec![FramePlayer {
                index: 0,
                alive: false,
                pos: [0.0; 3],
                yaw: 0.0,
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        sink.flush().unwrap();
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["type"], "GameStarted");
        assert_eq!(rows[0]["data"]["tick"], 12);
        assert_eq!(rows[0]["data"]["score_limit"], 25);
        assert_eq!(rows[0]["data"]["players"][0]["name"], "A");
        assert_eq!(rows[1]["type"], "GameFrame");
        assert_eq!(rows[1]["data"]["tick"], 14);
        let _ = std::fs::remove_file(path);
    }
}
