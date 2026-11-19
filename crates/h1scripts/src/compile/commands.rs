//! Emit ordinary Halo 2 HS, including the command_script roots started by the
//! AI scheduler. Keep generated definitions after all authored definitions so
//! existing script calls, globals and wake operands keep their indices.
use super::*;
use h1ai::h1::command_script_name;

mod atoms;
mod control;

const VOID: u16 = h2t::VOID;
const BOOL: u16 = h2t::BOOLEAN;
const REAL: u16 = h2t::REAL;
const SHORT: u16 = h2t::SHORT;
const CS: u16 = 21;
const POINT: u16 = 28;
const OBJECT: u16 = 50;
const OBJECT_NAME: u16 = 56;
const OBJECT_LIST: u16 = 31;

#[derive(Clone)]
enum Expr {
    Literal(u16, u32, String),
    Symbol(u16, String, String),
    Var(String),
    Set(String, Box<Expr>),
    Call(&'static str, Vec<Expr>),
    Compare(&'static str, u16, Box<[Expr; 2]>),
    Input(Datum),
}
use self::Expr::*;

fn lit(ty: u16, bits: u32, name: impl Into<String>) -> Expr {
    Literal(ty, bits, name.into())
}
fn short(n: i16) -> Expr {
    lit(SHORT, 0xffff0000 | u32::from(n as u16), n.to_string())
}
fn real(n: f32) -> Expr {
    lit(REAL, n.to_bits(), n.to_string())
}
fn boolean(b: bool) -> Expr {
    lit(BOOL, u32::from(b), b.to_string())
}
fn var(name: &str) -> Expr {
    Var(name.into())
}
fn set(name: &str, value: Expr) -> Expr {
    Set(name.into(), Box::new(value))
}
fn call<const N: usize>(name: &'static str, args: [Expr; N]) -> Expr {
    Call(name, args.into())
}
fn cmp(op: &'static str, ty: u16, a: Expr, b: Expr) -> Expr {
    Compare(op, ty, Box::new([a, b]))
}
fn begin(body: Vec<Expr>) -> Expr {
    Call("begin", body)
}
fn current() -> Expr {
    var("ai_current_actor")
}
fn unit() -> Expr {
    call("ai_get_unit", [current()])
}

struct Def {
    name: String,
    kind: u16,
    ty: u16,
    body: Expr,
}

#[derive(Default)]
pub(super) struct Program {
    scripts: Vec<Def>,
    globals: Vec<(String, u16, Expr)>,
    script_ids: HashMap<String, u16>,
    global_ids: HashMap<String, (u16, u16)>,
    track: bool,
}

impl Program {
    pub(super) fn new(t: &Tables, h1: &Catalog) -> Result<Self> {
        let mut p = Self::default();
        if t.commands.lists.is_empty() {
            return Ok(p);
        }
        p.track = t
            .nodes
            .iter()
            .filter(|n| n.live() && !n.atom() && !n.script_call())
            .any(|n| {
                h1.function(n.opcode()).is_ok_and(|f| {
                    matches!(
                        f.name.as_str(),
                        "ai_command_list_status"
                            | "ai_command_list_advance"
                            | "ai_command_list_advance_by_unit"
                    )
                })
            });
        for (i, list) in t.commands.lists.iter().enumerate() {
            p.define(
                command_script_name(i),
                5,
                VOID,
                atoms::body(t, h1, i, list, p.track)?,
            );
        }
        if t.commands
            .lists
            .iter()
            .any(|l| l.atoms.iter().any(|a| a.kind == 23))
        {
            p.global("h1_cmd_random_point", SHORT, short(0));
            p.global("h1_cmd_look_point", POINT, lit(POINT, NONE, "none"));
        }
        if t.commands.lists.iter().flat_map(|l| &l.atoms).any(|a| {
            a.kind == 13
                && t.commands
                    .animations
                    .get(a.animation as usize)
                    .is_some_and(|a| a.graph.is_none())
        }) {
            p.global("h1_cmd_graph_unit", 51, lit(51, NONE, "none"));
            p.global("h1_cmd_unit_graph", 36, lit(36, NONE, "none"));
        }
        if t.commands
            .lists
            .iter()
            .flat_map(|l| &l.atoms)
            .any(|a| a.kind == 16)
        {
            p.global("h1_cmd_voice_unit", 51, lit(51, NONE, "none"));
            p.global("h1_cmd_voice_tag", h2t::LONG, lit(h2t::LONG, NONE, "-1"));
        }
        if !t.commands.timing.is_empty() {
            p.global("h1_cmd_timing_unit", 51, lit(51, NONE, "none"));
            p.global("h1_cmd_idle_min", REAL, real(0.0));
            p.global("h1_cmd_idle_max", REAL, real(0.0));
            p.global("h1_cmd_recovery", REAL, real(2.0));
            for (i, times) in t.commands.timing.iter().enumerate() {
                for (field, val) in [
                    ("lo", times.idle[0]),
                    ("hi", times.idle[1]),
                    ("burst", times.recovery),
                ] {
                    p.global(&format!("h1_cmd_{field}_{i}"), REAL, real(val));
                }
                for name in &times.placements {
                    p.global(name, 11, Symbol(11, name.clone(), "string".into()));
                }
            }
        }
        let needs_pc = t.commands.lists.iter().any(atoms::needs_pc);
        if p.track || needs_pc || t.commands.lists.iter().any(|l| l.flags & 16 != 0) {
            control::library(&mut p, needs_pc);
        }
        for (i, name) in t
            .scripts
            .iter()
            .map(|s| &s.name)
            .chain(p.scripts.iter().map(|s| &s.name))
            .enumerate()
        {
            let k = u16::try_from(i)
                .ok()
                .filter(|i| *i < 0x8000)
                .ok_or_else(|| invalid("too many command scripts"))?;
            if p.script_ids.insert(name.to_ascii_lowercase(), k).is_some() && i >= t.scripts.len() {
                return Err(invalid(format!(
                    "generated command script name collision: {name}"
                )));
            }
        }
        for (i, (name, ty, _)) in p.globals.iter().enumerate() {
            if t.globals.iter().any(|g| g.name.eq_ignore_ascii_case(name)) {
                return Err(invalid(format!(
                    "generated command global name collision: {name}"
                )));
            }
            p.global_ids
                .insert(name.clone(), ((t.globals.len() + i) as u16, *ty));
        }
        // 0x209051 allocates 3072 HS cells, including the external globals.
        if t.globals.len() + p.globals.len() + 0x41d > 3072 {
            return Err(invalid("command-list state exceeds the HS global array"));
        }
        Ok(p)
    }
    fn define(&mut self, name: String, kind: u16, ty: u16, body: Expr) {
        self.scripts.push(Def {
            name,
            kind,
            ty,
            body,
        });
    }
    fn global(&mut self, name: &str, ty: u16, init: Expr) {
        self.globals.push((name.into(), ty, init));
    }
}

impl Compiler<'_> {
    pub(super) fn generate_commands(&mut self) -> Result<()> {
        let p = self.commands;
        for (name, ty, value) in &p.globals {
            let root = self.generated(value, *ty)?;
            self.out
                .globals
                .push(definition(name, *ty, 0, root, self.out.globals.len())?);
        }
        for d in &p.scripts {
            self.definition.clone_from(&d.name);
            let root = self.generated(&d.body, d.ty)?;
            self.out.scripts.push(definition(
                &d.name,
                d.kind,
                d.ty,
                root,
                self.out.scripts.len(),
            )?);
        }
        Ok(())
    }

    fn builtin_for(&self, name: &str, arity: usize) -> Result<&crate::translate::H2Builtin> {
        self.h2
            .overloads
            .get(&(name.to_string(), arity))
            .or_else(|| self.h2.builtins.get(name))
            .filter(|f| f.special || f.parameters.len() == arity)
            .ok_or_else(|| {
                invalid(format!(
                    "{}: missing Halo 2 {name}/{arity}",
                    self.definition
                ))
            })
    }

    fn variable(&self, name: &str) -> Result<(u16, u16)> {
        self.commands
            .global_ids
            .get(name)
            .copied()
            .or_else(|| self.h2.externals.get(name).map(|&(i, t)| (i | 0x8000, t)))
            .ok_or_else(|| invalid(format!("unknown command-script global {name}")))
    }

    fn generated(&mut self, e: &Expr, expected: u16) -> Result<Datum> {
        let (name, args) = match e {
            Input(d) => return self.expr(*d, expected),
            Literal(ty, bits, text) => {
                return Ok(self.literal(expected, *ty, *bits, text.as_bytes()));
            }
            Var(name) => {
                let (idx, ty) = self.variable(name)?;
                let string = self.intern(name.as_bytes());
                return Ok(self.push(New {
                    opcode: ty,
                    ty: expected,
                    flags: GLOBAL,
                    string,
                    data: idx.into(),
                }));
            }
            Symbol(ty, token, original) => {
                let d = self.literal(expected, *ty, NONE, token.as_bytes());
                self.out.hooks.push(super::Hook {
                    slot: d.slot() as u16,
                    h2_type: *ty,
                    type_name: self.h2.types[*ty as usize].clone(),
                    h1_type: original.clone(),
                    token: token.clone(),
                });
                return Ok(d);
            }
            Set(name, value) => {
                let (_, ty) = self.variable(name)?;
                let var = self.generated(&var(name), ty)?;
                ("set", vec![var, self.generated(value, ty)?])
            }
            Compare(name, ty, args) => (
                *name,
                vec![
                    self.generated(&args[0], *ty)?,
                    self.generated(&args[1], *ty)?,
                ],
            ),
            Call(name, args) => {
                let f = self.builtin_for(name, args.len())?.clone();
                let mut out = Vec::with_capacity(args.len());
                for (i, arg) in args.iter().enumerate() {
                    let ty = match *name {
                        "begin" if i + 1 < args.len() => VOID,
                        "begin" => expected,
                        "if" if i == 0 => BOOL,
                        "if" => expected,
                        "and" | "or" | "not" => BOOL,
                        "+" | "-" | "min" | "max" | "*" => REAL,
                        "sleep_until" if i == 0 => BOOL,
                        "sleep_until" | "sleep" => SHORT,
                        "wake" => h2t::SCRIPT,
                        _ => *f
                            .parameters
                            .get(i)
                            .ok_or_else(|| invalid(format!("{name} argument {i}")))?,
                    };
                    out.push(self.generated(arg, ty)?);
                }
                return Ok(self.call(f.index, CALL, expected, name.as_bytes(), &out));
            }
        };
        let index = self.builtin_for(name, args.len())?.index;
        Ok(self.call(index, CALL, expected, name.as_bytes(), &args))
    }

    pub(super) fn command_literal(&mut self, i: u16, expected: u16) -> Result<Datum> {
        if i == u16::MAX {
            return Ok(self.literal(expected, CS, NONE, b"none"));
        }
        let name = command_script_name(usize::from(i));
        let index = self
            .commands
            .script_ids
            .get(&name)
            .copied()
            .ok_or_else(|| invalid(format!("command list {i}")))?;
        Ok(self.literal(expected, CS, u32::from(index), name.as_bytes()))
    }

    pub(super) fn command_builtin(
        &mut self,
        name: &str,
        args: &[Datum],
        expected: u16,
    ) -> Result<Option<Datum>> {
        let e = match (name, args) {
            ("ai_command_list" | "ai_command_list_by_unit", &[who, list]) => {
                let who = if name.ends_with("_by_unit") {
                    call("object_get_ai", [Input(who)])
                } else {
                    Input(who)
                };
                call("cs_run_command_script", [who, Input(list)])
            }
            (
                "ai_command_list_status"
                | "ai_command_list_advance"
                | "ai_command_list_advance_by_unit",
                &[who],
            ) => {
                if !self.commands.track {
                    // A scenario with no command lists cannot have an actor executing one.
                    return Ok(Some(self.number(expected, 0)));
                }
                let list = if name == "ai_command_list_advance" {
                    call("ai_actors", [Input(who)])
                } else {
                    Input(who)
                };
                control::visit(list, name == "ai_command_list_status")
            }
            _ => return Ok(None),
        };
        Ok(Some(self.generated(&e, expected)?))
    }
}
