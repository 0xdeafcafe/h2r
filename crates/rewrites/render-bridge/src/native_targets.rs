//! `native-targets`: the scene's target select `0x0001cd30` and clear
//! `0x00014b60` answered without their guest bodies (native-only plan,
//! `nor-8b`; docs/graphics/framescene.md, "Slice 6"). On with the native
//! renderer.
//!
//! Both are thin wrappers of a D3D entry. The cut makes the wrapper's
//! decision in Rust and hands the device the bind or the clear through the
//! [`Host`] (boot's glue over the shared device), so a game view's binds
//! and clears make no guest D3D entry call:
//!
//! | Guest | Decision | Device |
//! |---|---|---|
//! | `0x0001cd30(esi colour, eax depth, [esp+4] flag)`, `ret 4`, `al = 1` | the cached pair `0x0051f3f4`/`0x0051f3f8` unchanged: nothing; a colour of the cached one's format and size (`+0`, `+0xc`, `+0x10`): the same-size bind `0x003f7b30`; else `0x003f7870` | [`Host::bind`] |
//! | `0x00014b60(eax flags, colour, z, stencil byte)`, `ret 0xc`, `eax = 0` | the game's flags as D3D clear flags ([`clear_flags`]); none: nothing | [`Host::clear`] |
//!
//! The arm: one call in [`CHECK_EVERY`] of each runs the guest body and
//! compares the device's bound target after it with the cut's
//! ([`Host::bound`]: the colour and depth surfaces, and the viewport where
//! the bind keeps it; a full bind's viewport is the device's own reset, the
//! same call either way). A difference
//! sends every later call to the guest body for the session, counted
//! `scene_kept_<call>_mismatch`. A call the cut does not make (no host, a
//! clear whose prelude `0x00014ac0` would run, a refused device call) runs
//! the guest body, counted `scene_kept_<call>_<cause>`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-targets";

/// One call in this many runs the guest body and is compared.
pub const CHECK_EVERY: u64 = 64;

/// The target select's cache of the bound pair and the call's flag byte.
pub const CACHE_COLOUR: u32 = 0x0051_f3f4;
pub const CACHE_DEPTH: u32 = 0x0051_f3f8;
pub const CACHE_FLAG: u32 = 0x0051_f3fc;

/// The clear's prelude gate: `0x00014ac0` runs when `[0x00485607]` is set
/// and `[0x004858b8]` is zero.
pub const CLEAR_PRELUDE_ON: u32 = 0x0048_5607;
pub const CLEAR_PRELUDE_TARGET: u32 = 0x0048_58b8;

/// Which bind the target select makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bind {
    /// The pair is the cached one: no bind.
    Kept,
    /// `0x003f7b30`: same size and format, the viewport left.
    SameSize,
    /// `0x003f7870`: the full bind, the viewport reset.
    Full,
}

/// The bind `0x0001cd30` makes for `colour` and `depth`.
pub fn select(mem: &dyn GuestMemory, colour: u32, depth: u32) -> Result<Bind, MemFault> {
    let cached = mem.u32(CACHE_COLOUR)?;
    if cached == colour && mem.u32(CACHE_DEPTH)? == depth {
        return Ok(Bind::Kept);
    }
    if cached != 0 && colour != 0 {
        let same = |at: u32| -> Result<bool, MemFault> { Ok(mem.u32(colour + at)? == mem.u32(cached + at)?) };
        if same(0)? && same(0xc)? && same(0x10)? {
            return Ok(Bind::SameSize);
        }
    }
    Ok(Bind::Full)
}

/// The D3D clear flags `0x00014b60` passes for the game's `flags`: bit 0
/// all four colour channels, bits 1..4 one channel each (`<< 3`), bits 5
/// and 6 depth and stencil.
pub fn clear_flags(flags: u32) -> u32 {
    let all = if flags & 1 != 0 { 0xf0 } else { 0 };
    all | (flags & 0x1e) << 3 | (flags >> 5) & 3
}

/// What answers the device's side: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Binds `colour` and `depth` as `bind` says (never [`Bind::Kept`]).
    /// `false`: the device refused, nothing changed.
    fn bind(&self, mem: &mut dyn GuestMemory, bind: Bind, colour: u32, depth: u32) -> bool;
    /// `D3DDevice_Clear(0, null, flags, colour, z, stencil)`. `false`: refused.
    fn clear(&self, mem: &mut dyn GuestMemory, flags: u32, colour: u32, z: u32, stencil: u32) -> bool;
    /// The device's bound target (`d3d::Device::bound_target`).
    fn bound(&self) -> Option<[u32; 8]>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one every call runs the guest body.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// One of the two calls, with its arm.
pub struct Arm {
    name: &'static str,
    calls: AtomicU64,
    cut: AtomicU64,
    checked: AtomicU64,
    fallback: AtomicBool,
    kept: Mutex<std::collections::BTreeMap<&'static str, u64>>,
}

impl Arm {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            calls: AtomicU64::new(0),
            cut: AtomicU64::new(0),
            checked: AtomicU64::new(0),
            fallback: AtomicBool::new(false),
            kept: Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    /// Whether this call is a check (the guest body, compared).
    pub fn next_is_check(&self) -> bool {
        self.calls.fetch_add(1, Ordering::Relaxed) % CHECK_EVERY == 0
    }

    pub fn fallen_back(&self) -> bool {
        self.fallback.load(Ordering::Relaxed)
    }

    /// The guest body ran for `cause`.
    pub fn keep(&self, cause: &'static str) {
        *self.kept.lock().unwrap_or_else(|e| e.into_inner()).entry(cause).or_default() += 1;
    }

    /// A check's comparison of `words`: equal targets pass; a difference
    /// falls back.
    pub fn compare(&self, cut: [u32; 8], guest: [u32; 8], words: std::ops::Range<usize>) -> bool {
        self.checked.fetch_add(1, Ordering::Relaxed);
        let same = cut[words.clone()] == guest[words];
        if !same {
            self.keep("mismatch");
            if !self.fallback.swap(true, Ordering::Relaxed) {
                eprintln!("native-targets: {} differs from the guest body (cut {cut:08x?}, guest {guest:08x?}); the guest body runs from here", self.name);
            }
        }
        same
    }

    fn counters(&self) -> Vec<(String, u64)> {
        let mut out = vec![
            (format!("{}_calls", self.name), self.calls.load(Ordering::Relaxed)),
            (format!("{}_cut", self.name), self.cut.load(Ordering::Relaxed)),
            (format!("{}_checked", self.name), self.checked.load(Ordering::Relaxed)),
        ];
        out.extend(
            self.kept.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(c, n)| (format!("scene_kept_{}_{c}", self.name), *n)),
        );
        out
    }
}

pub static TARGET_SELECT: Arm = Arm::new("target_select");
pub static SCENE_CLEAR: Arm = Arm::new("scene_clear");

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    TARGET_SELECT.counters().into_iter().chain(SCENE_CLEAR.counters()).collect()
}

/// The words of the bound target a check compares for `bind`: the pair,
/// and the viewport unless a full bind resets it.
pub fn compared(bind: Bind) -> std::ops::Range<usize> {
    match bind {
        Bind::Full => 0..2,
        Bind::Kept | Bind::SameSize => 0..8,
    }
}

/// The bound target the cut leaves for `bind` from `before`: the new pair
/// (a null colour keeps the bound one), the viewport as it was; see
/// [`compared`] for a full bind's.
pub fn expected(before: [u32; 8], bind: Bind, colour: u32, depth: u32) -> [u32; 8] {
    let mut after = before;
    if bind != Bind::Kept {
        after[0] = if colour == 0 { before[0] } else { colour };
        after[1] = depth;
    }
    after
}

/// The target select's cut: the bind through `host` and the cache words,
/// `eax` 1. `Err` names why the guest body runs instead (nothing written).
pub fn cut_select(mem: &mut dyn GuestMemory, host: &dyn Host, colour: u32, depth: u32, flag: u8) -> Result<Bind, &'static str> {
    let bind = select(&*mem, colour, depth).map_err(|_| "unread")?;
    if bind == Bind::Kept {
        return Ok(bind);
    }
    if !host.bind(mem, bind, colour, depth) {
        return Err("device");
    }
    let flag_word = mem.u32(CACHE_FLAG).map_err(|_| "unread")? & !0xff | u32::from(flag);
    for (at, v) in [(CACHE_COLOUR, colour), (CACHE_DEPTH, depth), (CACHE_FLAG, flag_word)] {
        mem.set_u32(at, v).map_err(|_| "unwritable")?;
    }
    Ok(bind)
}

/// The clear's cut: `Err` names why the guest body runs instead.
pub fn cut_clear(mem: &mut dyn GuestMemory, host: &dyn Host, flags: u32, colour: u32, z: u32, stencil: u8) -> Result<(), &'static str> {
    let d3d = clear_flags(flags);
    if d3d == 0 {
        return Ok(());
    }
    if mem.u8(CLEAR_PRELUDE_ON).map_err(|_| "unread")? != 0 && mem.u32(CLEAR_PRELUDE_TARGET).map_err(|_| "unread")? == 0 {
        return Err("prelude");
    }
    if !host.clear(mem, d3d, colour, z, u32::from(stencil)) {
        return Err("device");
    }
    Ok(())
}

/// Whether the native targets are placed: with the native renderer on,
/// unless `native-targets=off`.
pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches) && switches.feature(ID, true)
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

fn select_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let arm = &TARGET_SELECT;
    let check = arm.next_is_check();
    let Some(host) = host() else {
        arm.keep("host");
        return original(cpu, ctx, addr);
    };
    if arm.fallen_back() {
        arm.keep("fallback");
        return original(cpu, ctx, addr);
    }
    let (colour, depth) = (cpu.esi, cpu.eax);
    let rt = Arc::clone(ctx.runtime());
    if check {
        let predicted = {
            let mem = rt.memory();
            let bind = select(&*mem, colour, depth).ok();
            bind.zip(host.bound()).map(|(bind, before)| (bind, expected(before, bind, colour, depth)))
        };
        let out = original(cpu, ctx, addr)?;
        if let (Some((bind, cut)), Some(guest)) = (predicted, host.bound()) {
            arm.compare(cut, guest, compared(bind));
        }
        return Ok(out);
    }
    let made = {
        let mut mem = rt.memory();
        let flag = mem.u8(cpu.esp.wrapping_add(4)).unwrap_or(0);
        cut_select(&mut *mem, &*host, colour, depth, flag)
    };
    match made {
        Ok(_) => {
            arm.cut.fetch_add(1, Ordering::Relaxed);
            // `mov al, 1` after the bind's `eax = 0`, or over the cached colour.
            cpu.eax = 1;
            ctx.memory().ret(cpu, 4)
        }
        Err(cause) => {
            arm.keep(cause);
            original(cpu, ctx, addr)
        }
    }
}

fn clear_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let arm = &SCENE_CLEAR;
    let check = arm.next_is_check();
    let Some(host) = host() else {
        arm.keep("host");
        return original(cpu, ctx, addr);
    };
    if arm.fallen_back() {
        arm.keep("fallback");
        return original(cpu, ctx, addr);
    }
    if check {
        let before = host.bound();
        let out = original(cpu, ctx, addr)?;
        if let (Some(before), Some(after)) = (before, host.bound()) {
            arm.compare(before, after, 0..8);
        }
        return Ok(out);
    }
    let rt = Arc::clone(ctx.runtime());
    let made = {
        let mut mem = rt.memory();
        let word = |at: u32| mem.u32(cpu.esp.wrapping_add(at)).unwrap_or(0);
        let (colour, z, stencil) = (word(4), word(8), word(0xc) as u8);
        cut_clear(&mut *mem, &*host, cpu.eax, colour, z, stencil)
    };
    match made {
        Ok(()) => {
            arm.cut.fetch_add(1, Ordering::Relaxed);
            cpu.eax = 0;
            ctx.memory().ret(cpu, 0xc)
        }
        Err(cause) => {
            arm.keep(cause);
            original(cpu, ctx, addr)
        }
    }
}

pub struct NativeTargets;

impl Feature for NativeTargets {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The scene's target select and clear answered without their guest bodies").doc("docs/graphics/framescene.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The arm's fallback is session state no save holds: a load keeps
        // whichever path the session is on, and both bind the same target.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw().implement(&engine::points::SCENE_TARGET_SELECT, "native_targets_select", select_call).context_only();
        b.raw().implement(&engine::points::SCENE_CLEAR, "native_targets_clear", clear_call).context_only();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    #[derive(Default)]
    struct Device {
        binds: Mutex<Vec<(Bind, u32, u32)>>,
        clears: Mutex<Vec<[u32; 5]>>,
        refuse: AtomicBool,
    }

    impl Host for Device {
        fn bind(&self, _: &mut dyn GuestMemory, bind: Bind, colour: u32, depth: u32) -> bool {
            if self.refuse.load(Ordering::Relaxed) {
                return false;
            }
            self.binds.lock().unwrap().push((bind, colour, depth));
            true
        }
        fn clear(&self, _: &mut dyn GuestMemory, flags: u32, colour: u32, z: u32, stencil: u32) -> bool {
            self.clears.lock().unwrap().push([0, flags, colour, z, stencil]);
            true
        }
        fn bound(&self) -> Option<[u32; 8]> {
            None
        }
    }

    const A: u32 = 0x0060_0000;
    const B: u32 = 0x0060_0040;
    const C: u32 = 0x0060_0080;
    const Z: u32 = 0x0060_00c0;

    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0048_0000, 0x0020_0000);
        // A and B share format and size; C is larger.
        for (s, size) in [(A, 0x0011_f27f), (B, 0x0011_f27f), (C, 0x002c_f4ff)] {
            m.set_u32(s, 0x0004_0001).unwrap();
            m.set_u32(s + 0xc, 0x0000_0006).unwrap();
            m.set_u32(s + 0x10, size).unwrap();
        }
        m
    }

    /// A game view's binds and clears go to the device through the host,
    /// never through the guest's D3D entries: only one call in 64 runs the
    /// guest body, to compare.
    #[test]
    fn a_game_view_makes_no_target_bind() {
        let dev = Device::default();
        let mut m = mem();
        assert_eq!(cut_select(&mut m, &dev, A, Z, 1), Ok(Bind::Full), "nothing cached yet");
        assert_eq!((m.u32(CACHE_COLOUR).unwrap(), m.u32(CACHE_DEPTH).unwrap(), m.u8(CACHE_FLAG).unwrap()), (A, Z, 1));
        assert_eq!(cut_select(&mut m, &dev, A, Z, 0), Ok(Bind::Kept));
        assert_eq!(cut_select(&mut m, &dev, B, Z, 0), Ok(Bind::SameSize));
        assert_eq!(cut_select(&mut m, &dev, C, 0, 0), Ok(Bind::Full), "another size");
        assert_eq!(*dev.binds.lock().unwrap(), [(Bind::Full, A, Z), (Bind::SameSize, B, Z), (Bind::Full, C, 0)]);
        assert_eq!(cut_clear(&mut m, &dev, 1 | 1 << 5 | 1 << 6, 0xff00_0000, 0x3f80_0000, 0), Ok(()));
        assert_eq!(cut_clear(&mut m, &dev, 0, 0, 0, 0), Ok(()), "no flags: no clear");
        assert_eq!(dev.clears.lock().unwrap().len(), 1);
        let arm = Arm::new("test");
        let checks = (0..CHECK_EVERY * 2).filter(|_| arm.next_is_check()).count();
        assert_eq!(checks, 2, "one call in 64 runs the guest body");
    }

    #[test]
    fn the_clear_flags_are_the_games_as_d3ds() {
        assert_eq!(clear_flags(1), 0xf0, "all colour channels");
        assert_eq!(clear_flags(1 << 1), 0x10);
        assert_eq!(clear_flags(1 << 4), 0x80);
        assert_eq!(clear_flags(1 << 5), 1, "depth");
        assert_eq!(clear_flags(1 << 6), 2, "stencil");
        assert_eq!(clear_flags(0), 0);
    }

    /// A refused device call, or a clear whose prelude would run, leaves
    /// the call to the guest body with nothing written.
    #[test]
    fn a_call_the_cut_cannot_make_runs_the_guest_body_untouched() {
        let dev = Device::default();
        let mut m = mem();
        dev.refuse.store(true, Ordering::Relaxed);
        assert_eq!(cut_select(&mut m, &dev, A, Z, 1), Err("device"));
        assert_eq!(m.u32(CACHE_COLOUR).unwrap(), 0, "the cache unchanged");
        m.write(CLEAR_PRELUDE_ON, &[1]).unwrap();
        assert_eq!(cut_clear(&mut m, &dev, 1, 0, 0, 0), Err("prelude"));
        assert!(dev.clears.lock().unwrap().is_empty());
    }

    /// A check compares the target the cut would leave with the guest's;
    /// a difference falls back for good and is counted.
    #[test]
    fn a_check_that_differs_falls_back() {
        let before = [A, Z, 0, 0, 640, 480, 0, 1f32.to_bits()];
        assert_eq!(expected(before, Bind::Kept, A, Z), before);
        assert_eq!(expected(before, Bind::SameSize, B, 0)[..3], [B, 0, 0]);
        assert_eq!(expected(before, Bind::Full, 0, Z)[..2], [A, Z], "a null colour keeps the bound one");
        let arm = Arm::new("check");
        let reset = [C, Z, 0, 0, 1280, 720, 0, 1f32.to_bits()];
        assert!(arm.compare(expected(before, Bind::Full, C, Z), reset, compared(Bind::Full)), "a full bind's viewport is the device's");
        assert!(!arm.fallen_back());
        assert!(!arm.compare(before, expected(before, Bind::SameSize, B, Z), compared(Bind::SameSize)));
        assert!(arm.fallen_back());
        let c: std::collections::BTreeMap<_, _> = arm.counters().into_iter().collect();
        assert_eq!((c["check_checked"], c["scene_kept_check_mismatch"]), (2, 1));
    }
}
