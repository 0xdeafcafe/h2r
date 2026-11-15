use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use live_proto::game::Frame;
use serde::Serialize;
use serde_json::json;

use crate::event::GameEvent;

/// Output transport. Calls are made only by the queue worker, never by the
/// guest thread. `flush` batches events where the transport supports it.
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

pub struct QueuedSink {
    events: std::sync::mpsc::SyncSender<GameEvent>,
    frames: std::sync::mpsc::SyncSender<Frame>,
    stop: Arc<AtomicBool>,
    dropped_frames: Arc<AtomicU64>,
    error: Arc<Mutex<Option<String>>>,
    logged: AtomicBool,
    worker: Option<JoinHandle<()>>,
}

impl QueuedSink {
    pub fn new<S: Sink>(mut sink: S, event_capacity: usize, frame_capacity: usize) -> Self {
        let (event_tx, event_rx) =
            std::sync::mpsc::sync_channel::<GameEvent>(event_capacity.max(1));
        let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel::<Frame>(frame_capacity.max(1));
        let stop = Arc::new(AtomicBool::new(false));
        let dropped_frames = Arc::new(AtomicU64::new(0));
        let error = Arc::new(Mutex::new(None));
        let worker = {
            let stop = Arc::clone(&stop);
            let error = Arc::clone(&error);
            std::thread::Builder::new()
                .name("h2-telemetry-sink".into())
                .spawn(move || {
                    let mut failed = false;
                    while !stop.load(Ordering::Acquire) && !failed {
                        match event_rx.recv_timeout(Duration::from_millis(50)) {
                            Ok(event) => {
                                if let Err(e) = sink.event(event) {
                                    failed = true;
                                    *error.lock().unwrap() = Some(e);
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                        while !failed {
                            match event_rx.try_recv() {
                                Ok(event) => {
                                    if let Err(e) = sink.event(event) {
                                        failed = true;
                                        *error.lock().unwrap() = Some(e);
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        if let Err(e) = sink.flush() {
                            failed = true;
                            *error.lock().unwrap() = Some(e);
                        }
                        let mut latest = None;
                        while let Ok(frame) = frame_rx.try_recv() {
                            latest = Some(frame);
                        }
                        if let Some(frame) = latest
                            && let Err(e) = sink.frame(frame)
                        {
                            failed = true;
                            *error.lock().unwrap() = Some(e);
                        }
                    }
                    // Shutdown and failure still preserve every accepted event.
                    while let Ok(event) = event_rx.try_recv() {
                        if let Err(e) = sink.event(event) {
                            *error.lock().unwrap() = Some(e);
                            break;
                        }
                    }
                    let mut latest = None;
                    while let Ok(frame) = frame_rx.try_recv() {
                        latest = Some(frame);
                    }
                    if let Some(frame) = latest {
                        let _ = sink.frame(frame);
                    }
                    let _ = sink.flush();
                })
                .expect("spawn telemetry sink worker")
        };
        Self {
            events: event_tx,
            frames: frame_tx,
            stop,
            dropped_frames,
            error,
            logged: AtomicBool::new(false),
            worker: Some(worker),
        }
    }

    pub fn event(&self, event: GameEvent) -> Result<(), String> {
        if let Some(error) = self.error.lock().unwrap().clone() {
            return Err(error);
        }
        self.events.try_send(event).map_err(|e| match e {
            std::sync::mpsc::TrySendError::Full(_) => "telemetry event queue is full".into(),
            std::sync::mpsc::TrySendError::Disconnected(_) => "telemetry sink stopped".into(),
        })
    }

    pub fn frame(&self, frame: Frame) {
        if self.frames.try_send(frame).is_err() {
            self.dropped_frames.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames.load(Ordering::Relaxed)
    }

    pub fn worker_error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    pub fn log_error(&self, reason: &str) {
        if !self.logged.swap(true, Ordering::AcqRel) {
            eprintln!("live telemetry disabled: {reason}");
        }
    }
}

impl Drop for QueuedSink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(error) = self.worker_error() {
            self.log_error(&error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{GameInfo, PlayerState};
    use crate::session::Session;
    use live_proto::game::{FramePlayer, PlayerScore};

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
                },
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
                vehicle: None,
            }],
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

    struct BlockingSink {
        events: Arc<Mutex<Vec<GameEvent>>>,
        started: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        blocked: bool,
    }

    impl Sink for BlockingSink {
        fn event(&mut self, event: GameEvent) -> Result<(), String> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }

        fn frame(&mut self, _frame: Frame) -> Result<(), String> {
            if !self.blocked {
                self.blocked = true;
                self.started.send(()).unwrap();
                let _ = self.release.recv();
            }
            Ok(())
        }
    }

    #[test]
    fn backpressure_drops_frames_but_keeps_events() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let sink = BlockingSink {
            events: Arc::clone(&events),
            started: started_tx,
            release: release_rx,
            blocked: false,
        };
        let queue = QueuedSink::new(sink, 4, 1);
        let frame = |tick| Frame {
            game_id: "g".into(),
            tick,
            players: vec![],
        };
        queue.frame(frame(0));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        queue.frame(frame(2));
        queue.frame(frame(4));
        queue
            .event(GameEvent::new(
                "g",
                "ScoreChanged",
                live_proto::game::ScoreChanged {
                    tick: 4,
                    players: vec![PlayerScore {
                        index: 0,
                        score: 1,
                        kills: 1,
                        deaths: 0,
                        assists: 0,
                    }],
                    teams: None,
                },
            ))
            .unwrap();
        queue.frame(frame(6));
        assert!(queue.dropped_frames() >= 2);
        release_tx.send(()).unwrap();
        drop(queue);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "ScoreChanged");
    }
}
