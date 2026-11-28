//! Shadow arms (docs/graphics/native-visibility.md, "The shadow arms"): a
//! Rust port beside a guest function, held to it by a byte compare.
//!
//! The port builds what the function writes into a shadow buffer
//! ([`Proposal`]: named fields of bytes at guest addresses). On a sampled
//! call the guest body runs and, at its return, every field is compared
//! with guest memory ([`Proposal::differing`]). After [`PROVEN_AFTER`]
//! clean samples the arm answers its unsampled calls: Rust writes the
//! fields and the body is skipped. One difference falls the arm back to
//! the guest for the map; a refusal (the port cannot build the call) runs
//! the guest for that call only.
//!
//! | State | Sampled call (1 in [`SAMPLE_EVERY`]) | Other calls |
//! |---|---|---|
//! | learning | compare; the guest answers | the guest answers, nothing is made |
//! | proven | compare; Rust answers when equal | Rust answers when the switch is on |
//! | fallen | compare, counted; the guest answers | the guest answers |
//!
//! A map load starts every arm learning again; the counts stay. With
//! `impl.render.native.visibility=off` an arm still compares its samples
//! and never answers.

use std::collections::BTreeMap;

use guest_layout::memory::{GuestMemory, MemFault};

/// One frame in this many is compared with the guest's, whatever the
/// arm's state.
pub const SAMPLE_EVERY: u32 = 64;

/// Sampled matches, with no difference, before an arm answers.
pub const PROVEN_AFTER: u64 = 16;

/// What an arm does with one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Make the Rust result and compare it with the guest's.
    Compare,
    /// Make the Rust result and answer with it, uncompared.
    Answer,
    /// The guest answers; the Rust result is not needed.
    Guest,
}

/// Who answered one call, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Sampled, equal: answered by Rust once proven and on, else the guest.
    Matched,
    /// Sampled, different: the guest answers, and the arm falls back.
    Differed,
    /// Not sampled; Rust answered.
    Answered,
    /// Not sampled; the guest answered (learning, fallen or off).
    Guest,
    /// The Rust result refused or the guest's did not read: the guest
    /// answers.
    Declined,
}

/// An arm's counts since start: every call it saw is in exactly one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub matched: u64,
    pub differed: u64,
    pub answered: u64,
    pub guest: u64,
    pub declined: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Learning,
    Proven,
    Fallen,
}

/// One arm: its state on this map and its counts.
#[derive(Clone, Copy, Debug, Default)]
pub struct Shadow {
    state: State,
    /// Sampled matches on this map.
    clean: u64,
    pub counts: Counts,
}

/// Whether `frame` is a sampled one.
pub fn sampled(frame: u32) -> bool {
    frame % SAMPLE_EVERY == 0
}

impl Shadow {
    /// A learning arm with no counts.
    pub const NEW: Shadow = Shadow {
        state: State::Learning,
        clean: 0,
        counts: Counts {
            matched: 0,
            differed: 0,
            answered: 0,
            guest: 0,
            declined: 0,
        },
    };

    /// The arm answers its unsampled calls.
    pub fn proven(&self) -> bool {
        self.state == State::Proven
    }

    /// The arm has seen a difference on this map.
    pub fn fallen(&self) -> bool {
        self.state == State::Fallen
    }

    /// What to do with a call in `frame`. `on` is
    /// `impl.render.native.visibility`.
    pub fn plan(&self, frame: u32, on: bool) -> Plan {
        if sampled(frame) {
            Plan::Compare
        } else if on && self.proven() {
            Plan::Answer
        } else {
            Plan::Guest
        }
    }

    /// Record a compared call: `equal` is the comparison's result.
    /// Returns whether Rust answers it.
    pub fn compared(&mut self, equal: bool, on: bool) -> (Outcome, bool) {
        if !equal {
            self.counts.differed += 1;
            self.state = State::Fallen;
            return (Outcome::Differed, false);
        }
        self.counts.matched += 1;
        if self.state == State::Learning {
            self.clean += 1;
            if self.clean >= PROVEN_AFTER {
                self.state = State::Proven;
            }
        }
        (Outcome::Matched, on && self.proven())
    }

    /// Record an unsampled call: Rust answers it when `plan` was
    /// [`Plan::Answer`].
    pub fn unsampled(&mut self, plan: Plan) -> (Outcome, bool) {
        if plan == Plan::Answer {
            self.counts.answered += 1;
            (Outcome::Answered, true)
        } else {
            self.counts.guest += 1;
            (Outcome::Guest, false)
        }
    }

    /// Record a call whose Rust or guest side did not make a result.
    pub fn declined(&mut self) -> (Outcome, bool) {
        self.counts.declined += 1;
        (Outcome::Declined, false)
    }

    /// A map load: learn again; the counts stay.
    pub fn new_map(&mut self) {
        self.state = State::Learning;
        self.clean = 0;
    }
}

/// One named field of the shadow buffer: the bytes the function leaves at
/// `addr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub addr: u32,
    pub bytes: Vec<u8>,
}

/// What a port says the function writes, field by field, in the guest's
/// order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proposal {
    pub fields: Vec<Field>,
}

impl Proposal {
    pub fn push(&mut self, name: &'static str, addr: u32, bytes: impl Into<Vec<u8>>) {
        self.fields.push(Field { name, addr, bytes: bytes.into() });
    }
    pub fn u8(&mut self, name: &'static str, addr: u32, v: u8) {
        self.push(name, addr, [v]);
    }
    pub fn u16(&mut self, name: &'static str, addr: u32, v: u16) {
        self.push(name, addr, v.to_le_bytes());
    }
    pub fn u32(&mut self, name: &'static str, addr: u32, v: u32) {
        self.push(name, addr, v.to_le_bytes());
    }
    pub fn f32(&mut self, name: &'static str, addr: u32, v: f32) {
        self.u32(name, addr, v.to_bits());
    }

    /// The names of the fields whose bytes differ from `m`'s, in order,
    /// each once. A byte belongs to the last field that writes it, so a
    /// field a later one overwrites is held only to what survives. A byte
    /// that does not read differs.
    pub fn differing(&self, m: &dyn GuestMemory) -> Vec<&'static str> {
        let mut owner: BTreeMap<u32, (u8, usize)> = BTreeMap::new();
        for (i, f) in self.fields.iter().enumerate() {
            for (k, &b) in f.bytes.iter().enumerate() {
                owner.insert(f.addr.wrapping_add(k as u32), (b, i));
            }
        }
        let mut bad = vec![false; self.fields.len()];
        for (addr, (want, i)) in owner {
            if m.u8(addr).ok() != Some(want) {
                bad[i] = true;
            }
        }
        let mut out: Vec<&'static str> = Vec::new();
        for (f, bad) in self.fields.iter().zip(bad) {
            if bad && !out.contains(&f.name) {
                out.push(f.name);
            }
        }
        out
    }

    /// Write every field, in order. Check [`Proposal::writable`] first so
    /// a fault cannot leave a part written.
    pub fn apply(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        for f in &self.fields {
            m.write(f.addr, &f.bytes)?;
        }
        Ok(())
    }

    /// Whether every field's bytes read (so the writes land in mapped
    /// memory).
    pub fn writable(&self, m: &dyn GuestMemory) -> bool {
        self.fields.iter().all(|f| {
            let mut buf = vec![0; f.bytes.len()];
            m.read(f.addr, &mut buf).is_ok()
        })
    }
}

/// Guest memory as a port sees it while it builds: `base` with the
/// proposal's writes laid over it, so a read after a write sees the write,
/// as the guest's does. Nothing reaches `base`.
pub struct Shadowed<'a> {
    pub base: &'a dyn GuestMemory,
    pub proposal: Proposal,
}

impl<'a> Shadowed<'a> {
    pub fn new(base: &'a dyn GuestMemory) -> Self {
        Self { base, proposal: Proposal::default() }
    }

    /// Writes `bytes` at `addr` as the field `name`, refusing an address
    /// that does not read in `base`.
    pub fn put(&mut self, name: &'static str, addr: u32, bytes: &[u8]) -> Result<(), MemFault> {
        let mut probe = vec![0; bytes.len()];
        self.base.read(addr, &mut probe)?;
        self.proposal.push(name, addr, bytes.to_vec());
        Ok(())
    }
    pub fn put_u8(&mut self, name: &'static str, addr: u32, v: u8) -> Result<(), MemFault> {
        self.put(name, addr, &[v])
    }
    pub fn put_u16(&mut self, name: &'static str, addr: u32, v: u16) -> Result<(), MemFault> {
        self.put(name, addr, &v.to_le_bytes())
    }
    pub fn put_u32(&mut self, name: &'static str, addr: u32, v: u32) -> Result<(), MemFault> {
        self.put(name, addr, &v.to_le_bytes())
    }
}

impl GuestMemory for Shadowed<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.base.read(addr, buf)?;
        let end = u64::from(addr) + buf.len() as u64;
        for f in &self.proposal.fields {
            let (fs, fe) = (u64::from(f.addr), u64::from(f.addr) + f.bytes.len() as u64);
            if fs >= end || fe <= u64::from(addr) {
                continue;
            }
            for a in fs.max(u64::from(addr))..fe.min(end) {
                buf[(a - u64::from(addr)) as usize] = f.bytes[(a - fs) as usize];
            }
        }
        Ok(())
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        self.put("write", addr, data)
    }
}

/// An arm over one guest function: its [`Shadow`], the count of each
/// field that differed, and the first difference since the map loaded.
#[derive(Clone, Debug, Default)]
pub struct ByteArm {
    pub shadow: Shadow,
    pub differ: BTreeMap<&'static str, u64>,
    /// Refusals by name.
    pub refused: BTreeMap<&'static str, u64>,
    pub first: Option<String>,
}

impl ByteArm {
    pub const fn new() -> Self {
        Self { shadow: Shadow::NEW, differ: BTreeMap::new(), refused: BTreeMap::new(), first: None }
    }

    /// The guest ran `proposal`'s call: compare the fields with `m`, count
    /// each differing field, and keep the first difference (`at` names the
    /// call). Returns the differing fields.
    pub fn compare(&mut self, proposal: &Proposal, m: &dyn GuestMemory, on: bool, at: impl FnOnce() -> String) -> Vec<&'static str> {
        self.record(proposal.differing(m), on, at)
    }

    /// A compared call whose differing fields the caller found (a
    /// proposal's bytes and any register the function returns).
    pub fn record(&mut self, differing: Vec<&'static str>, on: bool, at: impl FnOnce() -> String) -> Vec<&'static str> {
        self.shadow.compared(differing.is_empty(), on);
        for &name in &differing {
            *self.differ.entry(name).or_default() += 1;
        }
        if let Some(name) = differing.first()
            && self.first.is_none()
        {
            self.first = Some(format!("{}: {name}", at()));
        }
        differing
    }

    /// The port refused the call (`why`): the guest answers it, counted.
    pub fn refuse(&mut self, why: &'static str) {
        self.shadow.declined();
        *self.refused.entry(why).or_default() += 1;
    }

    pub fn new_map(&mut self) {
        self.shadow.new_map();
        self.first = None;
    }

    /// `render.scene`-style counters under `prefix`: the outcomes, the
    /// state, each refusal and each differing field.
    pub fn counters(&self, prefix: &str) -> Vec<(&'static str, u64)> {
        let c = self.shadow.counts;
        let mut out: Vec<(&'static str, u64)> = [
            ("matched", c.matched),
            ("differ", c.differed),
            ("answered", c.answered),
            ("guest", c.guest),
            ("declined", c.declined),
            ("proven", u64::from(self.shadow.proven())),
            ("fallen", u64::from(self.shadow.fallen())),
        ]
        .into_iter()
        .map(|(k, v)| (intern(format!("{prefix}_{k}")), v))
        .collect();
        out.extend(self.refused.iter().map(|(k, v)| (intern(format!("{prefix}_refused_{k}")), *v)));
        out.extend(self.differ.iter().map(|(k, v)| (intern(format!("{prefix}_differ_{k}")), *v)));
        out
    }
}

/// A counter name made once: the set of arms, fields and refusals is
/// fixed, so each name is leaked at most once.
fn intern(name: String) -> &'static str {
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&n) = names.get(name.as_str()) {
        return n;
    }
    let n: &'static str = Box::leak(name.into_boxed_str());
    names.insert(n);
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn a_proposal_names_each_field_that_differs_once() {
        let mut m = FlatMemory::new(0x1000, 0x100);
        let mut p = Proposal::default();
        p.u32("a", 0x1000, 7);
        p.u32("b", 0x1004, 9);
        p.u8("b", 0x1008, 1);
        p.u32("unread", 0x9000, 0);
        assert_eq!(p.differing(&m), ["a", "b", "unread"]);
        assert!(!p.writable(&m));
        p.fields.pop();
        p.apply(&mut m).unwrap();
        assert!(p.differing(&m).is_empty());
    }

    #[test]
    fn a_read_after_a_shadowed_write_sees_the_write() {
        let mut base = FlatMemory::new(0x1000, 0x10);
        base.set_u32(0x1000, 0x1111_1111).unwrap();
        let mut s = Shadowed::new(&base);
        s.put_u16("low", 0x1000, 0x2222).unwrap();
        assert_eq!(s.u32(0x1000).unwrap(), 0x1111_2222);
        assert!(s.put_u8("past", 0x2000, 0).is_err());
        assert_eq!(base.u32(0x1000).unwrap(), 0x1111_1111, "nothing reaches the base");
    }

    #[test]
    fn the_arm_counts_each_differing_field_and_falls_back() {
        let m = FlatMemory::new(0x1000, 0x100);
        let mut arm = ByteArm::new();
        let mut p = Proposal::default();
        p.u32("zero", 0x1000, 0);
        for _ in 0..PROVEN_AFTER {
            assert!(arm.compare(&p, &m, true, String::new).is_empty());
        }
        assert!(arm.shadow.proven());
        assert_eq!(arm.shadow.plan(1, true), Plan::Answer);
        arm.refuse("override");
        assert!(arm.shadow.proven(), "a refusal is not a difference");
        p.u32("seven", 0x1004, 7);
        assert_eq!(arm.compare(&p, &m, true, || "call".into()), ["seven"]);
        assert!(arm.shadow.fallen());
        assert_eq!(arm.first.as_deref(), Some("call: seven"));
        let counters = arm.counters("x");
        assert!(counters.contains(&("x_differ_seven", 1)));
        assert!(counters.contains(&("x_refused_override", 1)));
        assert_eq!(arm.shadow.plan(1, true), Plan::Guest);
    }
}
