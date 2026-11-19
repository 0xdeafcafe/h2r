//! Halo 1 animation sounds as Halo 2 sound events.
//!
//! Halo 1 keeps a graph's sounds in its sound references block (`antr
//! +0x54`, [`H1_SOUND_REF_LEN`] each: a `snd!` reference) and gives each
//! animation at most one: `+0x3c` i16 the reference's index (-1 none),
//! `+0x3e` i16 the frame it plays on. The first-person reloads and
//! readies are these (Blood Gulch's assault rifle `reload-full`, `ready`).
//!
//! Halo 2's graph holds sound references at `jmad +0x14` ([`H2_SOUND_REF_LEN`]:
//! group, datum, u16 flags; E `0x001c7450` reads the datum at `+4`) and
//! per animation its sound events at `+0x54` ([`H2_SOUND_EVENT_LEN`]: i16
//! reference index, i16 frame, then a word passed to the event callback
//! unmodified, `tags::anim::SoundEvent`). The engine's event walk
//! (`0x001c7450`, the animation at `[esp+0x3c]`) reads the frame events at
//! `+0x4c`/`+0x50` and the sound events at `+0x54`/`+0x58` (`0x001c7545`,
//! `0x001c7564`) and plays those whose frame the update passed. The word
//! after the frame is left 0 (S: a marker name string ID, 0 the empty
//! name; `weapon_sounds_disc::retail_animation_sound_events` prints
//! retail's).
//!
//! A converted graph takes them after [`crate::jmad::convert_graph`]
//! ([`add_sound_events`]): the caller converts the sounds (a sound is not
//! a model, so `h1objects` does it) and passes their datums.

use h1map::{H1Map, Tag, TagRef};
use map::TagId;
use map::build::{Block, FixupKind};

use crate::{Error, Result};

/// Halo 1 `antr +0x54`: the sound references block.
pub const H1_SOUND_REFS: usize = 0x54;
/// A Halo 1 sound reference: the 16-byte reference and 4 bytes of padding.
/// V (`weapon_sounds_disc::halo1_animation_sounds`, every map): the
/// reference at `+0` names a `snd!` on every element.
pub const H1_SOUND_REF_LEN: usize = 0x14;
/// Halo 1 animation `+0x3c` (i16 sound reference, -1 none) and `+0x3e`
/// (i16 frame).
pub const H1_ANIMATION_SOUND: usize = 0x3c;

/// Halo 2 `jmad +0x14`, `0x0c` each.
pub const H2_SOUND_REFS: u32 = 0x14;
pub const H2_SOUND_REF_LEN: usize = 0x0c;
/// Halo 2 animation `+0x54`, 8 each.
pub const H2_SOUND_EVENTS: u32 = 0x54;
pub const H2_SOUND_EVENT_LEN: usize = 8;
/// Halo 2 `jmad +0x2c`: the animations (`0x6c` each).
const H2_ANIMATIONS: u32 = 0x2c;
const H2_ANIMATION_LEN: usize = 0x6c;

/// A Halo 1 graph's sounds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationSounds {
    /// The sound references, in order (`None`: a null reference).
    pub refs: Vec<Option<h1map::TagId>>,
    /// Per animation that plays one: (animation index, reference index,
    /// frame).
    pub events: Vec<(usize, usize, i16)>,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

/// The block at `owner + at` (`count`, address), `stride` bytes each.
fn elements<'a>(map: &'a H1Map, owner: &[u8], at: usize, stride: usize) -> Result<Vec<&'a [u8]>> {
    let count = u32_at(owner, at) as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    if count > 0x10000 {
        return Err(Error::Invalid(format!("block of {count} elements")));
    }
    let b = map.read(u32_at(owner, at + 4), count * stride)?;
    Ok(b.chunks_exact(stride).collect())
}

/// Reads the sounds of the Halo 1 graph `tag` (`antr`). An animation's
/// index past the references, or a frame outside the animation, is left
/// out.
pub fn read(map: &H1Map, tag: &Tag) -> Result<AnimationSounds> {
    if tag.group != h1map::Group::new(b"antr") {
        return Err(Error::WrongGroup(tag.group.to_string(), "antr"));
    }
    let h = map.tag_data(tag)?;
    if h.len() < crate::h1::ANTR_HEADER {
        return Err(Error::Truncated("antr header"));
    }
    let refs: Vec<Option<h1map::TagId>> = elements(map, h, H1_SOUND_REFS, H1_SOUND_REF_LEN)?
        .into_iter()
        .map(|e| {
            TagRef::read(e, 0)
                .map(|r| r.id)
                .filter(|id| *id != h1map::TagId::NONE)
        })
        .collect();
    let mut events = Vec::new();
    for (i, a) in elements(map, h, 0x74, 0xb4)?.into_iter().enumerate() {
        let (sound, frame) = (
            i16_at(a, H1_ANIMATION_SOUND),
            i16_at(a, H1_ANIMATION_SOUND + 2),
        );
        let frames = i16::from_le_bytes([a[0x22], a[0x23]]);
        if sound >= 0 && (sound as usize) < refs.len() && (0..frames.max(1)).contains(&frame) {
            events.push((i, sound as usize, frame));
        }
    }
    Ok(AnimationSounds { refs, events })
}

/// Writes `sounds` into the converted graph `root`
/// ([`crate::jmad::convert_graph`]'s): the references `targets` gives a
/// datum for (one per Halo 1 reference; `None` did not convert), in
/// order, and each animation's event for one of them. Events of a sound
/// that did not convert are left out.
pub fn add_sound_events(
    root: &mut Block,
    sounds: &AnimationSounds,
    targets: &[Option<TagId>],
) -> Result<()> {
    // Halo 1 reference index -> Halo 2 reference index.
    let mut index = vec![None; sounds.refs.len()];
    let mut refs = Vec::new();
    for (k, t) in targets.iter().enumerate().take(sounds.refs.len()) {
        if let Some(id) = t {
            index[k] = Some(refs.len() as i16);
            let mut b = Block::zeroed(H2_SOUND_REF_LEN);
            b.tag_ref(0, *id);
            refs.push(b);
        }
    }
    let wanted: Vec<(usize, i16, i16)> = sounds
        .events
        .iter()
        .filter_map(|(a, s, f)| Some((*a, index.get(*s).copied().flatten()?, *f)))
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let anims = root
        .fixups
        .iter_mut()
        .filter(|f| f.offset == H2_ANIMATIONS)
        .find_map(|f| match &mut f.kind {
            FixupKind::Block { child, .. } => Some(child),
            _ => None,
        })
        .ok_or(Error::Invalid("converted graph has no animations".into()))?;
    for (a, s, frame) in wanted {
        let at = a * H2_ANIMATION_LEN;
        if at + H2_ANIMATION_LEN > anims.bytes.len() {
            return Err(Error::Invalid(format!(
                "sound event for animation {a}, past the graph's"
            )));
        }
        let mut e = Block::zeroed(H2_SOUND_EVENT_LEN);
        e.bytes[0..2].copy_from_slice(&s.to_le_bytes());
        e.bytes[2..4].copy_from_slice(&frame.to_le_bytes());
        anims.block(at as u32 + H2_SOUND_EVENTS, 1, e);
    }
    let n = refs.len() as u32;
    let mut all = Block::zeroed(refs.len() * H2_SOUND_REF_LEN);
    for (i, r) in refs.into_iter().enumerate() {
        for mut f in r.fixups {
            f.offset += (i * H2_SOUND_REF_LEN) as u32;
            all.fixups.push(f);
        }
    }
    root.block(H2_SOUND_REFS, n, all);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each converted sound becomes a reference, each animation's sound an
    /// event naming it at Halo 1's frame; a sound that did not convert
    /// takes its events with it, and the references close up.
    #[test]
    fn halo1_animation_sounds_become_sound_events() {
        let mut root = Block::zeroed(0xbc);
        root.block(H2_ANIMATIONS, 3, Block::zeroed(3 * H2_ANIMATION_LEN));
        let sounds = AnimationSounds {
            refs: vec![None, Some(h1map::TagId(7)), Some(h1map::TagId(9))],
            events: vec![(2, 1, 5), (0, 0, 1), (1, 2, 3)],
        };
        let reload = TagId(0xe001_0001);
        // The first reference converts to nothing.
        add_sound_events(&mut root, &sounds, &[None, Some(reload), None]).unwrap();
        let refs = root
            .fixups
            .iter()
            .find_map(|f| match &f.kind {
                FixupKind::Block { count, child } if f.offset == H2_SOUND_REFS => {
                    Some((*count, child))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(refs.0, 1);
        assert_eq!(refs.1.fixups[0].kind, FixupKind::TagRef(reload));
        let anims = root
            .fixups
            .iter()
            .find_map(|f| match &f.kind {
                FixupKind::Block { child, .. } if f.offset == H2_ANIMATIONS => Some(child),
                _ => None,
            })
            .unwrap();
        let events: Vec<(u32, u32, Vec<u8>)> = anims
            .fixups
            .iter()
            .filter_map(|f| match &f.kind {
                FixupKind::Block { count, child } => Some((f.offset, *count, child.bytes.clone())),
                _ => None,
            })
            .collect();
        // Animation 2 plays reference 0 (Halo 1's second) at frame 5.
        let at = 2 * H2_ANIMATION_LEN as u32 + H2_SOUND_EVENTS;
        assert_eq!(events, [(at, 1, vec![0, 0, 5, 0, 0, 0, 0, 0])]);
    }

    /// A graph none of whose sounds converted is left as it was.
    #[test]
    fn no_converted_sound_writes_nothing() {
        let mut root = Block::zeroed(0xbc);
        root.block(H2_ANIMATIONS, 1, Block::zeroed(H2_ANIMATION_LEN));
        let before = root.clone();
        let sounds = AnimationSounds {
            refs: vec![Some(h1map::TagId(7))],
            events: vec![(0, 0, 1)],
        };
        add_sound_events(&mut root, &sounds, &[None]).unwrap();
        assert_eq!(root, before);
    }
}
