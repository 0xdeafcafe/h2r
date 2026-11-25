//! A preview's game calls, on a thread and a connection of its own
//! ([`spawn`]), in the order docs/editor/agent-api.md gives them. Every
//! call is a game agent method (docs/runtime/agent-api.md); a method the
//! game does not list is skipped with a note, or ends the job when the
//! preview needs it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use agent_proto::{CallError, Client, Endpoint};
use serde_json::{Map, Value, json};

/// What to put into the game.
#[derive(Clone, Debug, PartialEq)]
pub enum Task {
    /// `sim.give` the weapon (a `weap` tag's name or datum).
    Weapon { weapon: String, keep: bool },
    /// `sim.spawn` the object at `pos`, else `distance` in front of the
    /// player, facing them.
    Object {
        tag: String,
        pos: Option<[f64; 3]>,
        distance: f64,
    },
    /// `sim.spawn` the object as `Object` does, `world.object.animate` it
    /// to play `animation` (a name in the object's own graph), let
    /// `ticks` run and capture. `tag` carries its group (`scen:name`).
    Animation {
        tag: String,
        animation: String,
        pos: Option<[f64; 3]>,
        distance: f64,
    },
    /// Read the menu shown; `screen` is the one expected.
    Ui { screen: Option<String> },
    /// Show the HUD (`hide_hud` off for the capture); `state` names a
    /// `<state>.state` method to read with it (`superbounce`, `pogo`).
    Hud { state: Option<String> },
    /// Sample `audio.voices`, after `call`.
    Audio {
        seconds: f64,
        every: Duration,
        call: Option<(String, Value)>,
    },
}

/// Which picture of the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capture {
    /// `render.capture`: the frame of the next tick (headless too).
    Render,
    /// `host.screenshot`: the window's newest frame, as the player sees it.
    Screen,
    None,
}

/// A preview's whole input.
#[derive(Clone, Debug)]
pub struct Plan {
    pub endpoint: Endpoint,
    pub player: u64,
    pub task: Task,
    /// Ticks to let run before the reads and the capture.
    pub ticks: u64,
    pub capture: Capture,
    /// Where the capture is copied.
    pub out: Option<PathBuf>,
    /// What Studio measured itself, put in the report as `editor` (a
    /// sound's `sound.measure`).
    pub editor: Option<Value>,
}

/// Starts `plan` on its own thread; the report arrives on the receiver.
pub fn spawn(plan: Plan, steps: Arc<Mutex<Vec<String>>>) -> mpsc::Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("h2-editor-preview".into())
        .spawn(move || {
            let _ = tx.send(run(&plan, &steps));
        });
    if let Err(e) = spawned {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(json!({"state": "failed", "why": format!("no thread for the preview: {e}")}));
        return rx;
    }
    rx
}

/// Runs `plan` against the paired game; the report `pair.result` shows.
pub fn run(plan: &Plan, steps: &Arc<Mutex<Vec<String>>>) -> Value {
    let mut g = Game {
        client: None,
        methods: Vec::new(),
        steps: Arc::clone(steps),
        out: Map::new(),
        warnings: Vec::new(),
    };
    if let Some(editor) = &plan.editor {
        g.out.insert("editor".into(), editor.clone());
    }
    let outcome = g.connect(&plan.endpoint).and_then(|()| g.task(plan));
    let mut report = Value::Object(std::mem::take(&mut g.out));
    match outcome {
        Ok(()) => report["state"] = json!("done"),
        Err(why) => {
            report["state"] = json!("failed");
            report["why"] = json!(why);
        }
    }
    report["warnings"] = json!(g.warnings);
    report
}

struct Game {
    client: Option<Client>,
    methods: Vec<String>,
    steps: Arc<Mutex<Vec<String>>>,
    out: Map<String, Value>,
    warnings: Vec<String>,
}

type Step = Result<(), String>;

impl Game {
    fn connect(&mut self, endpoint: &Endpoint) -> Step {
        let client = Client::connect_with_timeout(endpoint, Some(Duration::from_secs(5)))
            .map_err(|e| format!("connecting to the paired game at {endpoint}: {e}"))?;
        client
            .set_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| format!("{endpoint}: {e}"))?;
        self.client = Some(client);
        let methods = self.call(agent_proto::method::METHODS, Value::Null)?;
        self.methods = serde_json::from_value(methods)
            .map_err(|_| "session.methods answered something other than a list of names".to_owned())?;
        Ok(())
    }

    fn has(&self, method: &str) -> bool {
        self.methods.iter().any(|m| m == method)
    }

    fn need(&self, method: &str, what: &str) -> Step {
        if self.has(method) {
            Ok(())
        } else {
            Err(format!("the paired game does not list {method} ({what})"))
        }
    }

    fn note(&self, step: String) {
        self.steps.lock().unwrap_or_else(|e| e.into_inner()).push(step);
    }

    /// One game call, noted in the steps.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.note(method.to_owned());
        let client = self.client.as_mut().ok_or("not connected")?;
        client.call(method, params).map_err(|e| match e {
            CallError::Rpc(e) => format!("{method}: {}", e.message),
            e => format!("{method}: {e}"),
        })
    }

    /// A read the preview can do without: `None` (and a warning) when the
    /// game does not list it or refuses it.
    fn read(&mut self, method: &str, params: Value) -> Option<Value> {
        if !self.has(method) {
            return None;
        }
        match self.call(method, params) {
            Ok(v) => Some(v),
            Err(why) => {
                self.warnings.push(why);
                None
            }
        }
    }

    fn task(&mut self, plan: &Plan) -> Step {
        let player = format!("player.{}", plan.player);
        match &plan.task {
            Task::Weapon { weapon, keep } => self.weapon(plan, &player, weapon, *keep),
            Task::Object { tag, pos, distance } => self.object(plan, &player, tag, *pos, *distance),
            Task::Animation { tag, animation, pos, distance } => {
                self.animation(plan, &player, tag, animation, *pos, *distance)
            }
            Task::Ui { screen } => self.ui(plan, screen.as_deref()),
            Task::Hud { state } => self.hud(plan, state.as_deref()),
            Task::Audio {
                seconds,
                every,
                call,
            } => self.audio(*seconds, *every, call.as_ref()),
        }
    }

    fn weapon(&mut self, plan: &Plan, player: &str, weapon: &str, keep: bool) -> Step {
        self.need(agent_proto::sim::method::GIVE, "giving a weapon")?;
        let mut params = json!({"player": player, "weapon": weapon});
        if keep {
            params["keep"] = json!(true);
        }
        let given = self.call(agent_proto::sim::method::GIVE, params)?;
        self.out.insert("applied".into(), given.clone());
        self.settle(plan.ticks)?;
        let tag = hex_of(&given["weapon"]["tag"]);
        let name = given["weapon"]["name"].as_str().unwrap_or(weapon).to_owned();
        let me = self.read(agent_proto::method::WORLD_OBJECT, json!({"id": player}));
        let held_tag = me
            .as_ref()
            .and_then(|v| v["object"]["weapon"].as_str())
            .and_then(|s| hex_of(&json!(s)));
        // Unknown without the player's read; a player holding nothing
        // does not hold it.
        let held = match (&tag, &me) {
            (Some(want), Some(_)) => Some(held_tag.as_ref() == Some(want)),
            _ => None,
        };
        if held == Some(false) {
            self.warnings.push(format!(
                "{player} does not hold {name} after {} ticks: it holds tag {}",
                plan.ticks,
                held_tag.as_deref().unwrap_or("none")
            ));
        }
        let mut reads = json!({
            "weapon": {
                "object": given["object"], "tag": tag, "name": name,
                "held": held, "held_tag": held_tag,
            },
        });
        let pogo = self.pogo(tag.as_deref(), &given["object"]);
        let chase = pogo
            .as_ref()
            .is_some_and(|p| p["stick"] == true && p["camera"]["drawn"] == true);
        if let Some(p) = pogo {
            reads["pogo"] = p;
        }
        if let Some(camera) = self.camera() {
            if camera["first_person"]["drawn"] == false && !chase {
                self.warnings.push(
                    "no first-person view drew anything: the held weapon has no first-person model on screen".into(),
                );
            }
            reads["camera"] = camera;
        }
        self.out.insert("reads".into(), reads);
        self.capture(plan)
    }

    fn object(
        &mut self,
        plan: &Plan,
        player: &str,
        tag: &str,
        pos: Option<[f64; 3]>,
        distance: f64,
    ) -> Step {
        let (made, at) = self.spawn(player, tag, pos, distance)?;
        self.out.insert("applied".into(), made.clone());
        self.settle(plan.ticks)?;
        let found = self.find_made(&made, at);
        let mut reads = json!({"object": found});
        if let Some(camera) = self.camera() {
            reads["camera"] = camera;
        }
        self.out.insert("reads".into(), reads);
        self.capture(plan)
    }

    fn animation(
        &mut self,
        plan: &Plan,
        player: &str,
        tag: &str,
        animation: &str,
        pos: Option<[f64; 3]>,
        distance: f64,
    ) -> Step {
        // Before anything is made: a game without the method gets no object.
        self.need(agent_proto::sim::method::OBJECT_ANIMATE, "starting an animation")?;
        let kind = tag
            .split_once(':')
            .and_then(|(group, _)| kind_of_group(group))
            .ok_or_else(|| {
                format!(
                    "an animation preview spawns scen, vehi, mach, bloc or crea objects: name the tag with its group ({tag:?} has none of those)"
                )
            })?;
        let (made, at) = self.spawn(player, tag, pos, distance)?;
        let handle = made["object"].as_str().unwrap_or_default().to_ascii_lowercase();
        let id = format!("{kind}.{handle}");
        let started = self.call(
            agent_proto::sim::method::OBJECT_ANIMATE,
            json!({"id": id, "animation": animation}),
        )?;
        self.out.insert("applied".into(), json!({"spawn": made, "animate": started}));
        self.settle(plan.ticks)?;
        let found = self.find_made(&made, at);
        let mut reads = json!({"object": found, "animation": started});
        if let Some(camera) = self.camera() {
            reads["camera"] = camera;
        }
        self.out.insert("reads".into(), reads);
        self.capture(plan)
    }

    /// `sim.spawn` the tag at `pos`, else `distance` in front of the
    /// player facing them: the game's answer and where it was made.
    fn spawn(
        &mut self,
        player: &str,
        tag: &str,
        pos: Option<[f64; 3]>,
        distance: f64,
    ) -> Result<(Value, Value), String> {
        self.need(agent_proto::sim::method::SPAWN, "making an object")?;
        let (pos, yaw) = match pos {
            Some(pos) => (pos, None),
            None => {
                self.need(agent_proto::method::WORLD_OBJECT, "finding the player")?;
                let me = self.call(agent_proto::method::WORLD_OBJECT, json!({"id": player}))?;
                in_front(&me["object"], distance)
                    .ok_or_else(|| format!("{player} has no position and facing to spawn in front of"))?
            }
        };
        let mut params = json!({"tag": tag, "pos": pos});
        if let Some(yaw) = yaw {
            params["yaw"] = json!(yaw);
        }
        let made = self.call(agent_proto::sim::method::SPAWN, params)?;
        let at = made["pos"].as_array().map_or(json!(pos), |_| made["pos"].clone());
        Ok((made, at))
    }

    /// The made object's row in `world.query` near `at`, or a warning.
    fn find_made(&mut self, made: &Value, at: Value) -> Option<Value> {
        let handle = made["object"].as_str().unwrap_or_default().to_ascii_lowercase();
        let found = self
            .read(
                agent_proto::method::WORLD_QUERY,
                json!({"near": {"point": at, "radius": 8}, "fields": ["pos", "tag", "tag_name", "velocity"], "limit": 100}),
            )
            .and_then(|q| {
                q["objects"].as_array()?.iter().find(|o| {
                    !handle.is_empty()
                        && o["id"].as_str().is_some_and(|id| id.ends_with(&format!(".{handle}")))
                }).cloned()
            });
        if found.is_none() {
            self.warnings.push(format!(
                "object {handle} is not in world.query near where it was made: gone, moved away, or a kind world.query does not list"
            ));
        }
        found
    }

    fn ui(&mut self, plan: &Plan, screen: Option<&str>) -> Step {
        if !self.has(agent_proto::method::UI_AUDIT) && !self.has(agent_proto::method::UI_STACK) {
            return Err("the paired game lists neither ui.audit nor ui.stack".into());
        }
        if plan.ticks > 0 {
            self.settle(plan.ticks)?;
        }
        let audit = self.read(agent_proto::method::UI_AUDIT, json!({}));
        let stack = self.read(agent_proto::method::UI_STACK, json!({}));
        let shown = self.read(agent_proto::method::UI_SCREEN, Value::Null);
        let names = shown_names(audit.as_ref(), shown.as_ref());
        let matches = screen.map(|want| {
            let want = want.to_ascii_lowercase();
            names.iter().any(|n| n.to_ascii_lowercase() == want)
        });
        if matches == Some(false) {
            self.warnings.push(format!(
                "the game shows {}, not {}: open it with ui.press (pair.preview reads the menu, it does not open one)",
                if names.is_empty() { "no named screen".to_owned() } else { names.join(" / ") },
                screen.unwrap_or_default()
            ));
        }
        if let Some(issues) = audit.as_ref().and_then(|a| a["issues"].as_array()).filter(|i| !i.is_empty()) {
            self.warnings.push(format!("ui.audit finds {} issues on the screen shown", issues.len()));
        }
        self.out.insert(
            "reads".into(),
            json!({"ui": {"shown": names, "matches": matches, "audit": audit, "stack": stack, "screen": shown}}),
        );
        self.capture(plan)
    }

    fn hud(&mut self, plan: &Plan, state: Option<&str>) -> Step {
        let hidden = self
            .read("cheat.list", json!({}))
            .and_then(|list| {
                list["cheats"]
                    .as_array()?
                    .iter()
                    .find(|c| c["name"] == "hide_hud")
                    .map(|c| c["value"] == true)
            });
        if hidden == Some(true) {
            self.call("cheat.set", json!({"name": "hide_hud", "value": false}))?;
        }
        let shown = self.settle(plan.ticks).and_then(|()| {
            let mut reads = json!({"hud": {"hidden_before": hidden, "shown": hidden.is_some()}});
            if let Some(name) = state {
                let method = format!("{name}.state");
                if !self.has(&method) {
                    return Err(format!("the paired game does not list {method}"));
                }
                reads["state"] = self.call(&method, json!({}))?;
            }
            if let Some(camera) = self.camera() {
                reads["camera"] = camera;
            }
            self.out.insert("reads".into(), reads);
            self.capture(plan)
        });
        if hidden.is_none() {
            self.warnings.push(
                "the game lists no hide_hud cheat (cheat.list): the HUD is captured as it is".into(),
            );
        }
        // Put the cheat back as it was, whatever happened.
        if hidden == Some(true) {
            self.call("cheat.set", json!({"name": "hide_hud", "value": true}))?;
        }
        shown
    }

    fn audio(&mut self, seconds: f64, every: Duration, call: Option<&(String, Value)>) -> Step {
        self.need(agent_proto::method::AUDIO_VOICES, "the mixer's voices")?;
        if let Some((method, params)) = call {
            if method == agent_proto::sim::method::AUDIO_PLAY {
                self.need(method, "starts the sound")?;
            }
            let answer = self.call(method, params.clone())?;
            self.out.insert("applied".into(), answer);
        }
        let mut meter = Meter::default();
        let end = Instant::now() + Duration::from_secs_f64(seconds);
        let mut first = true;
        while first || Instant::now() < end {
            first = false;
            let started = Instant::now();
            let client = self.client.as_mut().ok_or("not connected")?;
            let voices = client
                .call(agent_proto::method::AUDIO_VOICES, Value::Null)
                .map_err(|e| format!("audio.voices: {e}"))?;
            meter.add(&voices);
            std::thread::sleep(every.saturating_sub(started.elapsed()));
        }
        self.note(format!("audio.voices x{}", meter.samples));
        let loops = self.read(agent_proto::method::AUDIO_LOOPS, Value::Null);
        let mut audio = meter.report(seconds);
        audio["loops"] = loops
            .as_ref()
            .and_then(|l| l["loops"].as_array())
            .map(|rows| {
                rows.iter()
                    .map(|r| json!({"tag_name": r["tag_name"], "tag_datum": r["tag_datum"], "segment": r["segment"]}))
                    .collect::<Vec<_>>()
            })
            .map_or(Value::Null, Value::from);
        if meter.voices.is_empty() {
            self.warnings.push(
                "no voice played in the window: make the sound with call (pogo.test, a tag's effect) or in the game".into(),
            );
        }
        if meter.peak > 1.0 {
            self.warnings.push(format!(
                "a voice left the mixer above unity gain ({:.2}, {:+.1} dB)",
                meter.peak,
                db(meter.peak)
            ));
        }
        self.out.insert("audio".into(), audio);
        Ok(())
    }

    /// Lets the game run `ticks` ticks.
    fn settle(&mut self, ticks: u64) -> Step {
        if ticks == 0 {
            return Ok(());
        }
        // A tick is 1/30 s; leave room for a loaded machine.
        let timeout_ms = (ticks * 34 * 4).max(5_000);
        self.call(
            agent_proto::method::WAIT,
            json!({"until": {"ticks": ticks}, "timeout_ms": timeout_ms}),
        )
        .map(|_| ())
    }

    /// The last sealed scene's views (`render.scene`): which drew, and
    /// whether a first-person view drew anything.
    fn camera(&mut self) -> Option<Value> {
        let scene = self.read(agent_proto::method::RENDER_SCENE, json!({}))?;
        let views: Vec<Value> = scene["views"]
            .as_array()
            .map(|v| {
                v.iter()
                    .map(|v| json!({"key": v["key"], "instances": v["instances"], "cut": v["cut"]}))
                    .collect()
            })
            .unwrap_or_default();
        let first: Vec<&Value> = views
            .iter()
            .filter(|v| v["key"].as_str().is_some_and(|k| k.starts_with("first_person:")))
            .collect();
        let drawn = first.iter().any(|v| v["instances"].as_u64().unwrap_or(0) > 0);
        Some(json!({
            "sealed": scene["sealed"],
            "tick": scene["tick"],
            "views": views,
            "first_person": {"views": first.len(), "drawn": drawn},
        }))
    }

    /// `pogo.state`, when the game has it: the stick's rider for this
    /// weapon and the chase camera.
    fn pogo(&mut self, tag: Option<&str>, object: &Value) -> Option<Value> {
        let state = self.read(agent_proto::pogo::STATE, Value::Null)?;
        let stick = tag.is_some_and(|tag| {
            state["weapons"]
                .as_array()
                .is_some_and(|w| w.iter().any(|w| w.as_u64().map(|d| format!("{d:08x}")).as_deref() == Some(tag)))
        });
        let object = object.as_str().and_then(|o| u32::from_str_radix(o, 16).ok());
        let rider = state["riders"].as_array().and_then(|riders| {
            riders
                .iter()
                .find(|r| object.is_some_and(|o| r["weapon"].as_u64() == Some(u64::from(o))))
                .cloned()
        });
        if stick && rider.is_none() {
            self.warnings.push(
                "pogo.state lists no rider holding the given stick (is the superbounce mode on?)".into(),
            );
        }
        if stick && state["camera"]["on"] == true && state["camera"]["drawn"] != true {
            self.warnings.push(
                "the Pogo chase camera is asked for but the last frame was not drawn from it (false headless, in split screen and under Forge's or Theater's camera)".into(),
            );
        }
        Some(json!({
            "stick": stick,
            "active": state["active"],
            "rider": rider,
            "camera": state["camera"],
        }))
    }

    /// The frame, copied to `out`, with its brightness.
    fn capture(&mut self, plan: &Plan) -> Step {
        let shot = match plan.capture {
            Capture::None => return Ok(()),
            Capture::Render => {
                let render = self.read(agent_proto::method::RENDER_CAPTURE, json!({}));
                match render {
                    Some(r) => Some(("render.capture", r)),
                    None if self.has(agent_proto::host_shot::method::SHOT) => {
                        self.warnings.push("render.capture gave no frame: host.screenshot was taken instead".into());
                        self.screenshot(plan.out.as_deref())
                    }
                    None => None,
                }
            }
            Capture::Screen => self.screenshot(plan.out.as_deref()),
        };
        let Some((method, reply)) = shot else {
            self.warnings.push("no frame was captured: the game has neither render.capture nor host.screenshot, or both refused".into());
            self.out.insert("capture".into(), Value::Null);
            return Ok(());
        };
        let mut path = reply["path"].as_str().map(PathBuf::from);
        if let (Some(from), Some(to)) = (path.clone(), plan.out.clone())
            && from != to
        {
            if let Some(dir) = to.parent().filter(|d| !d.as_os_str().is_empty()) {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::copy(&from, &to) {
                Ok(_) => path = Some(to),
                Err(e) => self.warnings.push(format!("copying {} to {}: {e}", from.display(), to.display())),
            }
        }
        let mut capture = json!({
            "path": path.as_ref().map(|p| p.display().to_string()),
            "method": method,
            "tick": reply["tick"],
            "present": reply["present"],
        });
        match path.as_deref().map(brightness) {
            Some(Ok(b)) => {
                if b.black() {
                    self.warnings.push(format!(
                        "the captured frame is black: {:.1}% of its pixels are below 4% brightness",
                        b.dark_share * 100.0
                    ));
                }
                capture["width"] = json!(b.width);
                capture["height"] = json!(b.height);
                capture["mean_luma"] = json!(b.mean_luma);
                capture["dark_share"] = json!(b.dark_share);
                capture["black"] = json!(b.black());
            }
            Some(Err(why)) => self.warnings.push(format!("reading the capture: {why}")),
            None => {}
        }
        self.out.insert("capture".into(), capture);
        Ok(())
    }

    fn screenshot(&mut self, out: Option<&Path>) -> Option<(&'static str, Value)> {
        let mut params = json!({});
        if let Some(out) = out {
            params["path"] = json!(out.display().to_string());
        }
        self.read(agent_proto::host_shot::method::SHOT, params)
            .map(|r| ("host.screenshot", r))
    }
}

/// A tag datum as 8 lower-case hex digits (`0xE1230045`, `e1230045`, or a
/// number).
fn hex_of(v: &Value) -> Option<String> {
    if let Some(n) = v.as_u64() {
        return Some(format!("{n:08x}"));
    }
    let s = v.as_str()?.trim();
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u32::from_str_radix(s, 16).ok().map(|n| format!("{n:08x}"))
}

/// The `world.object` id prefix of the objects `sim.spawn` makes from a
/// tag of `group`, for the kinds `world.object.animate` can name.
fn kind_of_group(group: &str) -> Option<&'static str> {
    Some(match group {
        "scen" => "scenery",
        "vehi" => "vehicle",
        "mach" => "machine",
        "bloc" => "crate",
        "crea" => "creature",
        _ => return None,
    })
}

/// `distance` in front of a unit (`world.object`'s `pos` and `forward`),
/// level with it, and the yaw that faces it back.
fn in_front(unit: &Value, distance: f64) -> Option<([f64; 3], Option<f64>)> {
    let pos: [f64; 3] = serde_json::from_value(unit["pos"].clone()).ok()?;
    let forward: [f64; 3] = serde_json::from_value(unit["forward"].clone()).ok()?;
    let flat = forward[0].hypot(forward[1]);
    if !(flat > 1e-6) {
        return None;
    }
    let (fx, fy) = (forward[0] / flat, forward[1] / flat);
    let at = [pos[0] + fx * distance, pos[1] + fy * distance, pos[2]];
    let yaw = (fy.atan2(fx).to_degrees() + 180.0).rem_euclid(360.0);
    Some((at, Some(yaw)))
}

/// The names the game gives the screen shown: `ui.audit`'s active
/// surface's and menu's `name` and `id`, and `ui.screen`'s `class` and
/// `screen`.
fn shown_names(audit: Option<&Value>, screen: Option<&Value>) -> Vec<String> {
    let mut names = Vec::new();
    let mut add = |v: &Value| match v {
        Value::String(s) if !s.is_empty() => names.push(s.clone()),
        Value::Number(n) => names.push(n.to_string()),
        _ => {}
    };
    if let Some(a) = audit {
        for part in ["active", "screen"] {
            add(&a[part]["name"]);
            add(&a[part]["id"]);
        }
    }
    if let Some(s) = screen {
        add(&s["class"]);
        add(&s["screen"]);
    }
    names.dedup();
    names
}

fn db(gain: f64) -> f64 {
    if gain > 0.0 { 20.0 * gain.log10() } else { f64::NEG_INFINITY }
}

/// Peak levels over `audio.voices` samples.
#[derive(Default)]
struct Meter {
    samples: u64,
    peak: f64,
    most: usize,
    /// Each voice id seen: its row at its loudest, and how many samples.
    voices: Vec<(u64, Value, f64, u64)>,
}

impl Meter {
    fn add(&mut self, sample: &Value) {
        self.samples += 1;
        let Some(voices) = sample["voices"].as_array() else { return };
        self.most = self.most.max(voices.len());
        for v in voices {
            let Some(id) = v["id"].as_u64() else { continue };
            let gain = v["applied_gain"].as_f64().unwrap_or(0.0);
            self.peak = self.peak.max(gain);
            match self.voices.iter_mut().find(|(seen, ..)| *seen == id) {
                Some((_, row, peak, n)) => {
                    *n += 1;
                    if gain > *peak {
                        *peak = gain;
                        *row = v.clone();
                    }
                }
                None => self.voices.push((id, v.clone(), gain, 1)),
            }
        }
    }

    fn report(&self, seconds: f64) -> Value {
        let mut loudest: Vec<&(u64, Value, f64, u64)> = self.voices.iter().collect();
        loudest.sort_by(|a, b| b.2.total_cmp(&a.2));
        let loudest: Vec<Value> = loudest
            .iter()
            .take(8)
            .map(|(id, row, peak, n)| {
                json!({
                    "id": id, "kind": row["kind"], "peak_gain": peak, "peak_db": finite(db(*peak)),
                    "volume_mb": row["volume_mb"], "distance_units": row["distance_units"],
                    "samples": n,
                })
            })
            .collect();
        json!({
            "seconds": seconds,
            "samples": self.samples,
            "voices": self.voices.len(),
            "most_at_once": self.most,
            "peak_gain": self.peak,
            "peak_db": finite(db(self.peak)),
            "loudest": loudest,
        })
    }
}

fn finite(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

/// A PNG's size and how dark it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Brightness {
    pub width: u32,
    pub height: u32,
    /// Mean Rec. 709 luma, 0 to 1.
    pub mean_luma: f64,
    /// The share of pixels below 4% luma.
    pub dark_share: f64,
}

impl Brightness {
    /// Nearly every pixel is dark: the frame is black.
    pub fn black(&self) -> bool {
        self.dark_share >= 0.98
    }
}

pub(super) fn brightness(path: &Path) -> Result<Brightness, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
    let size = reader.output_buffer_size().ok_or("an image too large to read")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    let bytes = &buf[..info.buffer_size()];
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("an indexed PNG left unexpanded".into()),
    };
    let mut sum = 0.0;
    let mut dark = 0u64;
    let mut n = 0u64;
    for px in bytes.chunks_exact(channels) {
        let luma = if channels < 3 {
            f64::from(px[0]) / 255.0
        } else {
            (0.2126 * f64::from(px[0]) + 0.7152 * f64::from(px[1]) + 0.0722 * f64::from(px[2])) / 255.0
        };
        sum += luma;
        if luma < 0.04 {
            dark += 1;
        }
        n += 1;
    }
    if n == 0 {
        return Err("an empty image".into());
    }
    Ok(Brightness {
        width: info.width,
        height: info.height,
        mean_luma: sum / n as f64,
        dark_share: dark as f64 / n as f64,
    })
}
