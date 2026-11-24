//! `pair.preview`, `pair.audio` and `pair.result`: content put into the
//! paired game and read back from it (docs/editor/agent-api.md, "Previewing
//! content in the paired game").
//!
//! A preview is a job. `pair.preview` and `pair.audio` check their params,
//! start the job and answer with its id at once; `pair.result` reads it.
//! A job first lets the document's pending edits reach the game through
//! the pairing's own edit path (`mirror.apply send`, [`crate::play::Apply`]):
//! the window sends them between frames, so the job waits on the
//! [`crate::play::pair::Bridge`]'s pending rows, polled when `pair.result`
//! is asked. Then a thread of its own ([`run`]) opens a second connection
//! to the paired game's agent socket and makes the game calls in order:
//! `sim.give`, `sim.spawn`, `cheat.set`, `session.wait`, the reads and
//! `render.capture` (docs/runtime/agent-api.md). The window never waits on
//! the game.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use agent_proto::{Endpoint, RpcError};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::play::pair::Request;

mod run;

#[cfg(all(test, unix))]
mod tests;

pub use run::{Capture, Task};

/// How long pending edits may take to reach the game.
const SEND_WAIT: Duration = Duration::from_secs(15);
/// Jobs kept for `pair.result`.
const KEEP: usize = 8;

/// The preview provider: its jobs, newest last.
#[derive(Default)]
pub struct Preview {
    jobs: Vec<Job>,
    next: u64,
}

struct Job {
    id: u64,
    what: &'static str,
    target: Option<String>,
    started: Instant,
    /// The calls made so far, as `run` makes them.
    steps: Arc<Mutex<Vec<String>>>,
    edits: Value,
    state: State,
}

enum State {
    /// Waiting for the document's pending edits to reach the game.
    Sending { plan: run::Plan, deadline: Instant },
    Running { rx: mpsc::Receiver<Value> },
    Done(Value),
}

const RESULT_SHAPE: &str = r#"{"id":"integer","what":"weapon|object|ui|hud|audio","target":"string|null","state":"sending|running|done|failed","why?":"string","elapsed_ms":"integer","steps":"[string]","edits":"{pending,sent,left}","applied?":"object","reads?":"object","capture?":"{path,method,tick,present,width,height,mean_luma,dark_share,black}|null","audio?":"object","warnings?":"[string]"}"#;

const METHODS: &[MethodDef] = &[
    def(
        m::PAIR_PREVIEW,
        "Put content into the paired game and read it back: give the player a weapon, spawn an object in front of them, check the menu screen shown, or show the HUD; then wait, read the game state that matters and capture the frame. Pending edits are sent first. Answers with a job id at once; pair.result has the outcome",
        r#"{"what":"weapon|object|ui|hud","target?":"string","player?":"integer","pos?":"[x,y,z]","distance?":"number","keep?":"boolean","ticks?":"integer","view?":"render|screen","out?":"string","edits?":"send|skip"}"#,
        r#"{"id":"integer","state":"sending|running"}"#,
        r#"{"what":"weapon","target":"objects\\weapons\\rifle\\battle_rifle\\battle_rifle"}"#,
    ),
    def(
        m::PREVIEW_TARGET,
        "A document's preview switch: where its preview shows (studio, game for a game Studio started, attached for one someone else started), and what Preview in game asks of the paired game for its kind (`game`: the pair.preview params, or `why` it has none). With `target` the switch is set; a game target needs a paired game of that kind. The document is `doc` (a shell.tabs id) or the front one",
        r#"{"doc?":"string (a tab id from shell.tabs)","target?":"studio|game|attached"}"#,
        r#"{"doc":"string|null","target":"studio|game|attached","targets":"[{id,label,available,why?}]","game":"{what,target?}|null","why?":"string"}"#,
        "{}",
    ),
    def(
        m::PAIR_AUDIO,
        "Sample the paired game's mixer voices (audio.voices) for some seconds, after an optional game call that makes the sound: each voice's peak applied gain, the loudest, and the looping sounds playing. Answers with a job id at once; pair.result has the outcome",
        r#"{"seconds?":"number","every_ms?":"integer","call?":{"method":"string","params?":"any"}}"#,
        r#"{"id":"integer","state":"running"}"#,
        r#"{"seconds":3,"call":{"method":"pogo.test","params":{"trick":"backflip"}}}"#,
    ),
    def(
        m::PAIR_RESULT,
        "A pair.preview or pair.audio job: its state, the calls made so far and, once done, the game's answers, the capture and its brightness, and warnings",
        r#"{"id?":"integer"}"#,
        RESULT_SHAPE,
        "null",
    ),
];

impl Provider for Preview {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::PAIR_PREVIEW => self.preview(app, &p),
            m::PREVIEW_TARGET => self.target(app, &p),
            m::PAIR_AUDIO => self.audio(app, &p),
            m::PAIR_RESULT => self.result(app, &p),
            _ => return None,
        })
    }
}

/// The paired game's socket, from what the window published.
fn paired(app: &App) -> Result<(Endpoint, bool), RpcError> {
    let s = app.pairing.published.as_ref().ok_or_else(|| {
        err(
            code::REFUSED,
            "previews run in the editor window or h2-editor --serve: pair with a game first",
        )
    })?;
    if s["state"] != "connected" {
        return Err(err(
            code::REFUSED,
            "not paired with a game (pair.list, pair.connect)",
        ));
    }
    let socket = s["socket"]
        .as_str()
        .ok_or_else(|| err(code::REFUSED, "the pairing has no socket"))?;
    let endpoint = Endpoint::parse(socket).map_err(|e| err(code::REFUSED, e))?;
    Ok((endpoint, s["paused"] == true))
}

/// Rows of Pending the game has not taken yet and will (a conflict, a
/// refused write or one that needs a reload stays until someone acts).
fn waiting(app: &App) -> usize {
    app.pairing
        .pending
        .iter()
        .filter(|row| {
            row["conflict"] != true
                && row["refused"] != true
                && !crate::play::tag_sync::Effect::is_reload(row)
        })
        .count()
}

impl Preview {
    /// `preview.target`: read or set a document's preview switch.
    fn target(&mut self, app: &mut App, p: &P) -> Reply {
        use crate::app::preview_switch::PreviewTarget;
        let doc = if p.has("doc") {
            Some(super::shell::doc_param(app, p)?)
        } else {
            app.sync_tabs();
            app.front_doc()
        };
        if let Some(t) = p.opt_str("target")? {
            let target = PreviewTarget::parse(t)
                .ok_or_else(|| err(code::INVALID_PARAMS, "target is studio, game or attached"))?;
            let doc = doc.ok_or_else(|| err(code::INVALID_PARAMS, "no document is open"))?;
            app.set_preview_target(doc, target)
                .map_err(|why| err(code::REFUSED, why))?;
        }
        let targets: Vec<Value> = PreviewTarget::ALL
            .iter()
            .map(|t| match app.preview_target_available(*t) {
                Ok(()) => json!({"id": t.id(), "label": t.label(), "available": true}),
                Err(why) => json!({"id": t.id(), "label": t.label(), "available": false, "why": why}),
            })
            .collect();
        let mut out = json!({
            "doc": doc.map(|d| app.doc_id(d)),
            "target": doc.map_or(PreviewTarget::Studio, |d| app.preview_target(d)).id(),
            "targets": targets,
            "game": null,
        });
        match doc.map(|d| app.game_preview(d)) {
            None => out["why"] = json!("no document is open"),
            Some(Ok(g)) => out["game"] = g.params(),
            Some(Err(why)) => out["why"] = json!(why),
        }
        Ok(out)
    }

    fn preview(&mut self, app: &mut App, p: &P) -> Reply {
        let what = p.str("what")?;
        let target = p.opt_str("target")?.map(str::to_owned);
        let player = p.opt_u64("player")?.unwrap_or(0);
        if player > 15 {
            return Err(err(code::INVALID_PARAMS, "player is a player index, 0 to 15"));
        }
        let need_target = |what: &str| {
            target.clone().filter(|t| !t.is_empty()).ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    format!("a {what} preview needs a target: the tag's name or datum, as tag.list or the game's edit.tags lists it"),
                )
            })
        };
        let (task, what, settle) = match what {
            "weapon" => (
                Task::Weapon {
                    weapon: need_target("weapon")?,
                    keep: p.bool("keep")?,
                },
                "weapon",
                30,
            ),
            "object" => {
                let pos = p.opt_vec3("pos")?.map(|v| v.map(f64::from));
                let distance = p.opt_f32("distance")?.unwrap_or(3.0);
                if !(0.0..=100.0).contains(&distance) {
                    return Err(err(code::INVALID_PARAMS, "distance is 0 to 100 world units"));
                }
                (
                    Task::Object {
                        tag: need_target("object")?,
                        pos,
                        distance: f64::from(distance),
                    },
                    "object",
                    30,
                )
            }
            "ui" => (Task::Ui { screen: target.clone() }, "ui", 0),
            "hud" => (Task::Hud { state: target.clone() }, "hud", 30),
            _ => {
                return Err(err(
                    code::INVALID_PARAMS,
                    "what is weapon, object, ui or hud",
                ));
            }
        };
        let ticks = p.opt_u64("ticks")?.unwrap_or(settle);
        if ticks > 600 {
            return Err(err(code::INVALID_PARAMS, "ticks is 0 to 600"));
        }
        let capture = match p.opt_str("view")? {
            None | Some("render") => Capture::Render,
            Some("screen") => Capture::Screen,
            Some(_) => return Err(err(code::INVALID_PARAMS, "view is render or screen")),
        };
        let out = p.opt_str("out")?.map(PathBuf::from);
        let send = match p.opt_str("edits")? {
            None | Some("send") => true,
            Some("skip") => false,
            Some(_) => return Err(err(code::INVALID_PARAMS, "edits is send or skip")),
        };
        let (endpoint, paused) = paired(app)?;
        if paused && what != "ui" {
            return Err(err(
                code::REFUSED,
                "the paired game is paused: resume it (pair.control) to preview",
            ));
        }
        let plan = run::Plan {
            endpoint,
            player,
            task,
            ticks,
            capture,
            out,
        };
        self.start(app, what, target, plan, send)
    }

    fn audio(&mut self, app: &mut App, p: &P) -> Reply {
        let seconds = f64::from(p.opt_f32("seconds")?.unwrap_or(3.0));
        if !(0.1..=30.0).contains(&seconds) {
            return Err(err(code::INVALID_PARAMS, "seconds is 0.1 to 30"));
        }
        let every_ms = p.opt_u64("every_ms")?.unwrap_or(100);
        if !(20..=1000).contains(&every_ms) {
            return Err(err(code::INVALID_PARAMS, "every_ms is 20 to 1000"));
        }
        let call = match p.get("call") {
            None => None,
            Some(c) => {
                let method = c["method"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| err(code::INVALID_PARAMS, "call.method is a game method's name"))?;
                Some((method.to_owned(), c.get("params").cloned().unwrap_or(Value::Null)))
            }
        };
        let (endpoint, paused) = paired(app)?;
        if paused {
            return Err(err(
                code::REFUSED,
                "the paired game is paused: resume it (pair.control) to hear it",
            ));
        }
        let plan = run::Plan {
            endpoint,
            player: 0,
            task: Task::Audio {
                seconds,
                every: Duration::from_millis(every_ms),
                call,
            },
            ticks: 0,
            capture: Capture::None,
            out: None,
        };
        self.start(app, "audio", None, plan, false)
    }

    fn start(
        &mut self,
        app: &mut App,
        what: &'static str,
        target: Option<String>,
        plan: run::Plan,
        send: bool,
    ) -> Reply {
        if let Some(busy) = self
            .jobs
            .iter()
            .find(|j| !matches!(j.state, State::Done(_)))
        {
            return Err(err(
                code::REFUSED,
                format!(
                    "preview {} is still {}: read it with pair.result first",
                    busy.id,
                    busy.state_name()
                ),
            ));
        }
        let pending = app.pairing.pending.len();
        let waiting = waiting(app);
        let mut sent = false;
        if send && waiting > 0 {
            let published = app.pairing.published.as_ref().unwrap_or(&Value::Null);
            if published["live_edits"] != true {
                if let Some(why) = published["edit_refusal"].as_str() {
                    return Err(err(
                        code::REFUSED,
                        format!("{waiting} pending edits cannot reach the game: {why} (or preview with edits: skip)"),
                    ));
                }
                app.pairing
                    .requests
                    .push(Request::Apply(crate::play::Apply::Send));
            }
            sent = true;
        }
        self.next += 1;
        let id = self.next;
        let steps = Arc::new(Mutex::new(Vec::new()));
        let state = if sent {
            State::Sending {
                plan,
                deadline: Instant::now() + SEND_WAIT,
            }
        } else {
            State::Running {
                rx: run::spawn(plan, Arc::clone(&steps)),
            }
        };
        let job = Job {
            id,
            what,
            target,
            started: Instant::now(),
            steps,
            edits: json!({"pending": pending, "sent": sent, "left": []}),
            state,
        };
        let state_name = job.state_name();
        self.jobs.push(job);
        if self.jobs.len() > KEEP {
            self.jobs.remove(0);
        }
        Ok(json!({"id": id, "state": state_name}))
    }

    fn result(&mut self, app: &mut App, p: &P) -> Reply {
        self.advance(app);
        let job = match p.opt_u64("id")? {
            Some(id) => self
                .jobs
                .iter()
                .find(|j| j.id == id)
                .ok_or_else(|| err(code::INVALID_PARAMS, format!("no preview {id} (the last {KEEP} are kept)")))?,
            None => self
                .jobs
                .last()
                .ok_or_else(|| err(code::REFUSED, "no preview has run: pair.preview or pair.audio starts one"))?,
        };
        Ok(job.report())
    }

    /// Moves each job on: a send that has landed starts its run, a run
    /// that has finished is kept.
    fn advance(&mut self, app: &App) {
        for job in &mut self.jobs {
            match &job.state {
                State::Done(_) => {}
                State::Running { rx } => {
                    let done = match rx.try_recv() {
                        Ok(report) => Some(report),
                        Err(mpsc::TryRecvError::Empty) => None,
                        Err(mpsc::TryRecvError::Disconnected) => Some(json!({
                            "state": "failed",
                            "why": "the preview's thread stopped without an answer",
                        })),
                    };
                    if let Some(report) = done {
                        job.state = State::Done(report);
                    }
                }
                State::Sending { deadline, .. } => {
                    let left = waiting(app);
                    let asked = app
                        .pairing
                        .requests
                        .contains(&Request::Apply(crate::play::Apply::Send));
                    let landed = left == 0 && !asked;
                    if !landed && Instant::now() < *deadline {
                        continue;
                    }
                    job.edits["left"] = json!(app.pairing.pending.as_slice());
                    let State::Sending { plan, .. } =
                        std::mem::replace(&mut job.state, State::Done(Value::Null))
                    else {
                        unreachable!("matched above")
                    };
                    job.state = if landed {
                        State::Running {
                            rx: run::spawn(plan, Arc::clone(&job.steps)),
                        }
                    } else {
                        State::Done(json!({
                            "state": "failed",
                            "why": format!(
                                "{left} pending edits did not reach the game in {} s (mirror.pending)",
                                SEND_WAIT.as_secs()
                            ),
                        }))
                    };
                }
            }
        }
    }
}

impl Job {
    fn state_name(&self) -> &'static str {
        match &self.state {
            State::Sending { .. } => "sending",
            State::Running { .. } => "running",
            State::Done(v) if v["state"] == "failed" => "failed",
            State::Done(_) => "done",
        }
    }

    fn report(&self) -> Value {
        let mut out = match &self.state {
            State::Done(report) => report.clone(),
            _ => json!({}),
        };
        out["id"] = json!(self.id);
        out["what"] = json!(self.what);
        out["target"] = json!(self.target);
        out["state"] = json!(self.state_name());
        out["elapsed_ms"] = json!(self.started.elapsed().as_millis() as u64);
        out["steps"] = json!(*self.steps.lock().unwrap_or_else(|e| e.into_inner()));
        out["edits"] = self.edits.clone();
        let left = self.edits["left"].as_array().map_or(0, Vec::len);
        if left > 0 {
            let mut warnings = out["warnings"].as_array().cloned().unwrap_or_default();
            warnings.push(json!(format!(
                "{left} edits are not in the game: a conflict, a refused write or one that needs a reload (mirror.pending, mirror.apply)"
            )));
            out["warnings"] = json!(warnings);
        }
        out
    }
}
