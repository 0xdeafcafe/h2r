//! The part creation switch `0x17a8a0`: what each tag group's arm does.

/// Tag group words as the switch compares them (`0x17a8bd` and on).
pub mod group {
    pub const LENS: u32 = 0x6C65_6E73; // lens
    pub const MGS2: u32 = 0x4D47_5332; // MGS2
    pub const DECAL: u32 = 0x6465_6361; // deca
    pub const LIGHT: u32 = 0x6C69_6768; // ligh
    pub const DAMAGE: u32 = 0x6A70_7421; // jpt!
    pub const OBJECT: u32 = 0x6F62_6A65; // obje
    pub const SOUND: u32 = 0x736E_6421; // snd!
    pub const CHARACTER: u32 = 0x6368_6172; // char
    pub const COLN: u32 = 0x636F_6C6E; // coln
    /// `tdtl`: returns at `0x17adb1` like `lens`.
    pub const TDTL: u32 = 0x7464_746C;
}

/// What a part's arm does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PartClass {
    /// `jpt!` (`0x17a98a`): builds a damage record and applies area damage
    /// (`0xd6c80`): health, shields and impulses.
    Damage,
    /// `obje` (`0x17ab44`): creates an object; draws word 0 of the shared
    /// stream through `0x177d10` and `0x177f60`.
    Object,
    /// `snd!` (`0x17acef`): starts a sound.
    Sound,
    /// `deca` (`0x17aa5c`): places a decal; draws word +4.
    Decal,
    /// `ligh` (`0x17ac70`): creates a light.
    Light,
    /// `char` (`0x17a8f1`): an AI character part.
    Character,
    /// `coln` (`0x17a94f`): one draw from word +4, nothing else.
    Coln,
    /// `lens`, `MGS2` and `tdtl`: the arm is the epilogue; nothing is made.
    Nothing,
    /// Any other group: retail sets the warning byte `0x55e756` and returns.
    Other,
}

impl PartClass {
    pub fn of(group: u32) -> PartClass {
        match group {
            group::DAMAGE => PartClass::Damage,
            group::OBJECT => PartClass::Object,
            group::SOUND => PartClass::Sound,
            group::DECAL => PartClass::Decal,
            group::LIGHT => PartClass::Light,
            group::CHARACTER => PartClass::Character,
            group::COLN => PartClass::Coln,
            group::LENS | group::MGS2 | group::TDTL => PartClass::Nothing,
            _ => PartClass::Other,
        }
    }

    /// The part changes state the sim reads or the arena hash covers (or
    /// draws from the shared stream), so it must stay exact.
    pub fn touches_sim(self) -> bool {
        !matches!(self, PartClass::Nothing)
    }

    /// The renderer shows what the part makes: it becomes a scene event.
    pub fn visual(self) -> bool {
        matches!(self, PartClass::Decal | PartClass::Light | PartClass::Nothing)
    }
}

/// One part as the switch is entered with it: the effect datum, the part
/// record's fields and the three location words (`[esp+0xc..+0x14]`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    pub tick: u32,
    /// The effect's definition tag (`[effect + 4]`).
    pub definition: u32,
    /// The part record's group (`+8`) and tag (`+0x10`).
    pub group: u32,
    pub tag: u32,
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub scale: f32,
}

impl Part {
    pub fn class(&self) -> PartClass {
        PartClass::of(self.group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The switch's arms from the disassembly of `0x17a8a0`.
    #[test]
    fn every_group_has_its_arm() {
        let table = [
            (group::LENS, PartClass::Nothing),
            (group::MGS2, PartClass::Nothing),
            (group::TDTL, PartClass::Nothing),
            (group::DECAL, PartClass::Decal),
            (group::LIGHT, PartClass::Light),
            (group::DAMAGE, PartClass::Damage),
            (group::OBJECT, PartClass::Object),
            (group::SOUND, PartClass::Sound),
            (group::CHARACTER, PartClass::Character),
            (group::COLN, PartClass::Coln),
            (0x6566_6665, PartClass::Other),
        ];
        for (g, class) in table {
            assert_eq!(PartClass::of(g), class, "{g:#x}");
        }
        assert_eq!(&group::DAMAGE.to_be_bytes(), b"jpt!");
        assert_eq!(&group::TDTL.to_be_bytes(), b"tdtl");
        assert!(PartClass::Damage.touches_sim() && !PartClass::Damage.visual());
        assert!(PartClass::Decal.touches_sim() && PartClass::Decal.visual());
        assert!(!PartClass::Nothing.touches_sim());
    }
}
