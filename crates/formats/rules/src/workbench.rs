//! The workbench: a script's check report and a scenario run over
//! [`FakeGame`], as plain data. Live's `/v1/rules/check` and
//! `/v1/rules/run` and the web workbench read these
//! (`docs/game/rules-engine.md`, "Workbench").
//!
//! A run is deterministic: the same source, scenario and seed give the same
//! trace on every machine.

use std::collections::BTreeMap;

use crate::caps;
use crate::fake::{FakeGame, FakePlayer};
use crate::ir::{self, Event, Handler, Ir, Op, Scope};
use crate::ty::{MAX_PLAYERS, MAX_SQUADS, MAX_TEAMS, NONE, Ty};
use crate::vm::{self, Arg, ErrorKind, Fired, Input, ScriptError, Vm, Warning};

/// The most ticks a scenario runs: 20 minutes at 30 ticks a second.
pub const MAX_TICKS: u32 = 36_000;
/// The most events a scenario lists.
pub const MAX_STEPS: usize = 4096;
/// The most players a scenario seats.
pub const MAX_SCENARIO_PLAYERS: usize = MAX_PLAYERS;
/// The most VM ops a whole run spends before it stops early.
pub const MAX_RUN_OPS: u64 = 4_000_000;
/// The most ticks a trace records before it is cut short.
pub const MAX_TRACED_TICKS: usize = 2_000;

/// An error at a line and column (both from 1), or for the whole script
/// (line and column 0: a verifier refusal or the budget).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub line: u32,
    pub col: u32,
    pub message: String,
}

/// One handler's worst-case cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandlerCost {
    /// `kill`, `tick`, `timer(next)`.
    pub event: String,
    pub worst_ops: u64,
}

/// Slots of one scope in use against its limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotUse {
    pub scope: &'static str,
    pub used: usize,
    pub limit: usize,
}

/// A declared capability in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapLine {
    pub name: String,
    /// Empty for a name the API does not have.
    pub words: &'static str,
    pub grantable: bool,
    pub known: bool,
}

/// What a compiled script costs and declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckReport {
    /// The worst-tick estimate; 0 when the verifier refused the blob.
    pub worst_tick_ops: u64,
    pub budget: u64,
    pub fits: bool,
    /// The handler that costs the most in the estimate.
    pub costliest: Option<String>,
    pub handlers: Vec<HandlerCost>,
    pub slots: Vec<SlotUse>,
    pub ir_bytes: usize,
    pub ir_limit: usize,
    pub capabilities: Vec<CapLine>,
    /// Entries the script calls that no host can make yet, in first-call
    /// order, each once.
    pub unhostable: Vec<&'static str>,
}

/// A script checked: compiled, verified and measured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// It compiled, verified, fits the budget and the IR cap.
    pub ok: bool,
    /// The IR's SHA-256 in lowercase hex, when it compiled.
    pub hash: Option<String>,
    /// The script's `rules "<name>"`, when it compiled.
    pub name: Option<String>,
    pub version: Option<u16>,
    pub errors: Vec<Problem>,
    /// Present when it compiled.
    pub report: Option<CheckReport>,
}

fn text(ir: &Ir, index: u16) -> &str {
    ir.strings.get(index as usize).map_or("?", String::as_str)
}

/// A handler's name as the language writes it.
pub fn handler_name(ir: &Ir, h: &Handler) -> String {
    match (h.event, h.timer) {
        (Event::Timer, Some(slot)) => {
            let name = ir.slots.get(slot as usize).map_or("?", |s| text(ir, s.name));
            format!("timer({name})")
        }
        (e, _) => e.name().to_string(),
    }
}

/// Every entry the blob calls whose status no host can make, each once.
pub fn unhostable_all(ir: &Ir) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for hd in &ir.handlers {
        for op in &hd.code {
            if let Op::Call { entry, .. } = *op
                && let Some(e) = caps::entry(entry, ir.header.api_version)
                && !e.status.hostable()
                && !out.contains(&e.name)
            {
                out.push(e.name);
            }
        }
    }
    out
}

fn slots(ir: &Ir) -> Vec<SlotUse> {
    let count = |scope: Scope| ir.slots.iter().filter(|s| s.scope == scope).count();
    vec![
        SlotUse {
            scope: "global",
            used: count(Scope::Global),
            limit: ir::MAX_GLOBAL_SLOTS,
        },
        SlotUse {
            scope: "player",
            used: count(Scope::Player),
            limit: ir::MAX_PLAYER_SLOTS,
        },
        SlotUse {
            scope: "team",
            used: count(Scope::Team),
            limit: ir::MAX_TEAM_SLOTS,
        },
        SlotUse {
            scope: "object",
            used: count(Scope::Object),
            limit: ir::MAX_OBJECT_SLOTS,
        },
    ]
}

fn capabilities(ir: &Ir) -> Vec<CapLine> {
    ir.header
        .requires
        .iter()
        .map(|name| match caps::capability(name, ir.header.api_version) {
            Some(c) => CapLine {
                name: name.clone(),
                words: c.words,
                grantable: c.grantable,
                known: true,
            },
            None => CapLine {
                name: name.clone(),
                words: "",
                grantable: false,
                known: false,
            },
        })
        .collect()
}

/// Compiles, verifies and measures source: the workbench's Check.
pub fn check(src: &str) -> Check {
    let ir = match crate::compile(src) {
        Ok(ir) => ir,
        Err(e) => {
            return Check {
                ok: false,
                hash: None,
                name: None,
                version: None,
                errors: vec![Problem {
                    line: e.line,
                    col: e.col,
                    message: e.message,
                }],
                report: None,
            };
        }
    };
    let bytes = ir.encode();
    let mut errors = Vec::new();
    let budget = vm::TICK_BUDGET as u64;
    let (worst, fits, costliest, handlers) = match crate::verify(&ir) {
        Ok(r) => {
            let handlers = ir
                .handlers
                .iter()
                .zip(&r.handler_ops)
                .map(|(h, &ops)| HandlerCost {
                    event: handler_name(&ir, h),
                    worst_ops: ops,
                })
                .collect();
            let costliest = r
                .costliest
                .and_then(|i| ir.handlers.get(i))
                .map(|h| handler_name(&ir, h));
            if !r.fits() {
                errors.push(Problem {
                    line: 0,
                    col: 0,
                    message: format!(
                        "the worst tick costs {} ops, over the budget of {budget}",
                        r.worst_tick
                    ),
                });
            }
            (r.worst_tick, r.fits(), costliest, handlers)
        }
        Err(refusal) => {
            errors.push(Problem {
                line: 0,
                col: 0,
                message: format!("refused: {refusal}"),
            });
            (0, false, None, Vec::new())
        }
    };
    if bytes.len() > ir::MAX_IR_BYTES {
        errors.push(Problem {
            line: 0,
            col: 0,
            message: format!(
                "the IR is {} bytes, over the {} byte cap",
                bytes.len(),
                ir::MAX_IR_BYTES
            ),
        });
    }
    let report = CheckReport {
        worst_tick_ops: worst,
        budget,
        fits,
        costliest,
        handlers,
        slots: slots(&ir),
        ir_bytes: bytes.len(),
        ir_limit: ir::MAX_IR_BYTES,
        capabilities: capabilities(&ir),
        unhostable: unhostable_all(&ir),
    };
    Check {
        ok: errors.is_empty(),
        hash: Some(content::Sha256::of(&bytes).to_string()),
        name: Some(ir.header.name.clone()),
        version: Some(ir.header.version),
        errors,
        report: Some(report),
    }
}

// ---------- scenarios ----------

/// A seated player of a scenario.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seat {
    pub slot: u8,
    pub team: u8,
    pub lives: i32,
}

/// One event at one tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub tick: u32,
    pub input: Input,
}

/// What a run starts from and what happens in it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Scenario {
    pub players: Vec<Seat>,
    /// The teams in play; empty means 0 and 1.
    pub teams: Vec<u8>,
    /// Retail record fields (`team_play`, `score_limit`, ...).
    pub variant: BTreeMap<String, i32>,
    /// The packed option bits; None for every option's default.
    pub options: Option<u32>,
    pub seed: u64,
    /// `round_time_left()` at tick 0, counted down one a tick.
    pub round_time: i32,
    /// The last tick run; ticks 0..=ticks run.
    pub ticks: u32,
    /// Fire `init` and `round_start` at tick 0, before its steps.
    pub start: bool,
    pub steps: Vec<Step>,
}

/// An action the script made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TracedCall {
    pub entry: &'static str,
    pub args: Vec<String>,
}

/// A player's table row and variables after a tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerState {
    pub slot: u8,
    pub team: u8,
    pub alive: bool,
    pub score: i32,
    pub lives: i32,
    pub vars: Vec<(String, String)>,
}

/// One tick in which something happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TickTrace {
    pub tick: u32,
    /// The handlers that ran, in order (`kill`, `timer(next)`, `tick`).
    pub ran: Vec<String>,
    pub calls: Vec<TracedCall>,
    /// Ops the tick spent.
    pub ops: u32,
    pub warnings: Vec<String>,
    pub globals: Vec<(String, String)>,
    pub players: Vec<PlayerState>,
    pub error: Option<String>,
}

/// A whole run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trace {
    pub hash: String,
    pub options: u32,
    /// Ticks in which a handler other than `on tick` ran, a call was made,
    /// a variable other than a timer changed, a warning was raised or the
    /// script stopped.
    pub ticks: Vec<TickTrace>,
    /// The last tick run.
    pub last_tick: u32,
    /// The script error the VM stopped on.
    pub error: Option<String>,
    /// The tick `end_round` was called in; the run stops after it.
    pub round_ended: Option<u32>,
    /// Why the run stopped before its last tick or the trace was cut, if
    /// it did.
    pub cut: Option<String>,
    pub ops: u64,
    /// The final scores, by slot.
    pub scores: Vec<(u8, i32)>,
}

/// Why a run did not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    /// The script did not compile or verify.
    Script(Vec<Problem>),
    /// The scenario is out of bounds.
    Scenario(String),
}

fn validate(s: &Scenario) -> Result<(), String> {
    if s.ticks > MAX_TICKS {
        return Err(format!("a scenario runs at most {MAX_TICKS} ticks"));
    }
    if s.steps.len() > MAX_STEPS {
        return Err(format!("a scenario lists at most {MAX_STEPS} events"));
    }
    if s.players.len() > MAX_SCENARIO_PLAYERS {
        return Err(format!("a scenario seats at most {MAX_SCENARIO_PLAYERS} players"));
    }
    for seat in &s.players {
        if seat.slot as usize >= MAX_PLAYERS {
            return Err(format!("player slot {} is out of range (0..{MAX_PLAYERS})", seat.slot));
        }
        if seat.team as usize >= MAX_TEAMS {
            return Err(format!("team {} is out of range (0..{MAX_TEAMS})", seat.team));
        }
    }
    for t in &s.teams {
        if *t as usize >= MAX_TEAMS {
            return Err(format!("team {t} is out of range (0..{MAX_TEAMS})"));
        }
    }
    for step in &s.steps {
        if step.tick > s.ticks {
            return Err(format!(
                "an event at tick {} is past the last tick {}",
                step.tick, s.ticks
            ));
        }
        let player = |p: u8| (p as usize) < MAX_PLAYERS;
        let ok = match &step.input {
            Input::Kill { victim, killer } => player(*victim) && killer.is_none_or(player),
            Input::Spawn(p)
            | Input::Death(p)
            | Input::Join(p)
            | Input::Quit(p)
            | Input::Pickup(p, _)
            | Input::Drop(p, _)
            | Input::Press(p, _)
            | Input::Bounce(p, _)
            | Input::Edit(p, _) => player(*p),
            Input::ActorKill { killer, .. } => killer.is_none_or(player),
            Input::SquadDead(s) => (*s as usize) < MAX_SQUADS,
            _ => true,
        };
        if !ok {
            return Err(format!(
                "the {} event at tick {} names a slot out of range",
                step.input.event().name(),
                step.tick
            ));
        }
    }
    Ok(())
}

/// A raw value as the trace shows it.
pub fn value_text(ir: &Ir, ty: Ty, v: i32) -> String {
    match ty {
        Ty::Int | Ty::Timer => v.to_string(),
        Ty::Fixed => fixed_text(v),
        Ty::Bool => (v != 0).to_string(),
        Ty::Player if v == NONE => "none".into(),
        Ty::Player => format!("player {v}"),
        Ty::Team if v == NONE => "none".into(),
        Ty::Team => format!("team {v}"),
        Ty::Object if v == NONE => "none".into(),
        Ty::Object => format!("object {v}"),
        Ty::Squad => format!("squad {v}"),
        Ty::Class | Ty::Text => ir
            .strings
            .get(v as usize)
            .map_or_else(|| "none".into(), |s| format!("{s:?}")),
        Ty::Tag => ir
            .tags
            .get(v as usize)
            .map_or_else(|| "none".into(), |s| format!("{s:?}")),
        Ty::Label => ir
            .labels
            .get(v as usize)
            .map_or_else(|| "?".into(), |s| format!("labelled {s:?}")),
        Ty::Enum(e) => ir
            .enums
            .get(e as usize)
            .and_then(|d| d.variants.get(v as usize))
            .map_or_else(|| v.to_string(), |&n| text(ir, n).to_string()),
    }
}

fn fixed_text(v: i32) -> String {
    let f = v as f64 / 65536.0;
    let s = format!("{f:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// An argument as the trace shows it.
pub fn arg_text(arg: &Arg) -> String {
    match arg {
        Arg::Int(n) => n.to_string(),
        Arg::Fixed(v) => fixed_text(*v),
        Arg::Bool(b) => b.to_string(),
        Arg::Player(Some(p)) => format!("player {p}"),
        Arg::Team(Some(t)) => format!("team {t}"),
        Arg::Object(Some(o)) => format!("object {o}"),
        Arg::Player(None) | Arg::Team(None) | Arg::Object(None) => "none".into(),
        Arg::Squad(s) => format!("squad {s}"),
        Arg::Class(Some(s)) | Arg::Tag(Some(s)) => format!("{s:?}"),
        Arg::Class(None) | Arg::Tag(None) => "none".into(),
        Arg::Label(l) => format!("labelled {l:?}"),
        Arg::Text(t) => format!("{t:?}"),
        Arg::Enum(e) => format!("enum {e}"),
    }
}

fn kind_words(kind: ErrorKind) -> String {
    match kind {
        ErrorKind::Budget => format!("the tick's {} ops ran out", vm::TICK_BUDGET),
        ErrorKind::TableRow => "a table row is out of range".into(),
        ErrorKind::Objects => format!(
            "more than {} objects hold object variables",
            vm::MAX_OBJECT_RECORDS
        ),
        ErrorKind::Queue => format!("more than {} calls are queued", vm::MAX_QUEUE),
        ErrorKind::Squad => "a squad number is outside the kit's slots".into(),
    }
}

/// A script error in words.
pub fn error_text(ir: &Ir, e: &ScriptError) -> String {
    let handler = ir
        .handlers
        .get(e.handler as usize)
        .map_or_else(|| format!("handler {}", e.handler), |h| handler_name(ir, h));
    format!(
        "script error at tick {}, in on {handler}, op {}: {}",
        e.tick,
        e.pc,
        kind_words(e.kind)
    )
}

fn warning_text(ir: &Ir, w: &Warning) -> String {
    match *w {
        Warning::DivideByZero { handler, pc } => {
            let name = ir
                .handlers
                .get(handler as usize)
                .map_or_else(|| format!("handler {handler}"), |h| handler_name(ir, h));
            format!("division by zero in on {name}, op {pc}: it gave 0")
        }
    }
}

fn slot_name(ir: &Ir, slot: &ir::Slot) -> String {
    text(ir, slot.name).to_string()
}

fn globals(vm: &Vm) -> Vec<(String, String)> {
    let ir = vm.ir();
    ir.slots
        .iter()
        .filter(|s| s.scope == Scope::Global)
        .filter_map(|s| {
            let name = slot_name(ir, s);
            let v = vm.global(&name)?;
            Some((name, value_text(ir, s.ty, v)))
        })
        .collect()
}

fn players(vm: &Vm, g: &FakeGame) -> Vec<PlayerState> {
    let ir = vm.ir();
    g.players
        .iter()
        .map(|(&slot, p)| PlayerState {
            slot,
            team: p.team,
            alive: p.alive,
            score: p.score,
            lives: p.lives,
            vars: ir
                .slots
                .iter()
                .filter(|s| s.scope == Scope::Player)
                .filter_map(|s| {
                    let name = slot_name(ir, s);
                    let v = vm.player_var(slot, &name)?;
                    Some((name, value_text(ir, s.ty, v)))
                })
                .collect(),
        })
        .collect()
}

/// Global timers that reach 0 in this tick's update and have a handler.
fn due_timers(vm: &Vm) -> Vec<String> {
    let ir = vm.ir();
    ir.slots
        .iter()
        .enumerate()
        .filter(|(_, s)| s.ty == Ty::Timer && s.scope == Scope::Global)
        .filter(|(i, s)| {
            vm.global(&slot_name(ir, s)) == Some(1)
                && ir
                    .handlers
                    .iter()
                    .any(|h| h.event == Event::Timer && h.timer == Some(*i as u16))
        })
        .map(|(_, s)| format!("timer({})", slot_name(ir, s)))
        .collect()
}

/// What the fake game does when the host reports an event, before the
/// handler runs. A quit removes the player after it.
fn apply(g: &mut FakeGame, input: &Input) {
    match *input {
        Input::Kill { victim, killer } => {
            if let Some(v) = g.players.get_mut(&victim) {
                v.alive = false;
                v.deaths = v.deaths.saturating_add(1);
            }
            if let Some(k) = killer
                && k != victim
                && let Some(k) = g.players.get_mut(&k)
            {
                k.kills = k.kills.saturating_add(1);
            }
        }
        Input::Death(p) => {
            if let Some(v) = g.players.get_mut(&p) {
                v.alive = false;
            }
        }
        Input::Spawn(p) => {
            if let Some(v) = g.players.get_mut(&p) {
                v.alive = true;
            }
        }
        Input::Join(p) => {
            g.players.entry(p).or_insert(FakePlayer {
                alive: true,
                ..FakePlayer::default()
            });
        }
        _ => {}
    }
}

/// Compiles `src` and runs `scenario` over a [`FakeGame`].
pub fn run(src: &str, scenario: &Scenario) -> Result<Trace, RunError> {
    validate(scenario).map_err(RunError::Scenario)?;
    let (ir, _) = crate::build(src).map_err(|e| {
        RunError::Script(vec![match e {
            crate::BuildError::Compile(c) => Problem {
                line: c.line,
                col: c.col,
                message: c.message,
            },
            crate::BuildError::Refused(r) => Problem {
                line: 0,
                col: 0,
                message: format!("refused: {r}"),
            },
        }])
    })?;
    let hash = ir.id().to_string();
    let options = scenario.options.unwrap_or_else(|| ir.default_options());
    let mut vm = Vm::new(ir, options, scenario.seed).map_err(|r| {
        RunError::Script(vec![Problem {
            line: 0,
            col: 0,
            message: format!("refused: {r}"),
        }])
    })?;

    let mut g = FakeGame {
        teams: if scenario.teams.is_empty() {
            vec![0, 1]
        } else {
            scenario.teams.clone()
        },
        variant: scenario.variant.clone(),
        round_time_left: scenario.round_time.max(0),
        ..FakeGame::default()
    };
    for seat in &scenario.players {
        g.players.insert(
            seat.slot,
            FakePlayer {
                team: seat.team,
                alive: true,
                lives: seat.lives,
                ..FakePlayer::default()
            },
        );
    }

    let mut steps: BTreeMap<u32, Vec<&Input>> = BTreeMap::new();
    for s in &scenario.steps {
        steps.entry(s.tick).or_default().push(&s.input);
    }

    let mut trace = Trace {
        hash,
        options,
        ticks: Vec::new(),
        last_tick: 0,
        error: None,
        round_ended: None,
        cut: None,
        ops: 0,
        scores: Vec::new(),
    };
    // A timer counts down every tick; its value alone is no change.
    let timers: Vec<String> = vm
        .ir()
        .slots
        .iter()
        .filter(|s| s.ty == Ty::Timer)
        .map(|s| slot_name(vm.ir(), s))
        .collect();
    let untimed = |v: &[(String, String)]| -> Vec<(String, String)> {
        v.iter()
            .filter(|(n, _)| !timers.contains(n))
            .cloned()
            .collect()
    };
    let mut last_vars = (untimed(&globals(&vm)), players(&vm, &g));

    for tick in 0..=scenario.ticks {
        vm.begin_tick(tick);
        if tick > 0 {
            g.round_time_left = (g.round_time_left - 1).max(0);
        }
        let calls_before = g.calls.len();
        vm.flush_queue(&mut g);
        let mut ran = Vec::new();
        let note = |fired: Fired, name: &str, ran: &mut Vec<String>| match fired {
            Fired::Ran => ran.push(name.to_string()),
            Fired::Carried => ran.push(format!("{name} (carried: 64 zone events used)")),
            Fired::NoHandler | Fired::Stopped => {}
        };
        if tick == 0 && scenario.start {
            let f = vm.fire(&mut g, Input::Init);
            note(f, "init", &mut ran);
            let f = vm.fire(&mut g, Input::RoundStart);
            note(f, "round_start", &mut ran);
        }
        for input in steps.get(&tick).into_iter().flatten() {
            apply(&mut g, input);
            let name = input.event().name();
            let f = vm.fire(&mut g, (*input).clone());
            note(f, name, &mut ran);
            if let Input::Quit(p) = **input {
                g.players.remove(&p);
            }
        }
        let due = due_timers(&vm);
        let before_update = vm.ext().stopped.is_none();
        let f = vm.update(&mut g);
        if before_update {
            ran.extend(due);
        }
        let tick_ran = f == Fired::Ran;

        let ops = vm.ext().ops;
        trace.ops += ops as u64;
        trace.last_tick = tick;
        let calls: Vec<TracedCall> = g.calls[calls_before..]
            .iter()
            .map(|(entry, args)| TracedCall {
                entry: *entry,
                args: args.iter().map(arg_text).collect(),
            })
            .collect();
        let warnings: Vec<String> = vm
            .take_warnings()
            .iter()
            .map(|w| warning_text(vm.ir(), w))
            .collect();
        let error = vm
            .ext()
            .stopped
            .filter(|e| e.tick == tick)
            .map(|e| error_text(vm.ir(), &e));
        let vars = (globals(&vm), players(&vm, &g));
        let key = (untimed(&vars.0), vars.1.clone());
        let changed = key != last_vars;
        let ended = calls.iter().any(|c| c.entry == "end_round");
        if !ran.is_empty() || !calls.is_empty() || !warnings.is_empty() || error.is_some() || changed {
            if tick_ran {
                ran.push("tick".into());
            }
            if trace.ticks.len() < MAX_TRACED_TICKS {
                trace.ticks.push(TickTrace {
                    tick,
                    ran,
                    calls,
                    ops,
                    warnings,
                    globals: vars.0.clone(),
                    players: vars.1.clone(),
                    error: error.clone(),
                });
            } else if trace.cut.is_none() {
                trace.cut = Some(format!(
                    "the trace holds {MAX_TRACED_TICKS} ticks; later ones are not shown"
                ));
            }
        }
        last_vars = key;
        if let Some(e) = error {
            trace.error = Some(e);
            break;
        }
        if ended {
            trace.round_ended = Some(tick);
            break;
        }
        if trace.ops > MAX_RUN_OPS {
            trace.cut = Some(format!(
                "the run spent over {MAX_RUN_OPS} ops and stopped at tick {tick}"
            ));
            break;
        }
    }
    trace.scores = g.players.iter().map(|(&s, p)| (s, p.score)).collect();
    Ok(trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_checks_clean() {
        let c = check(crate::builtin::SLAYER);
        assert!(c.ok, "{:?}", c.errors);
        assert_eq!(c.name.as_deref(), Some("Slayer"));
        let r = c.report.unwrap();
        assert_eq!(r.handlers.len(), 1);
        assert_eq!(r.handlers[0].event, "kill");
        assert!(r.worst_tick_ops > 0 && r.fits);
        assert!(r.capabilities.is_empty());
        assert_eq!(r.slots[0].scope, "global");
        assert_eq!(
            c.hash.unwrap(),
            crate::compile(crate::builtin::SLAYER).unwrap().id().to_string()
        );
    }

    #[test]
    fn a_compile_error_has_its_place() {
        let c = check("rules \"X\" 1\ncarrier slayer\non kill(v, k) { score(k, ) }\n");
        assert!(!c.ok);
        assert!(c.report.is_none());
        assert_eq!(c.errors[0].line, 3);
        assert!(c.errors[0].col > 0);
    }

    #[test]
    fn capabilities_read_in_plain_words() {
        let src = "rules \"T\" 1\ncarrier slayer\nteams 2\nrequires player.team, forge.move\non spawn(p) { set_team(p, team 1) }\n";
        let c = check(src);
        let r = c.report.unwrap();
        let names: Vec<_> = r.capabilities.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["player.team", "forge.move"]);
        assert_eq!(r.capabilities[0].words, "moves players between teams");
        assert!(!r.capabilities[0].grantable && r.capabilities[1].grantable);
        assert_eq!(r.unhostable, ["set_team"]);
    }

    fn slayer_scenario() -> Scenario {
        Scenario {
            players: vec![
                Seat { slot: 0, team: 0, lives: 0 },
                Seat { slot: 1, team: 1, lives: 0 },
            ],
            seed: 7,
            round_time: 100,
            ticks: 30,
            start: true,
            steps: vec![
                Step { tick: 3, input: Input::Kill { victim: 1, killer: Some(0) } },
                Step { tick: 9, input: Input::Kill { victim: 0, killer: None } },
                Step { tick: 9, input: Input::Spawn(1) },
            ],
            ..Scenario::default()
        }
    }

    #[test]
    fn a_run_traces_the_ticks_that_did_something() {
        let t = run(crate::builtin::SLAYER, &slayer_scenario()).unwrap();
        let ticks: Vec<u32> = t.ticks.iter().map(|t| t.tick).collect();
        assert_eq!(ticks, [3, 9]);
        assert_eq!(t.ticks[0].ran, ["kill"]);
        assert_eq!(t.ticks[0].calls[0].entry, "score");
        assert_eq!(t.ticks[0].calls[0].args, ["player 0", "1"]);
        assert_eq!(t.ticks[1].calls[0].args, ["player 0", "-1"]);
        assert_eq!(t.scores, [(0, 0), (1, 0)]);
        assert_eq!(t.last_tick, 30);
        assert!(t.error.is_none() && t.cut.is_none());
        // Deterministic.
        assert_eq!(run(crate::builtin::SLAYER, &slayer_scenario()).unwrap(), t);
    }

    #[test]
    fn timers_end_round_and_variables_show() {
        let src = "rules \"T\" 1\ncarrier slayer\nglobal var left: timer = 5\nglobal var n: int = 0\non timer(left) { n = n + 1; end_round(none) }\n";
        let t = run(src, &Scenario { ticks: 100, ..Scenario::default() }).unwrap();
        let last = t.ticks.last().unwrap();
        assert_eq!(t.round_ended, Some(last.tick));
        assert!(last.ran.contains(&"timer(left)".to_string()));
        assert!(last.globals.contains(&("n".to_string(), "1".to_string())));
        assert!(t.last_tick < 100);
    }

    #[test]
    fn a_scenario_out_of_bounds_is_refused() {
        let mut s = slayer_scenario();
        s.ticks = MAX_TICKS + 1;
        assert!(matches!(run(crate::builtin::SLAYER, &s), Err(RunError::Scenario(_))));
        let mut s = slayer_scenario();
        s.steps.push(Step { tick: 31, input: Input::Spawn(0) });
        assert!(matches!(run(crate::builtin::SLAYER, &s), Err(RunError::Scenario(_))));
        let mut s = slayer_scenario();
        s.players.push(Seat { slot: 16, team: 0, lives: 0 });
        assert!(matches!(run(crate::builtin::SLAYER, &s), Err(RunError::Scenario(_))));
        assert!(matches!(run("nope", &slayer_scenario()), Err(RunError::Script(_))));
    }
}
