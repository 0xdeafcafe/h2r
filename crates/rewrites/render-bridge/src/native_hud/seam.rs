//! The native override seam of the HUD walk (docs/graphics/native-hud-overlay.md,
//! "The override seam"): where a feature changes what the Rust walk
//! ([`super::walk`]) shows and lays out, as it used to change the guest's
//! helpers.
//!
//! The walk asks the seams, in rank order, five questions about a bitmap
//! or text widget; the first seam that answers wins and the walk's own
//! answer (`hud_kit_core::walk`) is the fallback:
//!
//! | Question | The walk's own answer | What an answer replaces |
//! |---|---|---|
//! | `gate` | `walk::gate` on the widget's state bytes | whether the widget is shown |
//! | `inputs` | `walk::inputs` on its four input bytes | the four shader inputs |
//! | `string` | the string table, or the number/name | a text widget's UTF-16 units and engine font |
//! | `sprite` | the bitmap's own pick (`0x0022ae21`) | a bitmap widget's image, size and bounds |
//! | `scale` | none | factors on the basis' two axes, before the widget's effects |
//!
//! A read a seam makes that faults panics, named for the seam and the
//! question.

use std::sync::{Arc, Mutex};

use guest_layout::memory::{GuestMemory, MemFault};
pub use scene_extract::hud::walk::Sprite;

/// The widget a seam is asked about, and the walk it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    /// The widget's address (its state bytes are at `+8`, its inputs at `+4`).
    pub widget: u32,
    /// The walk's player (its first argument).
    pub player: u32,
    /// The walk's weapon record in the HUD record, when its slot has one.
    pub weapon: Option<u32>,
    /// The walk's state word D.
    pub word_d: u16,
}

/// A string a seam gives a text widget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    /// UTF-16, without its zero.
    pub units: Vec<u16>,
    /// The engine font.
    pub font: i16,
}

/// A feature's answers; each defaults to the walk's own.
pub trait Seam: Send + Sync {
    fn gate(&self, _mem: &dyn GuestMemory, _at: &At) -> Result<Option<bool>, MemFault> {
        Ok(None)
    }
    fn inputs(&self, _mem: &dyn GuestMemory, _at: &At) -> Result<Option<[f32; 4]>, MemFault> {
        Ok(None)
    }
    /// `inputs` are the widget's, as the walk draws it.
    fn string(&self, _mem: &dyn GuestMemory, _at: &At, _inputs: &[f32; 4]) -> Result<Option<Text>, MemFault> {
        Ok(None)
    }
    fn sprite(&self, _mem: &dyn GuestMemory, _at: &At) -> Result<Option<Sprite>, MemFault> {
        Ok(None)
    }
    /// Factors for the basis' x axis's x and y axis's y.
    fn scale(&self, _mem: &dyn GuestMemory, _at: &At) -> Result<Option<[f32; 2]>, MemFault> {
        Ok(None)
    }
}

struct Entry {
    rank: u8,
    name: &'static str,
    seam: Arc<dyn Seam>,
}

static SEAMS: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// Registers `seam` under `name`, replacing the one of that name; seams
/// answer in `rank` order, then registration order.
pub fn register(rank: u8, name: &'static str, seam: Arc<dyn Seam>) {
    let mut all = SEAMS.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|e| e.name != name);
    all.push(Entry { rank, name, seam });
    all.sort_by_key(|e| e.rank);
}

fn first<T>(question: &str, f: impl Fn(&dyn Seam) -> Result<Option<T>, MemFault>) -> Option<T> {
    let all: Vec<(&'static str, Arc<dyn Seam>)> = SEAMS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|e| (e.name, Arc::clone(&e.seam))).collect();
    for (name, seam) in all {
        match f(&*seam) {
            Ok(Some(answer)) => return Some(answer),
            Ok(None) => {}
            Err(fault) => panic!("hud seam {name}: its {question} read faulted: {fault:?}"),
        }
    }
    None
}

pub fn gate(mem: &dyn GuestMemory, at: &At) -> Option<bool> {
    first("gate", |s| s.gate(mem, at))
}

pub fn inputs(mem: &dyn GuestMemory, at: &At) -> Option<[f32; 4]> {
    first("inputs", |s| s.inputs(mem, at))
}

pub fn string(mem: &dyn GuestMemory, at: &At, inputs: &[f32; 4]) -> Option<Text> {
    first("string", |s| s.string(mem, at, inputs))
}

pub fn sprite(mem: &dyn GuestMemory, at: &At) -> Option<Sprite> {
    first("sprite", |s| s.sprite(mem, at))
}

pub fn scale(mem: &dyn GuestMemory, at: &At) -> Option<[f32; 2]> {
    first("scale", |s| s.scale(mem, at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    struct Hides(u32);
    impl Seam for Hides {
        fn gate(&self, _: &dyn GuestMemory, at: &At) -> Result<Option<bool>, MemFault> {
            Ok((at.widget == self.0).then_some(false))
        }
    }

    struct Shows;
    impl Seam for Shows {
        fn gate(&self, _: &dyn GuestMemory, at: &At) -> Result<Option<bool>, MemFault> {
            // Only the test's own widgets: the registry is shared by the process.
            Ok((at.widget >> 20 == 0x0aa).then_some(true))
        }
    }

    fn at(widget: u32) -> At {
        At { widget, player: 0, weapon: None, word_d: 0 }
    }

    #[test]
    fn the_first_seam_by_rank_that_answers_wins_and_a_name_registers_once() {
        let m = FakeGuest::new();
        register(5, "test-shows", Arc::new(Shows));
        register(1, "test-hides", Arc::new(Hides(0x0aa0_0000)));
        assert_eq!(gate(&m, &at(0x0aa0_0000)), Some(false));
        assert_eq!(gate(&m, &at(0x0aa0_0064)), Some(true));
        // The same name again replaces its seam.
        register(1, "test-hides", Arc::new(Hides(0x0aa0_0064)));
        assert_eq!(gate(&m, &at(0x0aa0_0000)), Some(true));
        assert_eq!(gate(&m, &at(0x0aa0_0064)), Some(false));
        assert_eq!(inputs(&m, &at(0x0aa0_0000)), None);
        let all = SEAMS.lock().unwrap();
        assert_eq!(all.iter().filter(|e| e.name == "test-hides").count(), 1);
    }
}
