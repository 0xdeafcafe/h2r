//! The start layer's records on the sound table
//! (docs/engine/sound-instances.md, "The start layer").
//!
//! A [`Start`] is one row of retail's `"sounds effects"` array
//! (`[0x51ebe0]`, 16 rows of 20 bytes, made by `0x21d490`): a sound tag's
//! custom playback effect, whose components (`snd!` sounds and `lsnd`
//! loops) it starts, keeps and stops. The rows live in the table's start
//! pool, beside the instances and the loops, and are saved and hashed with
//! them. [`plan`] is `0x21d630`'s choice of what each component does in a
//! [`Mode`]; the bridge makes the admissions and loop calls it names.
//!
//! ```text
//! 0x21d2c0 tag ──> Start::Tag ──┐
//! 0x21d110 sound ─> Admitting ──┴─> run(Start) ── walk 0x21d390: run(Update) ──> stop: run(Stop), done ──> freed
//!                    └ refused: freed (0x21dcf0)
//! ```

use sim_world::{Handle, Pool, Reader, Record, RestoreError, Writer};

use crate::instances::{Sounds, TagIndex};

/// Rows in the start pool: retail's `0x10` (`0x21d497`).
pub const CAPACITY: u32 = 16;
/// `"sounds effects"` (`0x4570fc`) seeds the first salt.
pub const POOL_NAME: u16 = u16::from_le_bytes(*b"so");
/// A tag start keys its loops on its own handle with this bit
/// (`0x21d908`, `0x21dbb5`).
pub const OWNER_BIT: u32 = 0x4000;
/// The component groups `0x21d630` runs: `snd!` and `lsnd`.
pub const SOUND_GROUP: u32 = 0x736e_6421;
pub const LOOP_GROUP: u32 = 0x6c73_6e64;
/// `0x44a200`: the loop call's mode for each [`Mode`].
pub const LOOP_MODES: [u32; 3] = [0, 1, 2];
/// `0x44a20c`: a dialogue start's loop mode by its row's byte `+2`; -1
/// reads the word before it (`0x44a208`, 2).
pub const DIALOGUE_LOOP_MODES: [u32; 4] = [0, 1, 2, 2];
/// The callback table of a sound a start admits (`0x44a1e0`: alive
/// `0x21d970` only) and of the sound a start belongs to (`0x44a1c0`).
pub const STARTED_TABLE: u32 = 0x0044_a1e0;
pub const SOUND_TABLE: u32 = 0x0044_a1c0;

/// What a start runs for: the record's kind byte (`+3`) and its word
/// `+0xC`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StartOf {
    /// Kind 0, `0x21d2c0`: a tag's effect with no sound of its own. The
    /// scale is the loops' source word 1 (`0x21dc4a`).
    Tag { scale: f32 },
    /// Kind 1, `0x21d110`: the effect of an admitted sound, its handle.
    Sound(Handle),
    /// Kind 2: a dialogue row (`[0x4ed28c]`, 24 bytes a row), its handle.
    Dialogue(u32),
    /// `0x21d110` between taking the row and the admission's answer: no
    /// kind yet. It is never walked: the admission runs inside the entry.
    Admitting,
}

/// One row of the start pool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Start {
    pub of: StartOf,
    /// `+6`: the request's word `+4` (kind 1) or the caller's argument
    /// (kind 0). Each component's request and loop call takes it.
    pub word: u16,
    /// `+0x10`: the custom playback row's effect (map data: component count
    /// `+8`, components `+0xC`, 16 bytes each), the address `0x21dd30`
    /// answers. Static while the map is loaded.
    pub effect: u32,
    /// Flag bit 0: stopped; the next walk frees the row.
    pub done: bool,
    /// Flag bit 1: its sound ended for a reason other than playing out
    /// (`0x21daee`). A stopping loop with component flag bit 4 then plays
    /// its alternate end.
    pub cut: bool,
    /// Flag bit 3: its sound's own owner is gone, or the request had none
    /// (`0x21dc91`, `0x21d614`).
    pub released: bool,
    /// Flag bit 4: the stop has begun (`0x21d374`, `0x21db11`).
    pub stopping: bool,
}

impl Start {
    /// A row as `0x21d2c0` makes it, kind 0.
    pub fn tag(effect: u32, scale: f32, word: u16) -> Self {
        Self {
            of: StartOf::Tag { scale },
            word,
            effect,
            done: false,
            cut: false,
            released: false,
            stopping: false,
        }
    }

    /// A row as `0x21d110` takes it before the admission.
    pub fn admitting(effect: u32) -> Self {
        Self {
            of: StartOf::Admitting,
            word: 0,
            effect,
            done: false,
            cut: false,
            released: false,
            stopping: false,
        }
    }

    /// The flag byte `+4` in retail's bits.
    pub fn flags(&self) -> u8 {
        u8::from(self.done)
            | (u8::from(self.cut) << 1)
            | (u8::from(self.released) << 3)
            | (u8::from(self.stopping) << 4)
    }

    /// The owner its loops are keyed on (`0x21d903` to `0x21d90f`): a tag
    /// start's own handle with [`OWNER_BIT`], else its word `+0xC`.
    pub fn loop_owner(&self, handle: Handle) -> u32 {
        match self.of {
            StartOf::Tag { .. } | StartOf::Admitting => handle.raw() | OWNER_BIT,
            StartOf::Sound(sound) => sound.raw(),
            StartOf::Dialogue(row) => row,
        }
    }
}

const KIND_ADMITTING: u8 = 0xff;

impl Record for Start {
    fn write(&self, w: &mut Writer) {
        let (kind, word) = match self.of {
            StartOf::Tag { scale } => (0, scale.to_bits()),
            StartOf::Sound(sound) => (1, sound.raw()),
            StartOf::Dialogue(row) => (2, row),
            StartOf::Admitting => (KIND_ADMITTING, 0),
        };
        w.u8(kind);
        w.u32(word);
        w.u16(self.word);
        w.u32(self.effect);
        w.u8(self.flags());
    }

    fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        let bad = |what| RestoreError::Corrupt {
            kind: sim_world::kind::SOUND,
            what,
        };
        let kind = r.u8()?;
        let word = r.u32()?;
        let of = match kind {
            0 => StartOf::Tag {
                scale: f32::from_bits(word),
            },
            1 => StartOf::Sound(Handle::from_raw(word)),
            2 => StartOf::Dialogue(word),
            KIND_ADMITTING => StartOf::Admitting,
            _ => return Err(bad("start kind")),
        };
        let word = r.u16()?;
        let effect = r.u32()?;
        let flags = r.u8()?;
        if flags & !0x1b != 0 {
            return Err(bad("start flags"));
        }
        Ok(Self {
            of,
            word,
            effect,
            done: flags & 1 != 0,
            cut: flags & 2 != 0,
            released: flags & 8 != 0,
            stopping: flags & 0x10 != 0,
        })
    }
}

/// `0x21d630`'s second argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// From the start entries: the sounds without component flag bit 0,
    /// the loops started.
    Start = 0,
    /// From the walk: no sounds, the loops given the source again.
    Update = 1,
    /// From the stop: the sounds with component flag bit 1, the loops
    /// stopped.
    Stop = 2,
}

/// One component of an effect (16 bytes: group, tag, gain, flags).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Component {
    pub group: u32,
    pub tag: TagIndex,
    /// `+8`: a sound's gain base, a loop's added source offset.
    pub gain: f32,
    pub flags: u32,
}

/// What `0x21d630` does for one component.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// The gate, the rule and `0x126000` on a request record: flags `0x95`,
    /// the start's word at `+4`, its source at `+0xC`, `gain` as the gain
    /// base (`+0x94`). `owned` (start mode) adds flag bit 5 with
    /// [`STARTED_TABLE`], the payload `+0x28` the start and `+0x30` the word
    /// `0x30`, which a stop leaves out.
    Admit {
        tag: TagIndex,
        word: u16,
        gain: f32,
        owned: bool,
    },
    /// `0x21a250(tag, word, owner, _, source, mode, flags, 0)`, the source
    /// the start's with `gain` added to its word 2 (`0x21d8fa`).
    Loop {
        tag: TagIndex,
        word: u16,
        owner: u32,
        gain: f32,
        mode: u32,
        flags: u32,
    },
}

/// The dialogue row's inputs to a kind 2 start's loops: its byte `+2`
/// (`movsx`) and flag bit 4 of its byte `+4` (`0x21d6a4`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DialogueRow {
    pub mode: i8,
    pub alternate: bool,
}

/// `0x21d630`'s choice for each component of `start` in `mode`, in
/// component order. `loop_flags` is a loop tag's flag word (`[tag] + 0`):
/// bit 3 clears the word's bit 0, bit 6 its bit 1. A component with tag -1
/// or another group does nothing; so does a dialogue loop whose row byte
/// is outside [`DIALOGUE_LOOP_MODES`].
pub fn plan(
    handle: Handle,
    start: &Start,
    mode: Mode,
    components: &[Component],
    loop_flags: &dyn Fn(TagIndex) -> u32,
    dialogue: DialogueRow,
) -> Vec<Action> {
    let mut out = Vec::new();
    for c in components {
        if c.tag.0 == u32::MAX {
            continue;
        }
        match c.group {
            SOUND_GROUP => {
                let plays = match mode {
                    Mode::Start => c.flags & 1 == 0,
                    Mode::Update => false,
                    Mode::Stop => c.flags & 2 != 0,
                };
                if plays {
                    out.push(Action::Admit {
                        tag: c.tag,
                        word: start.word,
                        gain: c.gain,
                        owned: mode != Mode::Stop,
                    });
                }
            }
            LOOP_GROUP => {
                let dialogue_start = matches!(start.of, StartOf::Dialogue(_));
                let (loop_mode, alternate) = if c.flags & 0x20 != 0 && dialogue_start {
                    let index = if dialogue.mode == -1 {
                        Some(2)
                    } else {
                        usize::try_from(dialogue.mode).ok()
                    };
                    let Some(m) = index.and_then(|i| DIALOGUE_LOOP_MODES.get(i)) else {
                        continue;
                    };
                    (*m, dialogue.alternate)
                } else {
                    let alternate =
                        c.flags & 8 != 0 || (start.stopping && start.cut && c.flags & 0x10 != 0);
                    (LOOP_MODES[mode as usize], alternate)
                };
                let tag_flags = loop_flags(c.tag);
                let mut word = start.word;
                if tag_flags & 8 != 0 {
                    word &= !1;
                }
                if tag_flags & 0x40 != 0 {
                    word &= !2;
                }
                let flags = u32::from(alternate) | if mode == Mode::Stop { 2 } else { 0 };
                out.push(Action::Loop {
                    tag: c.tag,
                    word,
                    owner: start.loop_owner(handle),
                    gain: c.gain,
                    mode: loop_mode,
                    flags,
                });
            }
            _ => {}
        }
    }
    out
}

/// `0x127f10`'s answer for a sound start's sound at the walk: `update` its
/// `al`, `gone` its out bit 5 (the sound's own owner said gone).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerPoll {
    pub update: bool,
    pub gone: bool,
}

impl Sounds {
    /// The start pool.
    pub fn starts(&self) -> &Pool<Start> {
        &self.starts
    }

    /// Take a row (`0x21d15f`, `0x21d2f4`). `None` when all 16 are taken.
    pub fn start_new(&mut self, start: Start) -> Option<Handle> {
        self.starts.alloc(start)
    }

    /// `0x21d21f`: the admission answered `sound`; the row becomes kind 1
    /// with the request's word.
    pub fn start_attach(&mut self, handle: Handle, sound: Handle, word: u16) -> bool {
        let Some(s) = self.starts.get_mut(handle) else {
            return false;
        };
        s.of = StartOf::Sound(sound);
        s.word = word;
        true
    }

    /// `0x21dc91`: the request names no owner of its own (flag bit 5 clear).
    pub fn start_release(&mut self, handle: Handle) {
        if let Some(s) = self.starts.get_mut(handle) {
            s.released = true;
        }
    }

    /// `0x21dcf0`: the admission refused; the row is freed at once.
    pub fn start_free(&mut self, handle: Handle) -> bool {
        self.starts.free(handle).is_ok()
    }

    /// `0x21d360` and the end of `0x21da90`, before `Mode::Stop` runs:
    /// stopping and released.
    pub fn start_stop_begin(&mut self, handle: Handle) -> bool {
        let Some(s) = self.starts.get_mut(handle) else {
            return false;
        };
        s.stopping = true;
        s.released = true;
        true
    }

    /// After `Mode::Stop` ran: done, freed by the next walk.
    pub fn start_stop_end(&mut self, handle: Handle) {
        if let Some(s) = self.starts.get_mut(handle) {
            s.done = true;
        }
    }

    /// `0x21da90`: the start's sound finished with retail reason `code`
    /// (`0x21db30` passes 1 for a stop). Reason `0xA` (the stop-all) does
    /// nothing; any other sets `cut` unless it is 2 (played out) and
    /// begins the stop. True when the caller runs `Mode::Stop` and then
    /// [`Sounds::start_stop_end`].
    pub fn start_sound_ended(&mut self, handle: Handle, code: u8) -> bool {
        if code == 0xA {
            return false;
        }
        let Some(s) = self.starts.get_mut(handle) else {
            return false;
        };
        s.cut = code != 2;
        self.start_stop_begin(handle)
    }

    /// `0x21d390`, in slot order: a done row is freed; any other is polled
    /// (`0x21d5a0`) and named in the answer when `Mode::Update` runs for it.
    /// A tag start updates until it stops; a sound start updates while it
    /// is stopping or released, else on `poll` of its sound, which marks
    /// it released when the sound's owner is gone; a dialogue start never.
    pub fn starts_walk(
        &mut self,
        poll: &mut dyn FnMut(Handle, &Start) -> OwnerPoll,
    ) -> Vec<Handle> {
        let mut update = Vec::new();
        for handle in self.starts.handles() {
            let Some(s) = self.starts.get(handle).copied() else {
                continue;
            };
            if s.done {
                let _ = self.starts.free(handle);
                continue;
            }
            let runs = match s.of {
                StartOf::Tag { .. } => !s.stopping,
                StartOf::Sound(_) if s.stopping || s.released => true,
                StartOf::Sound(_) => {
                    let p = poll(handle, &s);
                    if p.gone {
                        self.start_release(handle);
                    }
                    p.update
                }
                StartOf::Dialogue(_) | StartOf::Admitting => false,
            };
            if runs {
                update.push(handle);
            }
        }
        update
    }
}

#[cfg(test)]
mod tests;
