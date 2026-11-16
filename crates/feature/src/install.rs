//! Places a [`Plan`] in the runtime's dispatch table: one override per
//! address, plus one shadow entry where something runs after the body.
//!
//! The shadow is the existing after-hook technique in one place: the chain
//! pops the caller's return address, calls the body through the shadow
//! (whose override runs the implementation or the generated original), and
//! a continuation runs the `after` hooks and observers, then returns to the
//! caller. During the body the guest stack slot holds the continuation
//! token, not the caller's address; that is the same in every run with the
//! same plan. When the call cannot be set up, the body runs alone.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use runtime::exec::OriginalHandler;
use runtime::{CpuAbi, ExecCtx, GuestFault, Runtime, TransferAbi, abi};

use crate::plan::{Chain, Plan};
use crate::{Call, Flow, Hook, ImplFn, Obs, Run};

/// What went in, and what was left out.
#[derive(Debug, Default)]
pub struct Installed {
    /// Addresses overridden by the plan (chains and shadows).
    pub placed: Vec<u32>,
    /// Hooks or features left out at install, and why.
    pub notes: Vec<String>,
    /// Features turned off at install (a required point has no generated
    /// original), with the reason.
    pub off: Vec<(&'static str, String)>,
}

impl Installed {
    /// Why `feature` was turned off at install, if it was.
    pub fn off_reason(&self, feature: &str) -> Option<&str> {
        self.off
            .iter()
            .find(|(f, _)| *f == feature)
            .map(|(_, r)| r.as_str())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstallError {
    /// An override the plan did not place is already there. Nothing was
    /// installed.
    Occupied {
        addr: u32,
        point: &'static str,
        existing: String,
        features: Vec<&'static str>,
    },
    Map {
        addr: u32,
        error: String,
    },
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Occupied {
                addr,
                point,
                existing,
                features,
            } => write!(
                f,
                "{point} ({addr:#010x}) is already overridden by {existing:?}, not by the plan (wanted by {})",
                features.join(", ")
            ),
            InstallError::Map { addr, error } => {
                write!(f, "mapping plan memory at {addr:#010x}: {error}")
            }
        }
    }
}

impl std::error::Error for InstallError {}

/// The body of a chain.
#[derive(Clone)]
enum Body {
    Original(Arc<OriginalHandler>),
    Implement(Arc<ImplFn>),
}

impl Body {
    fn invoke(
        &self,
        cpu: &mut CpuAbi,
        ctx: &mut ExecCtx,
        addr: u32,
    ) -> Result<TransferAbi, GuestFault> {
        match self {
            Body::Original(o) => Ok(o.invoke(cpu, ctx, addr)),
            Body::Implement(f) => f(cpu, ctx, addr),
        }
    }
}

/// One hook with its "active" line, printed the first time it runs.
struct Link<F: ?Sized> {
    f: Arc<F>,
    name: String,
    seen: AtomicBool,
}

impl<F: ?Sized> Link<F> {
    fn new(h: &Hook, f: Arc<F>) -> Arc<Self> {
        Arc::new(Link {
            f,
            name: format!(
                "{} {} {}:{}",
                h.point.name,
                h.kind().as_str(),
                h.feature,
                h.label
            ),
            seen: AtomicBool::new(false),
        })
    }
    fn announce(&self) {
        // A load first: the swap is a write, and hot points run this often.
        if !self.seen.load(Ordering::Relaxed) && !self.seen.swap(true, Ordering::Relaxed) {
            eprintln!("feature: {} active", self.name);
        }
    }
}

type Observers = Vec<Arc<Link<crate::ObserveFn>>>;

impl Plan {
    /// Installs every chain, or nothing when an address is occupied.
    /// Resolves originals before taking the dispatch write lock.
    pub fn install(&self, rt: &Runtime) -> Result<Installed, InstallError> {
        let mut out = Installed::default();
        // Bodies, and the hooks a missing original leaves out.
        let mut off: Vec<(&'static str, String)> = Vec::new();
        let mut bodies = Vec::new();
        for chain in self.chains.values() {
            let body = match &chain.implement {
                Some(h) => match &h.run {
                    Run::Implement(f) => Some(Body::Implement(Arc::clone(f))),
                    _ => unreachable!("implement slot holds an implementation"),
                },
                None => rt
                    .resolve_original(chain.addr)
                    .map(|o| Body::Original(Arc::new(o))),
            };
            if body.is_none() {
                for h in chain.hooks() {
                    let why = format!("{} has no generated original", chain.point.name);
                    if h.optional {
                        out.notes.push(format!(
                            "{}: {} {} dropped: {why}",
                            h.feature,
                            h.kind().as_str(),
                            h.label
                        ));
                    } else if !off.iter().any(|(f, _)| *f == h.feature) {
                        off.push((h.feature, why));
                    }
                }
            }
            bodies.push((chain, body));
        }
        let off_ids: BTreeSet<&str> = off.iter().map(|(f, _)| *f).collect();
        for (f, why) in &off {
            out.notes.push(format!("{f}: off: {why}"));
        }
        out.off = off;
        let keep = |h: &Hook| !off_ids.contains(h.feature);
        let chains: Vec<(Chain, Body)> = bodies
            .into_iter()
            .filter(|(_, body)| body.is_some())
            .filter_map(|(c, _)| {
                let mut c = c.clone();
                c.observe_before.retain(keep);
                c.before.retain(keep);
                c.after.retain(keep);
                c.observe_after.retain(keep);
                if c.implement.as_ref().is_some_and(|h| !keep(h)) {
                    c.implement = None;
                }
                c.hooks().next()?;
                let body = match &c.implement {
                    Some(Hook {
                        run: Run::Implement(f),
                        ..
                    }) => Body::Implement(Arc::clone(f)),
                    _ => Body::Original(Arc::new(rt.resolve_original(c.addr)?)),
                };
                Some((c, body))
            })
            .collect();

        // Refuse before touching anything.
        let mut occupied = None;
        rt.edit_dispatch(|d| {
            for (c, _) in &chains {
                for a in std::iter::once(c.addr).chain(c.shadow) {
                    if let Some(existing) = d.override_name(a) {
                        occupied = Some(InstallError::Occupied {
                            addr: a,
                            point: c.point.name,
                            existing: existing.to_owned(),
                            features: c.hooks().map(|h| h.feature).collect(),
                        });
                        return;
                    }
                }
            }
        });
        if let Some(e) = occupied {
            return Err(e);
        }
        {
            let mut mem = rt.memory();
            for a in &self.allocs {
                if a.feature != "(plan)" && off_ids.contains(a.feature) {
                    continue;
                }
                let len = (a.len + 0xfff) & !0xfff;
                mem.map_zeroed(a.addr, len, a.perms)
                    .map_err(|e| InstallError::Map {
                        addr: a.addr,
                        error: format!("{e:?}"),
                    })?;
            }
        }
        rt.edit_dispatch(|d| {
            for (c, body) in chains {
                out.placed.push(c.addr);
                if let Some(s) = c.shadow {
                    out.placed.push(s);
                }
                place(d, c, body);
            }
        });
        Ok(out)
    }
}

fn observers(hooks: &[Hook]) -> Observers {
    hooks
        .iter()
        .map(|h| match &h.run {
            Run::ObserveBefore(f) | Run::ObserveAfter(f) => Link::new(h, Arc::clone(f)),
            _ => unreachable!("observer slot"),
        })
        .collect()
}

fn place(d: &mut runtime::Dispatch, c: Chain, body: Body) {
    let (addr, point) = (c.addr, c.point);
    let observe_before = observers(&c.observe_before);
    let observe_after = Arc::new(observers(&c.observe_after));
    let before: Vec<_> = c
        .before
        .iter()
        .map(|h| match &h.run {
            Run::Before(f) => Link::new(h, Arc::clone(f)),
            _ => unreachable!("before slot"),
        })
        .collect();
    let mut after: Vec<_> = c
        .after
        .iter()
        .map(|h| match &h.run {
            Run::After(f) => Link::new(h, Arc::clone(f)),
            _ => unreachable!("after slot"),
        })
        .collect();
    after.reverse();
    let after = Arc::new(after);
    let implement = c.implement.as_ref().map(|h| Link::new(h, Arc::new(())));
    let name = format!(
        "plan:{}[{}]",
        point.name,
        c.hooks()
            .map(|h| h.feature)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",")
    );
    if let Some(shadow) = c.shadow {
        let body = body.clone();
        let implement = implement.clone();
        d.register_override(shadow, format!("{name}:body"), move |cpu, ctx, _| {
            if let Some(l) = &implement {
                l.announce();
            }
            body.invoke(cpu, ctx, addr)
        });
    }
    let shadow = c.shadow;
    d.register_override(addr, name, move |cpu, ctx, _| {
        for o in &observe_before {
            o.announce();
            (o.f)(&Obs {
                cpu,
                runtime: ctx.runtime(),
                addr,
                point,
            });
        }
        for b in &before {
            b.announce();
            let flow = (b.f)(&mut Call {
                cpu,
                ctx,
                addr,
                point,
            })?;
            if let Flow::Return(t) = flow {
                return Ok(t);
            }
        }
        let Some(shadow) = shadow else {
            if let Some(l) = &implement {
                l.announce();
            }
            return body.invoke(cpu, ctx, addr);
        };
        let rt = Arc::clone(ctx.runtime());
        let Ok(return_pc) = rt.memory().u32(cpu.esp) else {
            return body.invoke(cpu, ctx, addr);
        };
        cpu.esp = cpu.esp.wrapping_add(4);
        let (after, observe_after) = (Arc::clone(&after), Arc::clone(&observe_after));
        let transfer = {
            let mut mem = rt.memory();
            ctx.call_guest(cpu, &mut mem, shadow, move |cpu, ctx| {
                for a in after.iter() {
                    a.announce();
                    (a.f)(&mut Call {
                        cpu,
                        ctx,
                        addr,
                        point,
                    })?;
                }
                for o in observe_after.iter() {
                    o.announce();
                    (o.f)(&Obs {
                        cpu,
                        runtime: ctx.runtime(),
                        addr,
                        point,
                    });
                }
                Ok(TransferAbi::ret(return_pc))
            })
        };
        match transfer {
            Ok(t) if t.kind != abi::FAULT => Ok(t),
            _ => {
                cpu.esp = cpu.esp.wrapping_sub(4);
                body.invoke(cpu, ctx, addr)
            }
        }
    });
}
