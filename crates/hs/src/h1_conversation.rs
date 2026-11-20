//! Wire contract between converted campaign scripts and the runtime.

/// Reserved retail builtin slots used by converted conversations.
pub const BUILTINS: [(&str, u16); 1] = [("__h1_conversation", 845)];
pub const EVALUATE: u32 = 0x002a_b130;
pub const MAGIC: u32 = 0x4831_4300;

/// The four long arguments are the operation and three operands. A global
/// operand is the scenario-local index, or `NONE` for an absent object.
#[derive(Clone, Copy)]
#[repr(u32)]
pub enum Op {
    /// Sound global, optional emitter global, instance global.
    StartSound = MAGIC,
    /// Instance global. Clears it when the native instance ends.
    PollSound,
    /// Instance global. Stops only that instance and clears the global.
    StopSound,
    /// Unit global, AI global.
    Actor,
    /// First role object global, focus object global, cast count and look requirement.
    Focus,
    /// Candidate global, three packed voice words, cast range and selection mode.
    /// The three cells after the candidate hold its rank, voice and history base.
    Score,
    /// Unit global, voice global, profile table base.
    RememberVoice,
    /// Unit global and command script index. Removes just that command from the actor stack.
    CancelCommand,
}

impl Op {
    pub fn decode(value: u32) -> Option<Self> {
        [
            Self::StartSound,
            Self::PollSound,
            Self::StopSound,
            Self::Actor,
            Self::Focus,
            Self::Score,
            Self::RememberVoice,
            Self::CancelCommand,
        ]
        .into_iter()
        .find(|&op| op as u32 == value)
    }
}

pub const NONE: u32 = u32::MAX;
pub const REAL: u16 = 6;
pub const SHORT: u16 = 7;
pub const LONG: u16 = 8;
pub const AI: u16 = 19;
pub const SOUND: u16 = 32;
pub const OBJECT: u16 = 50;

pub const ACTOR_CAPACITY: u32 = 256;
pub const ROLE_STRIDE: u32 = 5;

/// The versioned spawn metadata at the start of converted script strings.
pub mod spawn {
    pub const SIGNATURE: u32 = u32::from_le_bytes(*b"H1V2");
    pub const HEADER: usize = 20;
    pub const BANK_BYTES: usize = 36;
    pub const SQUAD_BYTES: usize = 10;
    pub const NO_BANK: u16 = u16::MAX;
    pub const VOICE_RECORDS: u32 = 256;
}
