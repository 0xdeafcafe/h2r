//! Agent diagnostics for the guest's looping sounds and the host's playout.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use agent::{Access, TagNames};
use agent_proto::{RpcError, code, method};
use dsound::output::OutputHealthProbe;
use serde::Serialize;
use serde_json::{Value, json};

use crate::audio_loops::{self, Track};
use crate::dsound_glue::Audio;
use crate::menu_music::MenuMusic;

const HISTORY_LIMIT: usize = 64;
type TrackKey = (u32, u8, u32);

#[derive(Clone, Copy, Serialize)]
struct SoundStamp {
    tick: Option<u64>,
    present: u64,
    logical_100ns: Option<u64>,
    elapsed_us: u64,
}

#[derive(Clone)]
struct Transition {
    at: SoundStamp,
    from: Option<Track>,
    to: Option<Track>,
}

#[derive(Default)]
struct History {
    map: Option<String>,
    last: Option<SoundStamp>,
    tracks: BTreeMap<TrackKey, Track>,
    transitions: VecDeque<Transition>,
    menu: Option<MenuMusic>,
    menu_changes: VecDeque<(SoundStamp, MenuMusic)>,
    stream: audio::StreamHealth,
    last_stream_starve_us: Option<u64>,
}

impl History {
    fn observe(
        &mut self,
        map: Option<String>,
        at: SoundStamp,
        tracks: Option<Vec<Track>>,
        menu: Option<MenuMusic>,
    ) {
        let rewound = self.last.is_some_and(|last| {
            at.logical_100ns
                .zip(last.logical_100ns)
                .is_some_and(|(new, old)| new < old)
                || at.tick.zip(last.tick).is_some_and(|(new, old)| new < old)
        });
        if map != self.map || rewound {
            self.transitions.clear();
            self.tracks.clear();
            self.menu_changes.clear();
            self.menu = None;
        }
        self.map = map;
        self.last = Some(at);
        if let Some(menu) = menu
            && self
                .menu
                .as_ref()
                .is_none_or(|previous| menu.changed_since(previous))
        {
            if self.menu_changes.len() == HISTORY_LIMIT {
                self.menu_changes.pop_front();
            }
            self.menu_changes.push_back((at, menu));
            self.menu = Some(menu);
        }
        if let Some(tracks) = tracks {
            self.observe_tracks(at, tracks);
        }
    }

    fn observe_tracks(&mut self, at: SoundStamp, tracks: Vec<Track>) {
        let mut previous = std::mem::take(&mut self.tracks);
        for track in tracks {
            let key = (track.loop_datum, track.track, track.sound_instance);
            let from = previous.remove(&key);
            if from.as_ref().is_none_or(|old| {
                old.tag_datum != track.tag_datum
                    || old.segment != track.segment
                    || old.sound_datum != track.sound_datum
                    || old.pitch_range != track.pitch_range
                    || old.permutation != track.permutation
            }) {
                self.push(Transition {
                    at,
                    from,
                    to: Some(track.clone()),
                });
            }
            self.tracks.insert(key, track);
        }
        for track in previous.into_values() {
            self.push(Transition {
                at,
                from: Some(track),
                to: None,
            });
        }
    }

    fn push(&mut self, change: Transition) {
        if self.transitions.len() == HISTORY_LIMIT {
            self.transitions.pop_front();
        }
        self.transitions.push_back(change);
    }

    fn observe_stream(&mut self, stream: audio::StreamHealth, elapsed_us: u64) {
        match stream.starves.cmp(&self.stream.starves) {
            std::cmp::Ordering::Greater => self.last_stream_starve_us = Some(elapsed_us),
            std::cmp::Ordering::Less => self.last_stream_starve_us = None,
            std::cmp::Ordering::Equal => {}
        }
        self.stream = stream;
    }
}

#[derive(Clone)]
struct Monitor {
    history: Arc<Mutex<History>>,
    mixer: Arc<Mutex<audio::Mixer>>,
    hle: Arc<dsound::Hle>,
    output: OutputHealthProbe,
}

impl Monitor {
    fn history(&self) -> std::sync::MutexGuard<'_, History> {
        self.history.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn read_stream(&self) -> audio::StreamHealth {
        self.mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stream_health()
    }

    fn add_playheads(&self, tracks: &mut [Track]) {
        if tracks.iter().all(|track| track.ds_stream.is_none()) {
            return;
        }
        let sound = self.hle.direct_sound();
        for track in tracks {
            if let Some((packet, position)) =
                track.ds_stream.and_then(|ptr| sound.stream_playhead(ptr))
            {
                track.play_packet = Some(packet);
                track.play_position_frames = Some(position);
            }
        }
    }
}

impl hooks::Feature for Monitor {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            "agent-audio",
            "Agent API: records looping-sound transitions",
        )
        .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let monitor = self.clone();
        // Menus service sound even when the simulation is not ticking.
        b.raw()
            .observe_after(
                &engine::points::SOUND_UPDATE,
                "looping-sound transitions",
                move |o| {
                    let (map, tick, mut tracks, menu) = {
                        let g = o.guest();
                        (
                            crate::telemetry::loaded_map_name(&g)
                                .ok()
                                .filter(|name| !name.is_empty()),
                            crate::telemetry::game_tick(&g).ok(),
                            audio_loops::read(&*g),
                            MenuMusic::read(&*g).ok().flatten(),
                        )
                    };
                    if let Ok(tracks) = &mut tracks {
                        monitor.add_playheads(tracks);
                    }
                    let at = SoundStamp {
                        tick,
                        present: crate::presents::presents(),
                        logical_100ns: o.clock(),
                        elapsed_us: monitor.output.elapsed_us(),
                    };
                    let stream = monitor.read_stream();
                    let mut history = monitor.history();
                    history.observe_stream(stream, at.elapsed_us);
                    history.observe(map, at, tracks.ok(), menu);
                },
            )
            .last();
        Ok(())
    }
}

pub fn install(
    registry: &mut hooks::Registry,
    agent: &agent::Agent,
    audio: &Audio,
    mut tags: impl TagNames + 'static,
) -> Result<(), hooks::BuildError> {
    let monitor = Monitor {
        history: Arc::default(),
        mixer: audio.mixer(),
        hle: Arc::clone(&audio.hle),
        output: audio.output.health_probe(),
    };
    let records_transitions = registry.add(&monitor)?;
    let wma = audio.wma.clone();
    agent.add_game_method(method::AUDIO_LOOPS, Access::Read, move |params, ctx| {
        validate_params(&params)?;
        let mut tracks = Err("guest memory unavailable".to_owned());
        let mut menu = None;
        ctx.reader.with_guest(&mut |g| {
            tracks = audio_loops::read(g)
                .map_err(|e| format!("looping sounds: guest memory at {:#x} faulted", e.addr));
            menu = MenuMusic::read(g).ok().flatten();
        });
        let mut tracks = tracks.map_err(|message| RpcError::new(code::UNAVAILABLE, message))?;
        monitor.add_playheads(&mut tracks);
        let output = monitor.output.read();
        let stream = monitor.read_stream();
        let (changes, menu_changes, last_stream_starve_us) = {
            let mut history = monitor.history();
            history.observe_stream(stream, output.elapsed_us);
            let (changes, menu_changes) = if history.map == ctx.at.map
                && history.last.and_then(|last| last.tick).zip(ctx.at.tick).is_none_or(|(old, now)| old <= now)
            {
                (
                    history.transitions.iter().cloned().collect::<Vec<_>>(),
                    history.menu_changes.iter().copied().collect::<Vec<_>>()
                )
            } else {
                (Vec::new(), Vec::new())
            };
            (changes, menu_changes, history.last_stream_starve_us)
        };
        let mut datums = tracks.iter().map(|t| t.tag_datum).collect::<Vec<_>>();
        for change in &changes {
            datums.extend(change.from.iter().chain(&change.to).map(|t| t.tag_datum));
        }
        datums.sort_unstable();
        datums.dedup();
        let names = if datums.is_empty() { Vec::new() } else { tags.names(&datums) };
        let name_track = |track: &Track| -> Value {
            let mut row = json!(track);
            row["tag_name"] = json!(datums.binary_search(&track.tag_datum)
                .ok().and_then(|index| names.get(index)).and_then(|name| name.as_deref()));
            row
        };
        let loops = tracks.iter().map(&name_track).collect::<Vec<_>>();
        let transitions = changes.iter().map(|change| json!({
            "at": change.at,
            "from": change.from.as_ref().map(&name_track),
            "to": change.to.as_ref().map(&name_track)
        })).collect::<Vec<_>>();
        let menu_changes = menu_changes.iter().map(|(at, state)| json!({
            "at": at,
            "state": state
        })).collect::<Vec<_>>();
        Ok(json!({
            "loops": loops,
            "records_transitions": records_transitions,
            "transitions": transitions,
            "menu": menu,
            "menu_changes": menu_changes,
            "output": {
                "elapsed_us": output.elapsed_us,
                "sink": output.sink.map(|s| json!({
                    "underruns": s.underruns, "underrun_frames": s.underrun_frames,
                    "last_underrun_us": output.last_underrun_us
                })),
                "playout": output.playout.map(|p| json!({
                    "starves": p.starves, "last_starve_us": output.last_playout_starve_us,
                    "primed": p.primed, "fill_ms": p.fill_ms,
                    "target_ms": p.target_ms, "trim_ppm": p.trim_ppm,
                })),
                "dropped_frames": output.dropped_frames,
                "last_drop_us": output.last_drop_us,
                "stream": {
                    "starves": stream.starves, "starve_frames": stream.starve_frames,
                    "last_starve_us": last_stream_starve_us
                },
                "wma": wma.as_ref().map(|codec| {
                    let s = codec.stats();
                    json!({ "restarts": s.restarts, "errors": s.errors, "dropped_partials": s.dropped_partials })
                })
            }
        }))
    });
    Ok(())
}

fn validate_params(params: &Value) -> Result<(), RpcError> {
    if params.is_null() || params.as_object().is_some_and(|p| p.is_empty()) {
        return Ok(());
    }
    Err(RpcError::new(
        code::INVALID_PARAMS,
        "audio.loops takes no params",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_loops::Segment;

    #[test]
    fn frame_2300_records_a_segment_change_and_ignores_packet_boundaries() {
        let mut history = History::default();
        let mut track = Track {
            loop_datum: 1,
            tag_datum: 2,
            track: 0,
            segment: Segment::In,
            sound_instance: 3,
            sound_datum: 4,
            pitch_range: Some(0),
            permutation: Some(0),
            chunk: Some(0),
            ds_stream: Some(5),
            play_packet: Some(20),
            play_position_frames: Some(200),
        };
        let stamp = |present| SoundStamp {
            present,
            tick: Some(present / 2),
            logical_100ns: Some(present * 10_000_000 / 60),
            elapsed_us: present * 1_000_000 / 60,
        };
        history.observe(None, stamp(2298), Some(vec![track.clone()]), None);
        track.play_packet = Some(21);
        track.play_position_frames = Some(1);
        history.observe(None, stamp(2299), Some(vec![track.clone()]), None);
        assert_eq!(history.transitions.len(), 1, "only the initial start");

        track.segment = Segment::Loop;
        track.sound_datum = 6;
        track.permutation = Some(2);
        history.observe(None, stamp(2300), Some(vec![track.clone()]), None);
        let transition = history.transitions.back().unwrap();
        assert_eq!(transition.at.present, 2300);
        assert_eq!(transition.at.tick, Some(1150));
        let from = transition.from.as_ref().unwrap();
        assert_eq!(from.segment, Segment::In);
        assert_eq!(from.play_packet, Some(21));
        assert_eq!(transition.to.as_ref(), Some(&track));
    }

    #[test]
    fn stream_starve_stamp_changes_only_when_the_counter_changes() {
        let mut history = History::default();
        history.observe_stream(audio::StreamHealth::default(), 500);
        assert_eq!(history.last_stream_starve_us, None);
        let starve = audio::StreamHealth {
            starves: 1,
            starve_frames: 30,
            ..Default::default()
        };
        history.observe_stream(starve, 600);
        history.observe_stream(starve, 700);
        assert_eq!(history.last_stream_starve_us, Some(600));
        history.observe_stream(audio::StreamHealth::default(), 800);
        assert_eq!(history.last_stream_starve_us, None);
    }
}
