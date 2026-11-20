//! Button icons for the device in use (`docs/input/button-icons.md`).
//!
//! Two parts. [`FontRewrite`] serves the disc's font files with extra glyphs:
//! each icon set of `font::icons` at code units of its own, next to the
//! retail button glyphs. [`install`] hooks the game's glyph lookups and, for
//! a button's code unit, substitutes the unit of the set that is showing.
//! The glyph cache is keyed by code unit, so changing set needs no flush: the
//! next string drawn looks up the other units.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// One bit per (set, button) alias glyph that every font holds: five sets
/// of 18 need more than one word.
#[derive(Debug)]
struct Available([AtomicU64; 2]);

impl Available {
    fn all() -> Self {
        Available([AtomicU64::new(u64::MAX), AtomicU64::new(u64::MAX)])
    }
    fn has(&self, bit: u128) -> bool {
        let (lo, hi) = (bit as u64, (bit >> 64) as u64);
        self.0[0].load(Ordering::Relaxed) & lo != 0 || self.0[1].load(Ordering::Relaxed) & hi != 0
    }
    fn keep(&self, mask: u128) {
        self.0[0].fetch_and(mask as u64, Ordering::Relaxed);
        self.0[1].fetch_and((mask >> 64) as u64, Ordering::Relaxed);
    }
}

use font::icons::{
    BUTTON_UNITS, Button, IconChoice, IconSet, KeyIcon, KeyLabels, MouseIcon, key_label,
    replacements, shown_unit,
};
use input::active::{ActiveDevice, DeviceKind};
use input::bindings::{Bindings, Control, Input, MouseButton};

/// Glyph lookup, `0x1407d0`: `eax` is the font id, `[esp+4]` the code unit,
/// `[esp+8]` flags. The measuring and drawing routines and the glyph
/// precache all come through it.
pub const GLYPH_LOOKUP: u32 = 0x0014_07d0;
/// A second path into the glyph cache, `0x1f490`, reached through a function
/// pointer: its third stack argument (`[esp+0xc]`) is the code unit.
pub const GLYPH_LOOKUP_CALLBACK: u32 = 0x0001_f490;

/// The `icons` setting as a host passes it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconOptions {
    pub choice: IconChoice,
    /// Physical pads are enabled (fallback when no keyboard is available).
    pub gamepads: bool,
}

/// `H2_ICONS`: `auto`, `xbox`, `xbox360`, `playstation`, `keyboard`, `remote` or `steam`.
/// `Ok(None)` when unset.
pub fn choice_from_env() -> anyhow::Result<Option<IconChoice>> {
    match std::env::var("H2_ICONS") {
        Ok(v) => v
            .parse()
            .map(Some)
            .map_err(|e: String| anyhow::anyhow!("H2_ICONS: {e}")),
        Err(_) => Ok(None),
    }
}

/// What the keyboard set shows for each button, from the bindings: the first
/// input bound to the control. A stick shows its four keys together when
/// each has a one-character label ("WASD"), otherwise its "up" key; the right
/// stick shows the mouse when mouse look drives it. An unbound control
/// draws a dash cap instead of suggesting a nonexistent button.
pub fn key_labels(bindings: &Bindings) -> KeyLabels {
    use Control as C;
    let first = |c: Control| -> Option<KeyIcon> {
        Some(match bindings.inputs(c).first()? {
            Input::Key(k) => KeyIcon::Key(key_label(k)),
            Input::Mouse(m) => KeyIcon::Mouse(match m {
                MouseButton::Left => MouseIcon::Left,
                MouseButton::Right => MouseIcon::Right,
                MouseButton::Middle => MouseIcon::Middle,
                MouseButton::Back => MouseIcon::Back,
                MouseButton::Forward => MouseIcon::Forward,
            }),
        })
    };
    let stick = |dirs: [Control; 4]| -> Option<KeyIcon> {
        let icons: Vec<Option<KeyIcon>> = dirs.iter().map(|c| first(*c)).collect();
        let letters: Option<String> = icons
            .iter()
            .map(|i| match i {
                Some(KeyIcon::Key(l)) if l.chars().count() == 1 => Some(l.as_str()),
                _ => None,
            })
            .collect();
        letters
            .map(KeyIcon::Key)
            .or_else(|| icons.into_iter().flatten().next())
    };
    let mut out = KeyLabels::new();
    let mut put = |b: Button, icon: Option<KeyIcon>| {
        out.insert(b, icon.unwrap_or_else(|| KeyIcon::Key("-".into())));
    };
    for (b, c) in [
        (Button::A, C::A),
        (Button::B, C::B),
        (Button::X, C::X),
        (Button::Y, C::Y),
        (Button::Black, C::Black),
        (Button::White, C::White),
        (Button::LeftTrigger, C::LeftTrigger),
        (Button::RightTrigger, C::RightTrigger),
        (Button::DpadUp, C::DpadUp),
        (Button::DpadDown, C::DpadDown),
        (Button::DpadLeft, C::DpadLeft),
        (Button::DpadRight, C::DpadRight),
        (Button::Start, C::Start),
        (Button::Back, C::Back),
        (Button::LeftStickClick, C::LeftThumb),
        (Button::RightStickClick, C::RightThumb),
    ] {
        put(b, first(c));
    }
    // Up, left, down, right: the order the letters read in ("WASD").
    put(
        Button::LeftStick,
        stick([
            C::LeftStickUp,
            C::LeftStickLeft,
            C::LeftStickDown,
            C::LeftStickRight,
        ]),
    );
    let right = stick([
        C::RightStickUp,
        C::RightStickLeft,
        C::RightStickDown,
        C::RightStickRight,
    ]);
    put(
        Button::RightStick,
        right.or_else(|| {
            bindings
                .mouse
                .look
                .then_some(KeyIcon::Mouse(MouseIcon::Move))
        }),
    );
    out
}

/// Which icon set is showing, and what each set draws.
pub struct Icons {
    choice: IconChoice,
    /// What `auto` shows until a device has been used.
    initial: IconSet,
    active: Option<ActiveDevice>,
    keys: KeyLabels,
    available: Arc<Available>,
}

impl Icons {
    /// `keyboard` is the player-one keyboard pad when there is one: its
    /// bindings label the keycaps and its last-used device drives `auto`.
    pub fn new(
        options: IconOptions,
        keyboard: Option<&input::backend::keyboard::KeyboardMouse>,
    ) -> Self {
        Self::with_parts(
            options,
            keyboard.map(|k| k.bindings()).as_ref(),
            keyboard.map(|k| k.active_device()),
        )
    }

    pub fn with_parts(
        options: IconOptions,
        bindings: Option<&Bindings>,
        active: Option<ActiveDevice>,
    ) -> Self {
        Icons {
            choice: options.choice,
            initial: options
                .choice
                .resolve(None, options.gamepads, bindings.is_some()),
            active,
            keys: bindings.map(key_labels).unwrap_or_default(),
            available: Arc::new(Available::all()),
        }
    }

    /// False when the retail glyphs are all that can ever show, so nothing
    /// needs installing.
    pub fn enabled(&self) -> bool {
        self.choice != IconChoice::Set(IconSet::Xbox)
    }

    /// The set showing now: a fixed choice, or for `auto` the last-used
    /// device's set.
    pub fn current(&self) -> IconSet {
        match (
            self.choice,
            self.active.as_ref().and_then(ActiveDevice::get),
        ) {
            (IconChoice::Set(set), _) => set,
            (IconChoice::Auto, Some(DeviceKind::Keyboard)) => IconSet::Keyboard,
            (IconChoice::Auto, Some(DeviceKind::Xbox)) => IconSet::Xbox360,
            (IconChoice::Auto, Some(DeviceKind::PlayStation)) => IconSet::PlayStation,
            (IconChoice::Auto, Some(DeviceKind::Remote)) => IconSet::Remote,
            (IconChoice::Auto, Some(DeviceKind::Steam)) => IconSet::Steam,
            (IconChoice::Auto, None) => self.initial,
        }
    }

    /// The code unit to draw for `unit` right now.
    pub fn shown(&self, unit: u16) -> u16 {
        let shown = shown_unit(self.current(), unit, &self.keys);
        if alias_bit(shown).is_some_and(|bit| !self.available.has(bit)) {
            unit
        } else {
            shown
        }
    }

    pub fn font_rewrite(&self) -> FontRewrite {
        FontRewrite {
            keys: self.keys.clone(),
            available: Arc::clone(&self.available),
        }
    }
}

fn alias_bit(unit: u16) -> Option<u128> {
    font::icons::ALIAS_BASES
        .iter()
        .enumerate()
        .find_map(|(set, (_, base))| {
            let offset = unit.checked_sub(*base)?;
            (offset < BUTTON_UNITS.len() as u16)
                .then(|| 1u128 << (set * BUTTON_UNITS.len() + usize::from(offset)))
        })
}

/// Adds the icon sets to every font file under `maps\fonts`.
pub struct FontRewrite {
    keys: KeyLabels,
    available: Arc<Available>,
}

impl FontRewrite {
    /// The font with the icon sets added, or `None` if it is not a font or
    /// has nothing to add.
    pub fn patch(&self, original: &[u8]) -> Option<Vec<u8>> {
        let patched = (|| {
            let font = font::Font::parse(original).ok()?;
            let new = replacements(&font, &self.keys);
            if new.is_empty() {
                return None;
            }
            let out = font::patch::apply(original, &new).ok()?;
            font::Font::parse(&out).ok()?.validate().ok()?;
            let mask = new
                .iter()
                .filter_map(|r| alias_bit(r.unit))
                .fold(0u128, |mask, bit| mask | bit);
            Some((out, mask))
        })();
        // Every opened font must support a substitution. A missing glyph or
        // declined rewrite disables it globally; never draw a filler or a
        // mod's unrelated glyph. Existing original code points remain intact.
        self.available
            .keep(patched.as_ref().map_or(0, |(_, mask)| *mask));
        patched.map(|(out, _)| out)
    }
}

impl vfs::Rewrite for FontRewrite {
    fn skipped(&mut self, path: &str) {
        self.available.keep(0);
        crate::log::note(&format!(
            "icons: {path} exceeds the rewrite limit; keeping original prompts"
        ));
    }
    fn wants(&self, path: &str) -> bool {
        let p = path.to_ascii_lowercase();
        p.starts_with("maps\\fonts\\") && !p.ends_with(".txt")
    }
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        let out = self.patch(original);
        if out.is_none() {
            crate::log::note(&format!("icons: {path} left as it is"));
        }
        out
    }
}

/// Hook the glyph lookups so a button's code unit draws the showing set's
/// glyph. Each hook rewrites the code-unit argument on the guest stack and
/// then runs the original function.
pub fn install(rt: &runtime::Runtime, icons: Arc<Icons>) -> anyhow::Result<()> {
    let (first, last) = (BUTTON_UNITS[0].0, BUTTON_UNITS[BUTTON_UNITS.len() - 1].0);
    for (addr, name, arg) in [
        (GLYPH_LOOKUP, "boot:icons-glyph-lookup", 4u32),
        (GLYPH_LOOKUP_CALLBACK, "boot:icons-glyph-callback", 0xc),
    ] {
        let original = rt
            .resolve_original(addr)
            .ok_or_else(|| anyhow::anyhow!("icons: no generated function at {addr:#x}"))?;
        let icons = Arc::clone(&icons);
        rt.edit_dispatch(|d| {
            d.register_override(addr, name, move |cpu, ctx, selector| {
                let slot = cpu.esp.wrapping_add(arg);
                let value = ctx.memory().u32(slot)?;
                let unit = value as u16;
                if (first..=last).contains(&unit) {
                    let shown = icons.shown(unit);
                    if shown != unit {
                        ctx.memory()
                            .set_u32(slot, (value & 0xffff_0000) | u32::from(shown))?;
                    }
                }
                Ok(original.invoke(cpu, ctx, selector))
            });
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs::Rewrite;

    fn options(choice: &str, gamepads: bool) -> IconOptions {
        IconOptions {
            choice: choice.parse().unwrap(),
            gamepads,
        }
    }

    #[test]
    fn default_bindings_label_every_button() {
        let k = key_labels(&Bindings::default());
        let key = |s: &str| KeyIcon::Key(s.to_owned());
        assert_eq!(k[&Button::A], key("SPACE"));
        assert_eq!(k[&Button::B], key("V"));
        assert_eq!(k[&Button::Start], key("ESC"));
        assert_eq!(k[&Button::LeftStickClick], key("CTRL"));
        assert_eq!(k[&Button::DpadUp], key("\u{2191}"));
        assert_eq!(k[&Button::RightTrigger], KeyIcon::Mouse(MouseIcon::Left));
        assert_eq!(
            k[&Button::RightStickClick],
            KeyIcon::Mouse(MouseIcon::Right)
        );
        // The stick's four keys read as one cap; mouse look is the right stick.
        assert_eq!(k[&Button::LeftStick], key("WASD"));
        assert_eq!(k[&Button::RightStick], KeyIcon::Mouse(MouseIcon::Move));
        assert_eq!(k.len(), BUTTON_UNITS.len());
    }

    #[test]
    fn labels_follow_the_users_bindings() {
        let b = Bindings::from_toml(
            r#"
            [mouse]
            look = false
            [bind]
            a = ["MouseForward", "Space"]
            b = []
            left_stick_up = ["ArrowUp"]
            left_stick_left = ["ShiftLeft"]
            right_stick_up = ["I"]
            right_stick_left = ["J"]
            right_stick_down = ["K"]
            right_stick_right = ["L"]
            "#,
        )
        .unwrap();
        let k = key_labels(&b);
        // The first input listed is the one shown.
        assert_eq!(k[&Button::A], KeyIcon::Mouse(MouseIcon::Forward));
        // Unbound: show a dash instead of a nonexistent physical button.
        assert_eq!(k[&Button::B], KeyIcon::Key("-".into()));
        // A stick with a long-named key shows its up key alone.
        assert_eq!(k[&Button::LeftStick], KeyIcon::Key("\u{2191}".into()));
        assert_eq!(k[&Button::RightStick], KeyIcon::Key("IJKL".into()));
        // No keys and no mouse look: the right stick is unbound.
        let none = Bindings::from_toml("[mouse]\nlook = false").unwrap();
        assert_eq!(
            key_labels(&none)[&Button::RightStick],
            KeyIcon::Key("-".into())
        );
    }

    #[test]
    fn fixed_choice_ignores_the_device_and_xbox_installs_nothing() {
        let active = ActiveDevice::default();
        active.set(DeviceKind::Keyboard);
        let b = Bindings::default();
        let ps = Icons::with_parts(options("playstation", true), Some(&b), Some(active.clone()));
        assert!(ps.enabled());
        assert_eq!(ps.current(), IconSet::PlayStation);
        assert_eq!(ps.shown(0xe100), 0xe220);
        // The trigger pair is labelled L2/R2 on a PlayStation pad.
        assert_eq!(ps.shown(0xe106), 0xe226);
        let xbox = Icons::with_parts(options("xbox", true), Some(&b), Some(active));
        assert!(!xbox.enabled());
        assert_eq!(xbox.shown(0xe100), 0xe100);
    }

    #[test]
    fn auto_follows_the_last_used_device() {
        let active = ActiveDevice::default();
        let b = Bindings::default();
        let icons = Icons::with_parts(options("auto", true), Some(&b), Some(active.clone()));
        assert!(icons.enabled());
        // Until device discovery runs, show the available keyboard.
        assert_eq!(icons.current(), IconSet::Keyboard);
        assert_eq!((icons.shown(0xe104), icons.shown(0xe100)), (0xe244, 0xe240));
        active.set(DeviceKind::Keyboard);
        assert_eq!((icons.shown(0xe104), icons.shown(0xe100)), (0xe244, 0xe240));
        active.set(DeviceKind::PlayStation);
        assert_eq!((icons.shown(0xe104), icons.shown(0xe100)), (0xe224, 0xe220));
        active.set(DeviceKind::Xbox);
        assert_eq!(icons.current(), IconSet::Xbox360);
        active.set(DeviceKind::Remote);
        assert_eq!(icons.current(), IconSet::Remote);
        assert_eq!(
            (icons.shown(0xe100), icons.shown(0xe10c)),
            (0xe260, 0xe26c),
            "A and Start"
        );
        assert_eq!(
            icons.shown(0xe102),
            0xe102,
            "X: the remote has none, so Xbox"
        );
        active.set(DeviceKind::Steam);
        assert_eq!(icons.current(), IconSet::Steam);
        assert_eq!(
            (
                icons.shown(0xe104),
                icons.shown(0xe10c),
                icons.shown(0xe10f)
            ),
            (0xe284, 0xe28c, 0xe28f),
            "Black, Start, right stick"
        );
        assert_eq!(icons.shown(0xe100), 0xe100, "A stays Xbox");
        active.set(DeviceKind::Xbox);
        // Units that are not buttons are never touched.
        for unit in [0x41, 0xe0ff, 0xe112, 0xe200] {
            assert_eq!(icons.shown(unit), unit);
        }
        // Without pads, auto starts on the keyboard; with neither, on retail.
        let kb = Icons::with_parts(options("auto", false), Some(&b), None);
        assert_eq!(kb.current(), IconSet::Keyboard);
        let headless = Icons::with_parts(options("auto", false), None, None);
        assert_eq!(headless.current(), IconSet::Xbox);
    }

    #[test]
    fn availability_covers_every_set_past_the_first_word() {
        let a = Available::all();
        // The remote's right stick click: bit 3 * 18 + 17 = 71.
        let remote_a = alias_bit(0xe271).unwrap();
        let ps_a = alias_bit(0xe220).unwrap();
        assert!(
            remote_a >> 64 != 0,
            "the fourth set's last bits are in the second word"
        );
        assert!(a.has(remote_a) && a.has(ps_a));
        a.keep(!remote_a);
        assert!(!a.has(remote_a) && a.has(ps_a));
        a.keep(0);
        assert!(!a.has(ps_a));
        let units = font::icons::ALIAS_BASES.len() * BUTTON_UNITS.len();
        assert!(units <= 128);
        let mut seen = 0u128;
        for (_, base) in font::icons::ALIAS_BASES {
            for i in 0..BUTTON_UNITS.len() as u16 {
                let bit = alias_bit(base + i).unwrap();
                assert_eq!(seen & bit, 0, "{:#x}", base + i);
                seen |= bit;
            }
        }
        assert_eq!(seen.count_ones() as usize, units);
    }

    #[test]
    fn rewrite_wants_only_font_files() {
        let r = Icons::with_parts(options("auto", true), None, None).font_rewrite();
        assert!(r.wants("maps\\fonts\\conduit-16"));
        assert!(r.wants("MAPS\\FONTS\\Arial-13"));
        assert!(!r.wants("maps\\fonts\\font_table.txt"));
        assert!(!r.wants("maps\\mainmenu.map"));
        assert!(!r.wants("fonts\\conduit-16"));
    }

    #[test]
    fn rewrite_leaves_non_fonts_alone() {
        let mut r = Icons::with_parts(options("auto", true), None, None).font_rewrite();
        assert_eq!(r.rewrite("maps\\fonts\\junk", b"not a font"), None);
        assert_eq!(r.rewrite("maps\\fonts\\empty", &[]), None);
    }
    // A synthetic font, with blank record 0 and solid button records. No
    // disc data is used by the rewrite/hook contract tests.
    fn synthetic_font(units: &[u16]) -> Vec<u8> {
        let end = font::RECORDS_OFFSET + (units.len() + 1) * font::RECORD_LEN;
        let mut data = vec![0; end];
        let put = |data: &mut Vec<u8>, at: usize, n: u32| {
            data[at..at + 4].copy_from_slice(&n.to_le_bytes())
        };
        put(&mut data, font::HEADER_OFFSET, font::MAGIC);
        put(&mut data, font::HEADER_OFFSET + 4, 18);
        put(
            &mut data,
            font::HEADER_OFFSET + 0xc,
            (units.len() + 1) as u32,
        );
        for (i, unit) in units.iter().enumerate() {
            put(
                &mut data,
                font::CHARMAP_OFFSET + usize::from(*unit) * 4,
                (i + 1) as u32,
            );
            let at = font::RECORDS_OFFSET + (i + 1) * font::RECORD_LEN;
            let stream = font::encode::encode_pixels(&[0xffff; 16 * 18]);
            for (off, n) in [
                (0, 16u16),
                (2, stream.len() as u16),
                (4, 16),
                (6, 18),
                (10, 18),
            ] {
                data[at + off..at + off + 2].copy_from_slice(&n.to_le_bytes());
            }
            let offset = data.len() as u32;
            put(&mut data, at + 12, offset);
            data.extend(stream);
        }
        let size = (data.len() - end) as u32;
        put(&mut data, font::HEADER_OFFSET + 0x18, size);
        data
    }

    #[test]
    fn failed_rewrite_disables_alias_lookup() {
        let icons = Icons::with_parts(options("playstation", false), None, None);
        let rewrite = icons.font_rewrite();
        assert_eq!(icons.shown(0xe100), 0xe220);
        assert!(rewrite.patch(b"invalid font").is_none());
        assert_eq!(icons.shown(0xe100), 0xe100);
    }

    #[test]
    fn only_successfully_written_aliases_are_selected() {
        let icons = Icons::with_parts(options("playstation", false), None, None);
        let rewrite = icons.font_rewrite();
        let bytes = rewrite.patch(&synthetic_font(&[0xe100])).unwrap();
        let f = font::Font::parse(&bytes).unwrap();
        f.validate().unwrap();
        assert_eq!(icons.shown(0xe100), 0xe220);
        assert_ne!(f.glyph_index(icons.shown(0xe100)), 0);
        assert_eq!(icons.shown(0xe101), 0xe101);
        // A later font must not re-enable an alias absent from an earlier one.
        rewrite.patch(&synthetic_font(&[0xe100, 0xe101])).unwrap();
        assert_eq!(icons.shown(0xe101), 0xe101);
    }

    #[test]
    fn occupied_alias_keeps_original_lookup_and_file() {
        let icons = Icons::with_parts(options("playstation", false), None, None);
        let bytes = synthetic_font(&[0xe100, 0xe220]);
        assert!(icons.font_rewrite().patch(&bytes).is_none());
        assert_eq!(icons.shown(0xe100), 0xe100);
    }

    #[test]
    fn hooks_rewrite_only_the_code_unit_and_delegate_to_the_original() {
        use runtime::guest::Permissions;
        use runtime::{
            CpuAbi, Dispatch, ExecCtx, ExecCtxOpaque, Guest, Runner, Runtime, TransferAbi,
        };
        unsafe extern "C" fn original(
            cpu: *mut CpuAbi,
            _: *mut ExecCtxOpaque,
            selector: u32,
        ) -> TransferAbi {
            // SAFETY: the runtime supplies its live CPU for the call.
            unsafe {
                (*cpu).ecx = selector;
            }
            TransferAbi::stop(0)
        }
        let mut guest = Guest::new();
        guest.map_zeroed(0x1000, 0x1000, Permissions::RW).unwrap();
        guest.map_zeroed(0x140000, 0x1000, Permissions::RX).unwrap();
        guest.map_zeroed(0x1f000, 0x1000, Permissions::RX).unwrap();
        let mut dispatch = Dispatch::default();
        for addr in [GLYPH_LOOKUP, GLYPH_LOOKUP_CALLBACK] {
            // SAFETY: the synthetic original follows the generated-handler ABI.
            unsafe { dispatch.register_generated(addr, runtime::cpu::ABI_VERSION, original) }
                .unwrap();
        }
        let rt = Runtime::new(guest, dispatch);
        let active = ActiveDevice::default();
        let icons = Arc::new(Icons::with_parts(
            options("auto", true),
            Some(&Bindings::default()),
            Some(active.clone()),
        ));
        install(&rt, icons).unwrap();
        for kind in [
            DeviceKind::PlayStation,
            DeviceKind::Keyboard,
            DeviceKind::Xbox,
            DeviceKind::Remote,
            DeviceKind::Steam,
        ] {
            active.set(kind);
            let expected = match kind {
                DeviceKind::PlayStation => 0xe220,
                DeviceKind::Keyboard => 0xe240,
                DeviceKind::Xbox => 0xe100,
                DeviceKind::Remote => 0xe260,
                // Steam keeps the retail A.
                DeviceKind::Steam => 0xe100,
            };
            for (addr, arg) in [(GLYPH_LOOKUP, 4), (GLYPH_LOOKUP_CALLBACK, 12)] {
                for unit in [0xe100, 0x41] {
                    for offset in [4, 8, 12, 16] {
                        rt.memory().set_u32(0x1800 + offset, 0x1234).unwrap();
                    }
                    rt.memory()
                        .set_u32(0x1800 + arg, 0xabcd0000 | unit)
                        .unwrap();
                    let mut cpu = CpuAbi {
                        esp: 0x1800,
                        eax: 7,
                        ..Default::default()
                    };
                    let mut ctx = ExecCtx::new(Arc::clone(&rt), 1);
                    Runner::default().run(&mut cpu, &mut ctx, addr);
                    assert_eq!(
                        cpu.ecx, addr,
                        "original must execute with its entry selector"
                    );
                    assert_eq!((cpu.eax, cpu.esp), (7, 0x1800));
                    for offset in [4, 8, 12, 16] {
                        let want = if offset == arg {
                            0xabcd0000 | if unit == 0xe100 { expected } else { unit }
                        } else {
                            0x1234
                        };
                        assert_eq!(rt.memory().u32(0x1800 + offset).unwrap(), want);
                    }
                }
            }
        }
    }
    #[test]
    fn an_oversized_font_disables_aliases_too() {
        let icons = Icons::with_parts(options("playstation", false), None, None);
        icons.font_rewrite().skipped("maps\\fonts\\large");
        assert_eq!(icons.shown(0xe100), 0xe100);
    }
}
