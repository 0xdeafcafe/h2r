//! The game state the preview draws a HUD at: the screen mode, health and
//! shield, grenades, ammunition, zoom; the widgets' inputs it gives; and
//! the engine's per-widget state gate.

use std::collections::BTreeMap;

use crate::layout::Mode;

/// Halo 2's grenade types, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Grenade {
    #[default]
    Frag,
    Plasma,
}

/// What the HUD is drawn at.
#[derive(Clone, Debug, PartialEq)]
pub struct HudState {
    pub mode: Mode,
    /// 0..1 each.
    pub shield: f32,
    pub body: f32,
    pub grenade: Grenade,
    pub frags: u8,
    pub plasmas: u8,
    pub rounds_loaded: u16,
    pub magazine: u16,
    pub rounds_reserve: u16,
    /// 0..1.
    pub heat: f32,
    /// 0..1 left.
    pub battery: f32,
    pub zoomed: bool,
    /// Gate widgets by their state flags (off: every widget is drawn).
    pub gate: bool,
    /// The gate's four state words (A, B, C, D: the gate's arguments, E
    /// `0x13a58a`..`0x13a5a2`), before [`HudState::masks`] adds what the
    /// state above is known to set.
    pub words: [u16; 4],
    /// Input values set by hand, by input index (for inputs whose source
    /// is not known).
    pub inputs: BTreeMap<u8, f32>,
}

impl Default for HudState {
    fn default() -> Self {
        HudState {
            mode: Mode::Full,
            shield: 1.0,
            body: 1.0,
            grenade: Grenade::Frag,
            frags: 2,
            plasmas: 2,
            rounds_loaded: 36,
            magazine: 36,
            rounds_reserve: 108,
            heat: 0.0,
            battery: 1.0,
            zoomed: false,
            gate: false,
            words: [0; 4],
            inputs: BTreeMap::new(),
        }
    }
}

/// Where an input's value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Read from the engine (`0x13ad48`'s case) and set by the state.
    Known(&'static str),
    /// Set by hand in the state panel.
    Manual,
    /// Not known: the preview uses 1.
    Unknown,
}

/// The inputs whose source is known (E `0x13ad48`, docs/halo1/health.md
/// and docs/halo1/hud.md).
pub fn input_name(n: u8) -> Option<&'static str> {
    Some(match n {
        0 => "zero",
        1 => "one",
        16 => "shield",
        17 => "body",
        20 => "frag grenades",
        21 => "plasma grenades",
        0x31 => "heat",
        0x36 => "rounds loaded / magazine",
        0x38 => "battery",
        _ => return None,
    })
}

impl HudState {
    /// Input `n`'s value (E `0x13ad48`) and where it came from.
    pub fn input(&self, n: u8) -> (f32, Source) {
        if let Some(v) = self.inputs.get(&n) {
            return (*v, Source::Manual);
        }
        let v = match n {
            0 => 0.0,
            1 => 1.0,
            16 => self.shield,
            17 => self.body,
            20 => f32::from(self.frags),
            21 => f32::from(self.plasmas),
            0x31 => self.heat,
            0x36 => f32::from(self.rounds_loaded) / f32::from(self.magazine.max(1)),
            0x38 => self.battery,
            _ => return (1.0, Source::Unknown),
        };
        (v, Source::Known(input_name(n).unwrap_or("")))
    }

    /// The gate's four words: [`HudState::words`] with the bits the state
    /// sets. Word A: `0x04` the frag grenade selected, `0x08` the plasma
    /// grenade (V: retail `ui\hud\masterchief`'s grenade widgets and
    /// counts show on exactly these); `0x40` not zoomed (V: the retail
    /// scope masks hide on it and show only zoomed; its meaning is read
    /// from them, not from the code that sets it).
    pub fn masks(&self) -> [u16; 4] {
        let mut w = self.words;
        w[0] &= !(0x04 | 0x08 | 0x40);
        w[0] |= match self.grenade {
            Grenade::Frag => 0x04,
            Grenade::Plasma => 0x08,
        };
        if !self.zoomed {
            w[0] |= 0x40;
        }
        w
    }
}

/// The engine's widget gate (E `0x13ac87`, on the 20 state bytes at widget
/// `+0x08`): shown when one of the four show words meets its state word
/// and none of the four hide words does. (Bytes `+0x10..+0x12` of the
/// state are thresholds the engine turns into word C's bits `0x08`,
/// `0x10`, `0x20` from the weapon's state, `0x13ac8f`..`0x13acd3`; the
/// preview takes word C as given.)
pub fn shown(state: &[u8], words: [u16; 4]) -> bool {
    let w = |i: usize| u16::from_le_bytes([state[2 * i], state[2 * i + 1]]);
    let any = |base: usize| (0..4).any(|i| w(base + i) & words[i] != 0);
    any(0) && !any(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state flags the Halo 1 converter copies from retail widgets
    /// (`h1objects::h1hud`, V): the frag grenade widgets show with word A
    /// `0x04` and not with `0x08`; the scope masks only zoomed.
    #[test]
    fn the_gate_reads_show_then_hide_words() {
        let frag: [u8; 20] = [0x04, 0, 0, 0, 0, 0, 0, 0, 0x2a, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let plasma: [u8; 20] = [0x08, 0, 0, 0, 0, 0, 0, 0, 0x26, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let scope: [u8; 20] = [
            0x90, 0x01, 0, 0, 0x01, 0, 0, 0, 0x40, 0, 0, 0, 0x04, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut s = HudState::default();
        assert!(shown(&frag, s.masks()));
        assert!(!shown(&plasma, s.masks()));
        s.grenade = Grenade::Plasma;
        assert!(!shown(&frag, s.masks()));
        assert!(shown(&plasma, s.masks()));
        // The scope: shows on word A 0x0190 or C 0x0001, hides on A 0x40.
        s.words = [0x0100, 0, 0, 0];
        assert!(!shown(&scope, s.masks()), "unzoomed");
        s.zoomed = true;
        assert!(shown(&scope, s.masks()), "zoomed");
        // Nothing to show on: hidden.
        assert!(!shown(&[0; 20], s.masks()));
    }

    #[test]
    fn inputs_come_from_the_state() {
        let mut s = HudState {
            shield: 0.25,
            body: 0.5,
            rounds_loaded: 9,
            magazine: 36,
            ..HudState::default()
        };
        assert_eq!(s.input(16), (0.25, Source::Known("shield")));
        assert_eq!(s.input(17).0, 0.5);
        assert_eq!(s.input(0x36).0, 0.25);
        assert_eq!(s.input(20).0, 2.0);
        assert_eq!(s.input(99), (1.0, Source::Unknown));
        s.inputs.insert(99, 0.3);
        assert_eq!(s.input(99), (0.3, Source::Manual));
    }
}
