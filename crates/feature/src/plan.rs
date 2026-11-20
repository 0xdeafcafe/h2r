//! From declarations to chains: order, conflicts, missing points.
//!
//! **Order.** At one address each kind is sorted outermost first by
//! [`Order`](crate::Order) (`First`, `Default`, `Last`), then by declared
//! `after_feature`, then by feature id. `before` hooks and observers run in
//! that order; `after` hooks run in reverse (the outermost `after` last),
//! so a feature that wraps another wraps it on both sides.
//!
//! **Conflicts** (all reported, none resolved by guessing):
//! two implementations at one address; two `before` or two `after` hooks
//! of different features at one address with nothing ordering them; an
//! `after_feature` cycle; a `before` on an opaque point; two point names
//! for one address; two features with one id.
//!
//! **Missing points.** [`ImageAddrs`] maps a point to this image's address
//! or to nothing. A missing point drops an optional hook (point class
//! `Optional`, or the hook marked so) with a note, and turns any other
//! hook's feature off with the reason.

use std::collections::{BTreeMap, BTreeSet};

use engine::points::{Class, Point};

use crate::arena::Arena;
use crate::{Alloc, Hook, Kind, Manifest};

/// Where a point is in the running image.
pub trait ImageAddrs {
    fn resolve(&self, point: &Point) -> Option<u32>;
}

/// The base (1.0) image: every point at its base address.
pub struct Base;

impl ImageAddrs for Base {
    fn resolve(&self, point: &Point) -> Option<u32> {
        Some(point.base)
    }
}

/// Points at addresses given by name; unnamed points are missing. For
/// tests on synthetic guests (and, later, a title update's map).
#[derive(Clone, Debug, Default)]
pub struct Remap(pub BTreeMap<&'static str, u32>);

impl Remap {
    pub fn new(pairs: &[(&'static Point, u32)]) -> Self {
        Remap(pairs.iter().map(|(p, a)| (p.name, *a)).collect())
    }
}

impl ImageAddrs for Remap {
    fn resolve(&self, point: &Point) -> Option<u32> {
        self.0.get(point.name).copied()
    }
}

/// A feature in the plan, and why it is off when it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeatureState {
    pub id: &'static str,
    pub title: &'static str,
    pub off: Option<String>,
}

impl FeatureState {
    pub(crate) fn new(m: &Manifest, off: Option<String>) -> Self {
        FeatureState {
            id: m.id,
            title: m.title,
            off,
        }
    }
}

/// A plan refused, naming the parties and the point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Conflict {
    TwoImplementations {
        point: &'static str,
        addr: u32,
        a: &'static str,
        b: &'static str,
    },
    Unordered {
        point: &'static str,
        addr: u32,
        kind: Kind,
        a: &'static str,
        b: &'static str,
    },
    Cycle {
        point: &'static str,
        addr: u32,
        features: Vec<&'static str>,
    },
    BeforeOnOpaque {
        point: &'static str,
        feature: &'static str,
    },
    TwoNames {
        addr: u32,
        a: &'static str,
        b: &'static str,
    },
    DuplicateFeature(&'static str),
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Conflict::TwoImplementations { point, addr, a, b } => {
                write!(f, "{point} ({addr:#010x}): implemented by both {a} and {b}")
            }
            Conflict::Unordered {
                point,
                addr,
                kind,
                a,
                b,
            } => write!(
                f,
                "{point} ({addr:#010x}): {a} and {b} both hook it {} with no order between them",
                kind.as_str()
            ),
            Conflict::Cycle {
                point,
                addr,
                features,
            } => write!(
                f,
                "{point} ({addr:#010x}): after_feature cycle among {}",
                features.join(", ")
            ),
            Conflict::BeforeOnOpaque { point, feature } => write!(
                f,
                "{point}: {feature} puts a `before` on a point whose signature is not read"
            ),
            Conflict::TwoNames { addr, a, b } => {
                write!(f, "{addr:#010x} is named both {a} and {b}")
            }
            Conflict::DuplicateFeature(id) => write!(f, "two features have the id {id}"),
        }
    }
}

/// Everything at one address, in run order.
#[derive(Clone)]
pub struct Chain {
    pub point: &'static Point,
    pub addr: u32,
    /// Where the body is called from when something runs after it.
    pub shadow: Option<u32>,
    pub observe_before: Vec<Hook>,
    pub before: Vec<Hook>,
    pub implement: Option<Hook>,
    /// Outermost first; they run in reverse.
    pub after: Vec<Hook>,
    pub observe_after: Vec<Hook>,
}

impl Chain {
    pub fn hooks(&self) -> impl Iterator<Item = &Hook> {
        self.observe_before
            .iter()
            .chain(&self.before)
            .chain(&self.implement)
            .chain(&self.after)
            .chain(&self.observe_after)
    }
    pub fn needs_shadow(&self) -> bool {
        !self.after.is_empty() || !self.observe_after.is_empty()
    }
    /// Whether the placed chain may run on the pipelined render thread,
    /// or the hooks that keep it off and why. A chain with something after
    /// the body calls the body through its shadow with a Rust continuation,
    /// which a fallback cannot resume, so every hook in it is refused;
    /// otherwise the hooks not marked [`context_only`](crate::HookRef::context_only)
    /// are.
    pub fn render_thread(&self) -> Result<(), Vec<(&Hook, &'static str)>> {
        let refused: Vec<_> = if self.needs_shadow() {
            self.hooks()
                .map(|h| {
                    (
                        h,
                        "runs after the body: a Rust continuation a fallback cannot resume",
                    )
                })
                .collect()
        } else {
            self.hooks()
                .filter(|h| !h.context_only)
                .map(|h| (h, "not marked context-only"))
                .collect()
        };
        if refused.is_empty() {
            Ok(())
        } else {
            Err(refused)
        }
    }
}

/// The result of [`crate::Registry::build`]: what to install.
#[derive(Clone)]
pub struct Plan {
    pub features: Vec<FeatureState>,
    pub chains: BTreeMap<u32, Chain>,
    pub allocs: Vec<Alloc>,
    /// Functions of ours at arena addresses ([`crate::Builder::entry`]).
    pub entries: Vec<crate::Entry>,
    /// Hooks dropped and why.
    pub notes: Vec<String>,
    /// Hooks on hot points: they leave the direct-call fast path.
    pub warnings: Vec<String>,
    /// Where our strings are served from, one line per source.
    pub locale: Vec<String>,
    /// Each built feature's snapshot policy, in build order.
    pub snapshot: Vec<crate::FeatureSnapshot>,
}

/// The `locale` facade's declarations (see [`crate::locale`]).
pub(crate) struct LocaleInput {
    pub sources: Vec<crate::locale::Source>,
    pub shared: crate::locale::Sources,
    pub server: Option<&'static str>,
}

/// Fills the shared sources and, when no native implementation serves
/// them, adds the bridge's `before` on `LOCALE_FIND`.
fn place_locale(
    locale: LocaleInput,
    off: &BTreeMap<&'static str, String>,
    placed: &mut Vec<(u32, Hook)>,
    notes: &mut Vec<String>,
    addrs: &dyn ImageAddrs,
) -> Vec<String> {
    use engine::points::LOCALE_FIND;
    let sources: Vec<_> = locale
        .sources
        .into_iter()
        .filter(|s| !off.contains_key(s.feature))
        .collect();
    locale.shared.fill(
        sources
            .iter()
            .map(|s| std::sync::Arc::clone(&s.f))
            .collect(),
    );
    if sources.is_empty() {
        return Vec::new();
    }
    let native = locale.server.filter(|f| {
        placed.iter().any(|(_, h)| {
            h.feature == *f && h.kind() == Kind::Implement && std::ptr::eq(h.point, &LOCALE_FIND)
        })
    });
    let by = match native {
        Some(f) => format!("served by {f}"),
        None => match addrs.resolve(&LOCALE_FIND) {
            Some(addr) => {
                let shared = locale.shared.clone();
                placed.push((
                    addr,
                    Hook {
                        feature: "locale",
                        point: &LOCALE_FIND,
                        run: crate::Run::Before(std::sync::Arc::new(move |c: &mut crate::Call| {
                            match shared.find(c.cpu.edi) {
                                Some(a) => {
                                    c.cpu.eax = a;
                                    Ok(crate::Flow::Return(c.ret(0)?))
                                }
                                None => Ok(crate::Flow::Continue),
                            }
                        })),
                        order: crate::Order::Default,
                        after_features: Vec::new(),
                        optional: false,
                        label: "sources",
                        // Reads edi, the sources' boot-fixed answers, and
                        // the stack it returns through.
                        context_only: true,
                    },
                ));
                "served by the locale bridge".to_owned()
            }
            None => {
                notes.push(
                    "locale: LOCALE_FIND is not in this image; our strings are not served".into(),
                );
                "not served".to_owned()
            }
        },
    };
    sources
        .iter()
        .map(|s| format!("locale source {}:{} ({by})", s.feature, s.label))
        .collect()
}

pub(crate) fn build(
    mut features: Vec<FeatureState>,
    hooks: Vec<Hook>,
    mut allocs: Vec<Alloc>,
    mut entries: Vec<crate::Entry>,
    mut arena: Arena,
    locale: LocaleInput,
    addrs: &dyn ImageAddrs,
) -> Result<Plan, Vec<Conflict>> {
    let mut conflicts = Vec::new();
    let mut seen = BTreeSet::new();
    for f in &features {
        if !seen.insert(f.id) {
            conflicts.push(Conflict::DuplicateFeature(f.id));
        }
    }

    // Missing points: drop optional hooks, turn other features off.
    let mut notes = Vec::new();
    let mut off: BTreeMap<&'static str, String> = BTreeMap::new();
    let mut placed = Vec::new();
    for h in hooks {
        match addrs.resolve(h.point) {
            Some(addr) => placed.push((addr, h)),
            None if h.optional || h.point.class == Class::Optional => notes.push(format!(
                "{}: {} {} dropped: {} is not in this image",
                h.feature,
                h.kind().as_str(),
                h.label,
                h.point.name
            )),
            None => {
                off.entry(h.feature)
                    .or_insert_with(|| format!("{} is not in this image", h.point.name));
            }
        }
    }
    for f in &mut features {
        if let Some(reason) = off.get(f.id) {
            notes.push(format!("{}: off: {reason}", f.id));
            f.off = Some(reason.clone());
        }
    }
    placed.retain(|(_, h)| !off.contains_key(h.feature));
    allocs.retain(|a| !off.contains_key(a.feature));
    entries.retain(|e| !off.contains_key(e.feature));
    let locale_lines = place_locale(locale, &off, &mut placed, &mut notes, addrs);

    // Group by address; one name per address.
    let mut by_addr: BTreeMap<u32, Vec<Hook>> = BTreeMap::new();
    for (addr, h) in placed {
        by_addr.entry(addr).or_default().push(h);
    }
    let mut chains = BTreeMap::new();
    let mut warnings = Vec::new();
    for (addr, hooks) in by_addr {
        let point = hooks[0].point;
        if let Some(other) = hooks.iter().find(|h| !std::ptr::eq(h.point, point)) {
            conflicts.push(Conflict::TwoNames {
                addr,
                a: point.name,
                b: other.point.name,
            });
            continue;
        }
        let mut chain = Chain {
            point,
            addr,
            shadow: None,
            observe_before: Vec::new(),
            before: Vec::new(),
            implement: None,
            after: Vec::new(),
            observe_after: Vec::new(),
        };
        let mut kinds: BTreeMap<Kind, Vec<Hook>> = BTreeMap::new();
        for h in hooks {
            if h.kind() == Kind::Before && point.sig.is_opaque() {
                conflicts.push(Conflict::BeforeOnOpaque {
                    point: point.name,
                    feature: h.feature,
                });
            }
            kinds.entry(h.kind()).or_default().push(h);
        }
        for (kind, list) in kinds {
            if kind == Kind::Implement {
                if list.len() > 1 {
                    conflicts.push(Conflict::TwoImplementations {
                        point: point.name,
                        addr,
                        a: list[0].feature,
                        b: list[1].feature,
                    });
                }
                chain.implement = list.into_iter().next();
                continue;
            }
            let sorted = match order(point, addr, kind, list) {
                Ok(s) => s,
                Err(mut c) => {
                    conflicts.append(&mut c);
                    continue;
                }
            };
            match kind {
                Kind::ObserveBefore => chain.observe_before = sorted,
                Kind::Before => chain.before = sorted,
                Kind::After => chain.after = sorted,
                Kind::ObserveAfter => chain.observe_after = sorted,
                Kind::Implement => unreachable!(),
            }
        }
        if point.hot {
            warnings.push(format!(
                "{} ({addr:#010x}) is hot: hooking it leaves the direct-call fast path",
                point.name
            ));
        }
        if chain.needs_shadow() {
            match arena.alloc(4) {
                Some(s) => {
                    chain.shadow = Some(s);
                    allocs.push(Alloc {
                        feature: "(plan)",
                        addr: s,
                        len: 0x1000,
                        perms: runtime::guest::Permissions::RX,
                    });
                }
                None => notes.push(format!("{}: guest arena full", point.name)),
            }
        }
        chains.insert(addr, chain);
    }
    if conflicts.is_empty() {
        Ok(Plan {
            features,
            chains,
            allocs,
            entries,
            notes,
            warnings,
            locale: locale_lines,
            snapshot: Vec::new(),
        })
    } else {
        Err(conflicts)
    }
}

/// Sorts one kind's hooks at one point; refuses unordered mutating pairs
/// and cycles.
fn order(
    point: &'static Point,
    addr: u32,
    kind: Kind,
    mut list: Vec<Hook>,
) -> Result<Vec<Hook>, Vec<Conflict>> {
    list.sort_by(|a, b| (a.order, a.feature).cmp(&(b.order, b.feature)));
    let present: BTreeSet<&str> = list.iter().map(|h| h.feature).collect();
    // `edges[i]` holds the hooks that must sit outside hook i.
    let outside = |h: &Hook| -> Vec<&'static str> {
        h.after_features
            .iter()
            .copied()
            .filter(|f| present.contains(f) && *f != h.feature)
            .collect()
    };
    let mut conflicts = Vec::new();
    if kind.mutates() {
        for i in 0..list.len() {
            for j in i + 1..list.len() {
                let (a, b) = (&list[i], &list[j]);
                if a.feature == b.feature || a.order != b.order {
                    continue;
                }
                let related =
                    a.after_features.contains(&b.feature) || b.after_features.contains(&a.feature);
                if !related {
                    conflicts.push(Conflict::Unordered {
                        point: point.name,
                        addr,
                        kind,
                        a: a.feature,
                        b: b.feature,
                    });
                }
            }
        }
    }
    // Kahn's algorithm, taking the first ready hook in (order, id) order.
    let mut out = Vec::with_capacity(list.len());
    let mut left = list;
    while !left.is_empty() {
        let ready = left.iter().position(|h| {
            outside(h).iter().all(|f| {
                // Every hook of `f` (within this order class or not) placed.
                !left.iter().any(|o| o.feature == *f)
            })
        });
        match ready {
            Some(i) => out.push(left.remove(i)),
            None => {
                conflicts.push(Conflict::Cycle {
                    point: point.name,
                    addr,
                    features: left.iter().map(|h| h.feature).collect(),
                });
                break;
            }
        }
    }
    if conflicts.is_empty() {
        Ok(out)
    } else {
        Err(conflicts)
    }
}

impl Plan {
    /// One line per feature, then one per address with its chain in run
    /// order (`h2 features plan`; boot logs it).
    pub fn text(&self) -> String {
        let mut s = String::new();
        for f in &self.features {
            match &f.off {
                None => s.push_str(&format!("feature {}: on\n", f.id)),
                Some(r) => s.push_str(&format!("feature {}: off ({r})\n", f.id)),
            }
        }
        for c in self.chains.values() {
            let mut links = Vec::new();
            let names = |hs: &[Hook]| -> String {
                hs.iter()
                    .map(|h| format!("{}:{}", h.feature, h.label))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            if !c.observe_before.is_empty() {
                links.push(format!("observe_before [{}]", names(&c.observe_before)));
            }
            if !c.before.is_empty() {
                links.push(format!("before [{}]", names(&c.before)));
            }
            links.push(match &c.implement {
                Some(h) => format!("implement {}:{}", h.feature, h.label),
                None => "original".into(),
            });
            if !c.after.is_empty() {
                let mut rev = c.after.clone();
                rev.reverse();
                links.push(format!("after [{}]", names(&rev)));
            }
            if !c.observe_after.is_empty() {
                links.push(format!("observe_after [{}]", names(&c.observe_after)));
            }
            s.push_str(&format!(
                "{:#010x} {}: {}\n",
                c.addr,
                c.point.name,
                links.join(" -> ")
            ));
        }
        for e in &self.entries {
            s.push_str(&format!(
                "{:#010x} entry {}:{}\n",
                e.addr, e.feature, e.label
            ));
        }
        for l in &self.locale {
            s.push_str(&format!("{l}\n"));
        }
        for n in &self.notes {
            s.push_str(&format!("note: {n}\n"));
        }
        for w in &self.warnings {
            s.push_str(&format!("warning: {w}\n"));
        }
        s
    }

    /// What boot prints after installation. The summary counts installed
    /// points and entries; the full listing describes the plan as built.
    pub fn startup_text(&self, installed: &crate::Installed, verbose: bool) -> String {
        let mut text = if verbose {
            self.text()
        } else {
            let off: Vec<_> = self
                .features
                .iter()
                .filter(|f| f.off.is_some() || installed.off_reason(f.id).is_some())
                .map(|f| f.id)
                .collect();
            let off_names = if off.is_empty() {
                String::new()
            } else {
                format!(" ({})", off.join(", "))
            };
            let points = self
                .chains
                .keys()
                .filter(|addr| installed.placed.contains(addr))
                .count();
            let entries = self
                .entries
                .iter()
                .filter(|e| installed.placed.contains(&e.addr))
                .count();
            let mut text = format!(
                "{} features on, {} off{off_names}, {points} hooked points, {entries} entries; H2_FEATURE_PLAN=1 lists them\n",
                self.features.len() - off.len(),
                off.len(),
            );
            for n in &self.notes {
                text.push_str(&format!("note: {n}\n"));
            }
            text
        };
        for n in &installed.notes {
            text.push_str(n);
            text.push('\n');
        }
        text
    }

    /// Whether `feature` is in the plan and on.
    pub fn is_on(&self, feature: &str) -> bool {
        self.features
            .iter()
            .any(|f| f.id == feature && f.off.is_none())
    }
}
