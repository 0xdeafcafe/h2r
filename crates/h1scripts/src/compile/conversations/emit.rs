//! Typed expressions for generated dialogue scripts. Their emitter shares
//! the ordinary compiler's node and string allocation.

use hs::Datum;

use super::{CALL, Compiler, GLOBAL, Hook, NONE, New, Result, SCRIPT_CALL, h2t, invalid};

#[derive(Clone)]
pub(super) struct Global {
    pub(super) index: u16,
    pub(super) name: String,
    pub(super) ty: u16,
}

impl Global {
    pub(super) fn get(&self) -> Expr {
        Expr::Global(self.clone())
    }

    pub(super) fn set(&self, value: Expr) -> Expr {
        call("set", h2t::VOID, [self.get(), value])
    }
}

#[derive(Clone)]
pub(super) struct Script {
    pub(super) index: u16,
    pub(super) name: String,
    pub(super) ty: u16,
}

impl Script {
    pub(super) fn call(&self) -> Expr {
        Expr::Script(self.clone())
    }
}

#[derive(Clone)]
pub(super) enum Expr {
    Number(u16, i32),
    Real(f32),
    None(u16),
    Hook(u16, String),
    As(u16, Box<Self>),
    Global(Global),
    Script(Script),
    Call(&'static str, u16, Vec<Self>),
}

impl Expr {
    pub(super) fn ty(&self) -> u16 {
        match self {
            Self::Number(ty, _)
            | Self::None(ty)
            | Self::Hook(ty, _)
            | Self::As(ty, _)
            | Self::Call(_, ty, _) => *ty,
            Self::Real(_) => h2t::REAL,
            Self::Global(g) => g.ty,
            Self::Script(s) => s.ty,
        }
    }

    pub(super) fn emit(self, c: &mut Compiler<'_>, expected: u16) -> Result<Datum> {
        Ok(match self {
            Self::Number(ty, value) => {
                let node = c.number(ty, value);
                c.set_type(node, expected);
                node
            }
            Self::Real(value) => c.literal(
                expected,
                h2t::REAL,
                value.to_bits(),
                value.to_string().as_bytes(),
            ),
            Self::None(ty) => c.literal(expected, ty, NONE, b"none"),
            Self::Hook(ty, name) => {
                let node = c.literal(expected, ty, NONE, name.as_bytes());
                c.out.hooks.push(Hook {
                    slot: node.slot() as u16,
                    h2_type: ty,
                    type_name: c.h2.types[ty as usize].clone(),
                    h1_type: "conversation".into(),
                    token: name,
                });
                node
            }
            Self::As(_, expr) => return expr.emit(c, expected),
            Self::Global(g) => {
                let string = c.intern(g.name.as_bytes());
                c.push(New {
                    opcode: g.ty,
                    ty: expected,
                    flags: GLOBAL,
                    string,
                    data: g.index.into(),
                })
            }
            Self::Script(s) => {
                // The argumentless head is immutable and can be shared by
                // the scanner's repeated calls. Each call has its own sibling.
                let head = match c.conversation_heads.get(&s.index).copied() {
                    Some(head) => head,
                    None => {
                        let head = c.literal(h2t::FUNCTION_NAME, s.index, 0, s.name.as_bytes());
                        c.conversation_heads.insert(s.index, head);
                        head
                    }
                };
                c.push(New {
                    opcode: s.index,
                    ty: expected,
                    flags: SCRIPT_CALL,
                    string: 0,
                    data: head.0,
                })
            }
            Self::Call(name, _, arguments) => {
                let b = c.h2.builtins.get(name).ok_or_else(|| {
                    invalid(format!("conversation requires Halo 2 builtin {name}"))
                })?;
                if !b.special && b.parameters.len() != arguments.len() {
                    return Err(invalid(format!(
                        "conversation: unexpected {name} signature"
                    )));
                }
                let index = b.index;
                let parameters = b.parameters.clone();
                let mut args = Vec::with_capacity(arguments.len());
                for (i, arg) in arguments.into_iter().enumerate() {
                    let ty = parameters.get(i).copied().unwrap_or_else(|| arg.ty());
                    args.push(arg.emit(c, ty)?);
                }
                c.call(index, CALL, expected, name.as_bytes(), &args)
            }
        })
    }
}

pub(super) fn call(name: &'static str, ty: u16, args: impl IntoIterator<Item = Expr>) -> Expr {
    Expr::Call(name, ty, args.into_iter().collect())
}

pub(super) fn add_one(ty: u16, value: Expr) -> Expr {
    call(
        "+",
        ty,
        [Expr::As(h2t::REAL, Box::new(value)), Expr::Real(1.0)],
    )
}

pub(super) fn short(value: i32) -> Expr {
    Expr::Number(h2t::SHORT, value)
}

pub(super) fn boolean(value: bool) -> Expr {
    Expr::Number(h2t::BOOLEAN, i32::from(value))
}

pub(super) fn begin(ty: u16, args: impl IntoIterator<Item = Expr>) -> Expr {
    call("begin", ty, args)
}

pub(super) fn pred(name: &'static str, args: impl IntoIterator<Item = Expr>) -> Expr {
    call(name, h2t::BOOLEAN, args)
}

pub(super) fn and(args: impl IntoIterator<Item = Expr>) -> Expr {
    logic("and", true, args)
}

pub(super) fn or(args: impl IntoIterator<Item = Expr>) -> Expr {
    logic("or", false, args)
}

fn logic(name: &'static str, empty: bool, args: impl IntoIterator<Item = Expr>) -> Expr {
    let mut args: Vec<_> = args.into_iter().collect();
    match args.len() {
        0 => boolean(empty),
        1 => args.pop().unwrap(),
        _ => pred(name, args),
    }
}

pub(super) fn eq(a: Expr, b: Expr) -> Expr {
    pred("=", [a, b])
}

pub(super) fn not(a: Expr) -> Expr {
    pred("not", [a])
}

pub(super) fn when(condition: Expr, yes: Expr) -> Expr {
    call("if", h2t::VOID, [condition, yes])
}

pub(super) fn either(condition: Expr, yes: Expr, no: Expr) -> Expr {
    call("if", yes.ty(), [condition, yes, no])
}

pub(super) fn alive(object: Expr) -> Expr {
    pred(
        ">",
        [
            call("object_get_health", h2t::REAL, [object]),
            Expr::Real(0.0),
        ],
    )
}

pub(super) fn wait_until(condition: Expr, interval: i32) -> Expr {
    call("sleep_until", h2t::VOID, [condition, short(interval)])
}
