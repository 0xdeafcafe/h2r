//! What a scenario's scripts spawn, for `map.glb` (docs/live/live-art.md,
//! "Scripted spawns"): objects a script creates that the level does not
//! place at the start, and effects a script plays at a cutscene flag or on
//! an object, with how often a looping script does it.
//!
//! Mods lean on this: a vehicle that appears after a minute, a turret that
//! respawns, a projectile-firing effect played every few seconds from a
//! `continuous` script. Retail multiplayer maps have almost none.
//!
//! The source is the scenario's scripts decompiled with the disc's own
//! executable (`hs::Scenario::decompile`), read here as text: a small
//! s-expression reader, not a compiler. What it reads:
//!
//! - `object_create`, `object_create_anew` (a scenario object name) and
//!   their `_containing` forms (every name containing the text);
//! - `effect_new` (an effect tag and a cutscene flag) and
//!   `effect_new_on_object_marker` (an effect, an object name, a marker);
//! - static scripts called by name, followed (a few levels deep), so a
//!   loop that calls a helper counts the helper's spawns and sleeps.
//!
//! A `continuous` script runs every tick; its period is the sum of the
//! literal `(sleep N)` ticks it (and what it calls) sleeps, at least one
//! tick. `sleep_until`, a computed sleep or `(sleep -1)` (forever) make the
//! period unknown: the spawn is still listed, as looping with no period.
//! Other script kinds spawn once (`period` none, `looping` false).

use std::collections::{BTreeMap, BTreeSet};

/// Game ticks per second (the period is reported in seconds).
pub const TICKS_PER_SECOND: f32 = 30.0;
/// How deep calls into static scripts are followed.
const MAX_CALL_DEPTH: usize = 4;

/// One parsed expression.
#[derive(Clone, Debug, PartialEq)]
enum Sexp {
    Atom(String),
    List(Vec<Sexp>),
}

impl Sexp {
    fn atom(&self) -> Option<&str> {
        match self {
            Sexp::Atom(a) => Some(a),
            Sexp::List(_) => None,
        }
    }
}

/// Tokens: `(`, `)`, and atoms (a quoted string unquoted, its escapes kept
/// as the decompiler wrote them except `\"` and `\\`).
fn tokens(source: &str) -> Vec<Option<String>> {
    let mut out = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '(' => out.push(None),
            ')' => out.push(Some(String::from(")"))),
            c if c.is_whitespace() => {}
            '"' => {
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some(e @ ('"' | '\\')) => s.push(e),
                            Some(e) => {
                                s.push('\\');
                                s.push(e);
                            }
                            None => break,
                        },
                        c => s.push(c),
                    }
                }
                // A quoted atom is never `)`: mark it so it is not taken for one.
                out.push(Some(format!("\"{s}")));
            }
            ';' => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            c => {
                let mut s = String::from(c);
                while let Some(&n) = chars.peek() {
                    if n.is_whitespace() || n == '(' || n == ')' {
                        break;
                    }
                    s.push(n);
                    chars.next();
                }
                out.push(Some(s));
            }
        }
    }
    out
}

/// Every top-level expression; an unbalanced tail is dropped.
fn parse(source: &str) -> Vec<Sexp> {
    let mut stack: Vec<Vec<Sexp>> = vec![Vec::new()];
    for t in tokens(source) {
        match t.as_deref() {
            None => stack.push(Vec::new()),
            Some(")") => {
                if stack.len() > 1 {
                    let list = stack.pop().unwrap();
                    stack.last_mut().unwrap().push(Sexp::List(list));
                }
            }
            Some(a) => {
                let a = a.strip_prefix('"').unwrap_or(a).to_string();
                stack.last_mut().unwrap().push(Sexp::Atom(a));
            }
        }
    }
    stack.swap_remove(0)
}

/// What a script spawns.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Spawned {
    /// A scenario object by name, or (`containing`) every name containing it.
    Object { name: String, containing: bool },
    /// An effect tag played at a cutscene flag.
    EffectAtFlag { effect: String, flag: String },
    /// An effect tag played on an object's marker.
    EffectOnObject {
        effect: String,
        object: String,
        marker: String,
    },
}

/// One spawn and the script that does it.
#[derive(Clone, Debug, PartialEq)]
pub struct Spawn {
    pub script: String,
    pub what: Spawned,
    /// Whether the script runs again and again (`continuous`).
    pub looping: bool,
    /// For a looping script, seconds between runs when its sleeps say.
    pub period: Option<f32>,
}

/// One script: its kind, the spawns and sleeps in its body, and the
/// scripts it calls.
#[derive(Default)]
struct Body {
    kind: String,
    spawned: Vec<Spawned>,
    /// Literal sleep ticks, summed; `None` once a sleep is not a plain
    /// non-negative literal (or is `sleep_until`).
    sleep: Option<u32>,
    calls: Vec<String>,
}

fn walk(e: &Sexp, body: &mut Body) {
    let Sexp::List(items) = e else { return };
    let Some(head) = items.first().and_then(Sexp::atom) else {
        for i in items {
            walk(i, body);
        }
        return;
    };
    let arg = |n: usize| items.get(n).and_then(Sexp::atom).map(str::to_string);
    match head {
        "object_create" | "object_create_anew" => {
            if let Some(name) = arg(1) {
                body.spawned.push(Spawned::Object {
                    name,
                    containing: false,
                });
            }
        }
        "object_create_containing" | "object_create_anew_containing" => {
            if let Some(name) = arg(1) {
                body.spawned.push(Spawned::Object {
                    name,
                    containing: true,
                });
            }
        }
        "effect_new" => {
            if let (Some(effect), Some(flag)) = (arg(1), arg(2)) {
                body.spawned.push(Spawned::EffectAtFlag { effect, flag });
            }
        }
        "effect_new_on_object_marker" => {
            if let (Some(effect), Some(object)) = (arg(1), arg(2)) {
                body.spawned.push(Spawned::EffectOnObject {
                    effect,
                    object,
                    marker: arg(3).unwrap_or_default(),
                });
            }
        }
        "sleep" => {
            let ticks = arg(1).and_then(|n| n.parse::<i64>().ok());
            body.sleep = match (body.sleep, ticks) {
                (Some(s), Some(t)) if t >= 0 => {
                    Some(s.saturating_add(t.min(u32::MAX as i64) as u32))
                }
                _ => None,
            };
        }
        "sleep_until" | "sleep_forever" => body.sleep = None,
        other => body.calls.push(other.to_string()),
    }
    for i in &items[1..] {
        walk(i, body);
    }
}

/// The scripts of a decompiled source, by name.
fn scripts(source: &str) -> BTreeMap<String, Body> {
    let mut out = BTreeMap::new();
    for top in parse(source) {
        let Sexp::List(items) = top else { continue };
        if items.first().and_then(Sexp::atom) != Some("script") {
            continue;
        }
        let Some(kind) = items.get(1).and_then(Sexp::atom) else {
            continue;
        };
        // `(script <kind> [<type>] <name> <body>...)`: the name is the
        // last atom before the body.
        let atoms = items[2..].iter().take_while(|e| e.atom().is_some()).count();
        let Some(name) = items
            .get(1 + atoms)
            .and_then(Sexp::atom)
            .filter(|_| atoms > 0)
        else {
            continue;
        };
        let mut body = Body {
            kind: kind.to_string(),
            sleep: Some(0),
            ..Body::default()
        };
        for e in &items[2 + atoms..] {
            walk(e, &mut body);
        }
        out.insert(name.to_string(), body);
    }
    out
}

/// A script's spawns and sleep ticks with what it calls folded in.
fn folded(
    all: &BTreeMap<String, Body>,
    name: &str,
    depth: usize,
    seen: &mut BTreeSet<String>,
) -> (Vec<Spawned>, Option<u32>) {
    let Some(b) = all.get(name) else {
        return (Vec::new(), Some(0));
    };
    let mut spawned = b.spawned.clone();
    let mut sleep = b.sleep;
    if depth < MAX_CALL_DEPTH {
        for c in &b.calls {
            if !all.contains_key(c) || !seen.insert(c.clone()) {
                continue;
            }
            let (s, t) = folded(all, c, depth + 1, seen);
            spawned.extend(s);
            sleep = sleep.zip(t).map(|(a, b)| a.saturating_add(b));
            seen.remove(c);
        }
    }
    (spawned, sleep)
}

/// Every spawn a decompiled source's scripts make. Static scripts are
/// reported through the scripts that call them (not on their own).
pub fn spawns(source: &str) -> Vec<Spawn> {
    let all = scripts(source);
    let mut out = Vec::new();
    for (name, b) in &all {
        if b.kind == "static" || b.kind == "stub" {
            continue;
        }
        let mut seen = BTreeSet::from([name.clone()]);
        let (spawned, sleep) = folded(&all, name, 0, &mut seen);
        let looping = b.kind == "continuous";
        let period = looping
            .then_some(sleep)
            .flatten()
            .map(|t| t.max(1) as f32 / TICKS_PER_SECOND);
        let mut once = BTreeSet::new();
        for what in spawned {
            if once.insert(what.clone()) {
                out.push(Spawn {
                    script: name.clone(),
                    what,
                    looping,
                    period,
                });
            }
        }
    }
    out
}

/// A scenario object name's entry (`scnr+0x48`, docs/formats/objects.md):
/// the name, the object type (type table index) and placement index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectName {
    pub name: String,
    pub object_type: i16,
    pub placement: i16,
}

/// Object names: `(count, address)` at `scnr+0x48`, stride `0x24`; name
/// (32 bytes) at `+0`, type `+0x20`, placement `+0x22` (as
/// `scenario_edit::layout` has them, with its evidence).
pub const OBJECT_NAMES: usize = 0x48;
pub const OBJECT_NAME_STRIDE: usize = 0x24;
/// Cutscene flags: `(count, address)` at `scnr+0x1e0`, stride `0x38`; name
/// (32 bytes) at `+4`, position at `+0x24` (docs/halo1/scripts.md, "Level
/// data the scripts name").
pub const CUTSCENE_FLAGS: usize = 0x1e0;
pub const CUTSCENE_FLAG_STRIDE: usize = 0x38;

fn c_string(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

impl ObjectName {
    pub fn parse(record: &[u8]) -> Option<ObjectName> {
        let i16_at = |o: usize| Some(i16::from_le_bytes(record.get(o..o + 2)?.try_into().ok()?));
        Some(ObjectName {
            name: c_string(record.get(..0x20)?),
            object_type: i16_at(0x20)?,
            placement: i16_at(0x22)?,
        })
    }
}

/// A cutscene flag's name and position.
pub fn parse_flag(record: &[u8]) -> Option<(String, [f32; 3])> {
    let f = |o: usize| Some(f32::from_le_bytes(record.get(o..o + 4)?.try_into().ok()?));
    Some((
        c_string(record.get(4..0x24)?),
        [f(0x24)?, f(0x28)?, f(0x2c)?],
    ))
}

/// The script behind a placement: which script creates it and how often.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptedBy {
    pub script: String,
    pub looping: bool,
    pub period: Option<f32>,
}

impl ScriptedBy {
    fn of(s: &Spawn) -> ScriptedBy {
        ScriptedBy {
            script: s.script.clone(),
            looping: s.looping,
            period: s.period,
        }
    }

    /// The placement node's `extras.script`.
    pub fn extras(&self) -> serde_json::Value {
        let mut e = serde_json::json!({ "name": self.script, "looping": self.looping });
        if let Some(p) = self.period {
            e["period"] = serde_json::json!(p);
        }
        e
    }
}

/// An effect a script plays somewhere: `asset.extras.spawners` in `map.glb`.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct EffectSpawner {
    pub script: String,
    /// The effect tag's name as the script gives it.
    pub effect: String,
    /// The cutscene flag or object it is played at.
    pub at: String,
    /// Game coordinates.
    pub pos: [f32; 3],
    pub looping: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<f32>,
}

/// The spawns resolved against the scenario: which placements (by object
/// type and placement index) a script creates, and where effects play.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scripted {
    pub placements: BTreeMap<(i16, usize), ScriptedBy>,
    pub effects: Vec<EffectSpawner>,
    /// Names the scripts use that the scenario does not have.
    pub unresolved: BTreeSet<String>,
}

impl Scripted {
    /// Resolves `spawns` with the scenario's object names and cutscene
    /// flags; `position` gives a placement's position by (type, index).
    /// A looping spawn wins over a one-off for the same placement.
    pub fn resolve(
        spawns: &[Spawn],
        names: &[ObjectName],
        flags: &[(String, [f32; 3])],
        position: impl Fn(i16, usize) -> Option<[f32; 3]>,
    ) -> Scripted {
        let mut out = Scripted::default();
        let named = |name: &str| {
            names
                .iter()
                .find(|n| n.name.eq_ignore_ascii_case(name))
                .and_then(|n| Some((n.object_type, usize::try_from(n.placement).ok()?)))
        };
        for s in spawns {
            match &s.what {
                Spawned::Object { name, containing } => {
                    let lower = name.to_ascii_lowercase();
                    let hits: Vec<(i16, usize)> = names
                        .iter()
                        .filter(|n| {
                            if *containing {
                                n.name.to_ascii_lowercase().contains(&lower)
                            } else {
                                n.name.eq_ignore_ascii_case(name)
                            }
                        })
                        .filter_map(|n| Some((n.object_type, usize::try_from(n.placement).ok()?)))
                        .collect();
                    if hits.is_empty() {
                        out.unresolved.insert(name.clone());
                    }
                    for k in hits {
                        let by = ScriptedBy::of(s);
                        let keep = out
                            .placements
                            .get(&k)
                            .is_some_and(|old| old.looping || !by.looping);
                        if !keep {
                            out.placements.insert(k, by);
                        }
                    }
                }
                Spawned::EffectAtFlag { effect, flag } => {
                    match flags.iter().find(|(n, _)| n.eq_ignore_ascii_case(flag)) {
                        Some((n, pos)) => out.effects.push(EffectSpawner {
                            script: s.script.clone(),
                            effect: effect.clone(),
                            at: n.clone(),
                            pos: *pos,
                            looping: s.looping,
                            period: s.period,
                        }),
                        None => {
                            out.unresolved.insert(flag.clone());
                        }
                    }
                }
                Spawned::EffectOnObject { effect, object, .. } => {
                    match named(object).and_then(|(t, i)| position(t, i)) {
                        Some(pos) => out.effects.push(EffectSpawner {
                            script: s.script.clone(),
                            effect: effect.clone(),
                            at: object.clone(),
                            pos,
                            looping: s.looping,
                            period: s.period,
                        }),
                        None => {
                            out.unresolved.insert(object.clone());
                        }
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = r#"(global short wave 0
  0)

(script startup intro
  (begin
    (object_create hog_late)
    (effect_new "effects\mod\boom" flag_center)))

(script static void fire_salvo
  (begin
    (effect_new_on_object_marker "effects\mod\salvo" tower "barrel")
    (sleep 15)))

(script continuous respawn_ghosts
  (begin
    (object_create_anew_containing ghost_)
    (sleep 300)
    (fire_salvo)))

(script continuous waits
  (begin
    (sleep_until (> wave 2))
    (object_create_anew "odd name")))

(script dormant later
  (object_create tower))
"#;

    #[test]
    fn spawns_are_read_with_their_loops_and_periods() {
        let s = spawns(SOURCE);
        let find = |script: &str| s.iter().filter(|x| x.script == script).collect::<Vec<_>>();
        let intro = find("intro");
        assert_eq!(intro.len(), 2);
        assert!(intro.iter().all(|x| !x.looping && x.period.is_none()));
        assert_eq!(
            intro[1].what,
            Spawned::EffectAtFlag {
                effect: "effects\\mod\\boom".into(),
                flag: "flag_center".into()
            }
        );
        let ghosts = find("respawn_ghosts");
        assert_eq!(ghosts.len(), 2, "its own spawn and the static script's");
        // 300 ticks of its own and 15 of the helper's, at 30 a second.
        assert!(ghosts.iter().all(|x| x.looping && x.period == Some(10.5)));
        assert_eq!(
            ghosts[1].what,
            Spawned::EffectOnObject {
                effect: "effects\\mod\\salvo".into(),
                object: "tower".into(),
                marker: "barrel".into()
            }
        );
        let waits = find("waits");
        assert_eq!(waits.len(), 1);
        assert!(
            waits[0].looping && waits[0].period.is_none(),
            "sleep_until: no period"
        );
        assert_eq!(
            waits[0].what,
            Spawned::Object {
                name: "odd name".into(),
                containing: false
            }
        );
        assert!(
            find("fire_salvo").is_empty(),
            "static scripts only through callers"
        );
        assert_eq!(find("later").len(), 1);
    }

    #[test]
    fn a_continuous_script_with_no_sleep_runs_every_tick() {
        let s = spawns("(script continuous spam (object_create_anew crate_a))");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].period, Some(1.0 / TICKS_PER_SECOND));
        let s = spawns("(script continuous never (begin (sleep -1) (object_create_anew a)))");
        assert_eq!(s[0].period, None, "sleeping forever has no period");
    }

    #[test]
    fn recursive_static_scripts_end() {
        let src = "(script static void a (begin (b) (object_create x)))
(script static void b (begin (a) (sleep 30)))
(script continuous c (a))";
        let s = spawns(src);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].period, Some(1.0));
    }

    #[test]
    fn unbalanced_or_odd_source_does_not_panic() {
        for src in [
            "(",
            ")",
            "(script",
            "(script continuous",
            "\"",
            "(script continuous x (sleep",
            "(()))",
        ] {
            let _ = spawns(src);
        }
    }

    fn name(n: &str, t: i16, p: i16) -> ObjectName {
        ObjectName {
            name: n.into(),
            object_type: t,
            placement: p,
        }
    }

    #[test]
    fn spawns_resolve_to_placements_flags_and_objects() {
        let names = [
            name("hog_late", 1, 4),
            name("ghost_a", 1, 0),
            name("ghost_b", 1, 1),
            name("tower", 6, 2),
            name("unplaced", 2, -1),
        ];
        let flags = [("flag_center".to_string(), [1.0, 2.0, 3.0])];
        let s = spawns(SOURCE);
        let r = Scripted::resolve(&s, &names, &flags, |t, i| {
            (t == 6 && i == 2).then_some([7.0, 8.0, 9.0])
        });
        assert_eq!(r.placements.len(), 4);
        assert!(!r.placements[&(1, 4)].looping);
        assert_eq!(r.placements[&(1, 0)].period, Some(10.5));
        assert_eq!(r.placements[&(1, 1)].script, "respawn_ghosts");
        assert_eq!(r.placements[&(6, 2)].script, "later");
        assert_eq!(r.effects.len(), 2);
        assert_eq!(r.effects[0].pos, [1.0, 2.0, 3.0]);
        assert_eq!(r.effects[1].pos, [7.0, 8.0, 9.0]);
        assert!(r.effects[1].looping);
        assert_eq!(r.unresolved, BTreeSet::from(["odd name".to_string()]));
    }

    #[test]
    fn a_looping_spawn_wins_over_a_one_off() {
        let s = spawns(
            "(script startup a (object_create v))
(script continuous b (begin (object_create_anew v) (sleep 60)))",
        );
        let r = Scripted::resolve(&s, &[name("v", 1, 0)], &[], |_, _| None);
        assert_eq!(r.placements[&(1, 0)].script, "b");
        assert_eq!(r.placements[&(1, 0)].period, Some(2.0));
    }

    #[test]
    fn names_and_flags_parse_from_records() {
        let mut rec = vec![0u8; OBJECT_NAME_STRIDE];
        rec[..5].copy_from_slice(b"ghost");
        rec[0x20..0x22].copy_from_slice(&1i16.to_le_bytes());
        rec[0x22..0x24].copy_from_slice(&3i16.to_le_bytes());
        assert_eq!(ObjectName::parse(&rec), Some(name("ghost", 1, 3)));
        assert_eq!(ObjectName::parse(&rec[..0x10]), None);
        let mut flag = vec![0u8; CUTSCENE_FLAG_STRIDE];
        flag[4..8].copy_from_slice(b"boom");
        for (i, v) in [1.5f32, -2.0, 3.25].into_iter().enumerate() {
            flag[0x24 + 4 * i..0x28 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        assert_eq!(parse_flag(&flag), Some(("boom".into(), [1.5, -2.0, 3.25])));
    }
}
