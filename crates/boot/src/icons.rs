//! Button icons for the device in use (`docs/button-icons.md`).
//!
//! Two parts. [`FontRewrite`] serves the disc's font files with extra glyphs:
//! each icon set of `font::icons` at code units of its own, next to the
//! retail button glyphs. [`install`] hooks the game's glyph lookups and, for
//! a button's code unit, substitutes the unit of the set that is showing.
//! The glyph cache is keyed by code unit, so changing set needs no flush: the
//! next string drawn looks up the other units.
use std::sync::Arc;

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
    /// Physical pads are enabled, so `auto` starts on pad icons.
    pub gamepads: bool,
}

/// `H2_ICONS`: `auto`, `xbox`, `xbox360`, `playstation` or `keyboard`.
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
/// stick shows the mouse when mouse look drives it. An unbound control has no
/// entry and keeps its retail glyph.
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
        if let Some(icon) = icon {
            out.insert(b, icon);
        }
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
            (IconChoice::Auto, None) => self.initial,
        }
    }

    /// The code unit to draw for `unit` right now.
    pub fn shown(&self, unit: u16) -> u16 {
        shown_unit(self.current(), unit, &self.keys)
    }

    pub fn font_rewrite(&self) -> FontRewrite {
        FontRewrite {
            keys: self.keys.clone(),
        }
    }
}

/// Adds the icon sets to every font file under `maps\fonts`.
pub struct FontRewrite {
    keys: KeyLabels,
}

impl FontRewrite {
    /// The font with the icon sets added, or `None` if it is not a font or
    /// has nothing to add.
    pub fn patch(&self, original: &[u8]) -> Option<Vec<u8>> {
        let font = font::Font::parse(original).ok()?;
        let new = replacements(&font, &self.keys);
        if new.is_empty() {
            return None;
        }
        font::patch::apply(original, &new).ok()
    }
}

impl vfs::Rewrite for FontRewrite {
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
        // Unbound: no entry, so the retail glyph stays.
        assert!(!k.contains_key(&Button::B));
        // A stick with a long-named key shows its up key alone.
        assert_eq!(k[&Button::LeftStick], KeyIcon::Key("\u{2191}".into()));
        assert_eq!(k[&Button::RightStick], KeyIcon::Key("IJKL".into()));
        // No keys and no mouse look: nothing to show for the right stick.
        let none = Bindings::from_toml("[mouse]\nlook = false").unwrap();
        assert!(!key_labels(&none).contains_key(&Button::RightStick));
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
        // Triggers keep the retail picture on a PlayStation pad.
        assert_eq!(ps.shown(0xe106), 0xe106);
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
        // Before any input: pads are enabled, so pad icons.
        assert_eq!(icons.current(), IconSet::Xbox360);
        assert_eq!((icons.shown(0xe104), icons.shown(0xe100)), (0xe204, 0xe100));
        active.set(DeviceKind::Keyboard);
        assert_eq!((icons.shown(0xe104), icons.shown(0xe100)), (0xe244, 0xe240));
        active.set(DeviceKind::PlayStation);
        assert_eq!((icons.shown(0xe104), icons.shown(0xe100)), (0xe224, 0xe220));
        active.set(DeviceKind::Xbox);
        assert_eq!(icons.current(), IconSet::Xbox360);
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
}
