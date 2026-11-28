//! Guest-side rows for `audio.loops` (docs/runtime/agent-api.md).

use std::collections::HashSet;

use guest_layout::datum::DatumArray;
use guest_layout::sound::{
    LoopingSound, LoopingSoundTag, LoopingTrackTag, SoundChannel, SoundInstance, SoundStream,
};
use guest_layout::tags::{self, GroupTag, entry_layout};
use guest_layout::{DatumHandle, GuestMemory, MemFault, points};
use serde::Serialize;

const LOOP_SLOTS: i32 = 128;
const SOUND_SLOTS: i32 = 384;
const CHANNELS: u16 = 72;
const LSND: GroupTag = GroupTag::from_chars(*b"lsnd");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Segment {
    In,
    Loop,
    Out,
    AltIn,
    Alt,
    AltTransitionOut,
    AltOut,
    Unknown,
}

impl Segment {
    // The track updater resolves these states through the track's lsnd
    // references (0x21a75e..0x21a779).
    fn from_state(state: u8) -> Self {
        match state {
            0 => Self::In,
            1 => Self::Loop,
            2 => Self::Out,
            3 => Self::AltIn,
            4 => Self::Alt,
            5 => Self::AltTransitionOut,
            6 => Self::AltOut,
            _ => Self::Unknown,
        }
    }

    fn for_sound(
        m: &dyn GuestMemory,
        track: LoopingTrackTag,
        sound: u32,
        selected: Self,
    ) -> Result<Self, MemFault> {
        // Old and new segments can coexist. Label by the instance's tag
        // and use the selected state only if two segments share that tag.
        let refs = [
            (Self::In, track.in_sound(m)?.raw()),
            (Self::Loop, track.loop_sound(m)?.raw()),
            (Self::Out, track.out_sound(m)?.raw()),
            (Self::AltIn, track.alt_in_sound(m)?.raw()),
            (Self::Alt, track.alt_loop_sound(m)?.raw()),
            (
                Self::AltTransitionOut,
                track.alt_transition_out_sound(m)?.raw(),
            ),
            (Self::AltOut, track.alt_out_sound(m)?.raw()),
        ];
        let mut matches = refs
            .into_iter()
            .filter_map(|(segment, tag)| (tag == sound).then_some(segment));
        let Some(first) = matches.next() else {
            return Ok(Self::Unknown);
        };
        let Some(second) = matches.next() else {
            return Ok(first);
        };
        Ok(
            if first == selected || second == selected || matches.any(|s| s == selected) {
                selected
            } else {
                Self::Unknown
            },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Track {
    pub(crate) loop_datum: u32,
    pub(crate) tag_datum: u32,
    pub(crate) track: u8,
    pub(crate) segment: Segment,
    pub(crate) sound_instance: u32,
    pub(crate) sound_datum: u32,
    pub(crate) pitch_range: Option<u8>,
    pub(crate) permutation: Option<u8>,
    pub(crate) chunk: Option<u16>,
    #[serde(skip)]
    pub(crate) ds_stream: Option<u32>,
    pub(crate) play_packet: Option<u64>,
    pub(crate) play_position_frames: Option<u64>,
}

fn pool(
    m: &dyn GuestMemory,
    global: &points::Global,
    stride: u32,
    limit: i32,
) -> Result<Option<DatumArray>, MemFault> {
    let Some(addr) = global.pointer(m)? else {
        return Ok(None);
    };
    let array = DatumArray::at(addr);
    let header = array.header(m)?;
    Ok((header.valid
        && header.signature == guest_layout::datum::SIGNATURE
        && header.data != 0
        && header.bitmap != 0
        && header.element_size == stride as i32
        && (0..=limit).contains(&header.maximum_count)
        && (0..=header.maximum_count).contains(&header.high_water))
    .then_some(array))
}

pub(crate) fn read(m: &dyn GuestMemory) -> Result<Vec<Track>, MemFault> {
    let mut rows = Vec::new();
    let Some(loops) = pool(m, &points::LOOPING_SOUNDS, LoopingSound::SIZE, LOOP_SLOTS)? else {
        return Ok(rows);
    };
    let Some(instances) = pool(
        m,
        &points::SOUND_INSTANCES,
        SoundInstance::SIZE,
        SOUND_SLOTS,
    )?
    else {
        return Ok(rows);
    };
    if points::TAG_INDEX.pointer(m)?.is_none() {
        return Ok(rows);
    }

    let mut visited = HashSet::new();
    let mut cursor = None;
    while let Some(loop_datum) = loops.next_datum(m, cursor)? {
        cursor = Some(loop_datum);
        let Some(addr) = loops.try_get(m, loop_datum)? else {
            continue;
        };
        let looping = LoopingSound::at(addr);
        let Some(tag_datum) = DatumHandle::from_raw(looping.tag(m)?.raw()) else {
            continue;
        };
        let Some(def) = definition(m, tag_datum)? else {
            continue;
        };
        let count = def
            .track_count(m)?
            .clamp(0, i32::from(LoopingSound::TRACK_COUNT)) as u8;
        let tracks = def.tracks(m)?;
        if tracks == 0 {
            continue;
        }

        for (index, state) in (0..count).filter_map(|i| looping.track(i).map(|state| (i, state))) {
            let tag =
                LoopingTrackTag::at(tracks.wrapping_add(u32::from(index) * LoopingTrackTag::SIZE));
            let selected = Segment::from_state(state.segment(m)?);
            let mut next = DatumHandle::from_raw(state.head(m)?.raw());
            while let Some(sound_instance) = next {
                let Some(addr) = instances.try_get(m, sound_instance)? else {
                    break;
                };
                let sound = SoundInstance::at(addr);
                if sound.loop_owner(m)?.raw() != loop_datum.raw()
                    || sound.track(m)? != index
                    || !visited.insert(sound_instance)
                {
                    break;
                }
                next = DatumHandle::from_raw(sound.next_on_track(m)?.raw());
                let Some(sound_datum) = sound.tag(m)?.get() else {
                    continue;
                };
                rows.push(Track {
                    loop_datum: loop_datum.raw(),
                    tag_datum: tag_datum.raw(),
                    track: index,
                    segment: Segment::for_sound(m, tag, sound_datum, selected)?,
                    sound_instance: sound_instance.raw(),
                    sound_datum,
                    pitch_range: sound.pitch_range(m)?.try_into().ok(),
                    permutation: sound.permutation(m)?.try_into().ok(),
                    chunk: sound.chunk(m)?.try_into().ok(),
                    ds_stream: stream_object(m, sound, sound_instance).ok().flatten(),
                    play_packet: None,
                    play_position_frames: None,
                });
            }
        }
    }
    Ok(rows)
}

fn definition(m: &dyn GuestMemory, tag: DatumHandle) -> Result<Option<LoopingSoundTag>, MemFault> {
    let Some(entry) = tags::tag_entry_checked(m, tag)? else {
        return Ok(None);
    };
    if m.u32(entry.wrapping_add(entry_layout::GROUP))? != LSND.0 {
        return Ok(None);
    }
    let addr = m.u32(entry.wrapping_add(entry_layout::ADDRESS))?;
    Ok((addr != 0).then(|| LoopingSoundTag::at(addr)))
}

fn stream_object(
    m: &dyn GuestMemory,
    sound: SoundInstance,
    instance: DatumHandle,
) -> Result<Option<u32>, MemFault> {
    let Ok(channel) = u16::try_from(sound.channel(m)?) else {
        return Ok(None);
    };
    if channel >= CHANNELS {
        return Ok(None);
    }
    let Some(channels) = points::SOUND_CHANNELS.pointer(m)? else {
        return Ok(None);
    };
    let row = SoundChannel::at(channels.wrapping_add(u32::from(channel) * SoundChannel::SIZE));
    if row.owner(m)?.raw() != instance.raw() {
        return Ok(None);
    }
    let Ok(index) = u16::try_from(row.stream(m)?) else {
        return Ok(None);
    };
    let Some(driver) = points::SOUND_DRIVER.pointer(m)? else {
        return Ok(None);
    };
    let stream = SoundStream::at(
        driver.wrapping_add(SoundStream::ARRAY_OFFSET + u32::from(index) * SoundStream::SIZE),
    );
    let object = stream.object(m)?;
    Ok((object != 0).then_some(object))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::datum::layout;
    use guest_layout::tags::index_layout;
    use guest_layout::testing::FakeGuest;

    const LOOP_POOL: u32 = 0x20000;
    const SOUND_POOL: u32 = 0x21000;
    const LOOP_ROW: u32 = 0x22000;
    const SOUND_ROWS: u32 = 0x23000;
    const TAG_INDEX: u32 = 0x24000;
    const TAG_ENTRY: u32 = 0x25000;
    const DEFINITION: u32 = 0x26000;
    const TRACK_TAG: u32 = 0x27000;
    const CHANNEL_ROWS: u32 = 0x28000;
    const DRIVER: u32 = 0x29000;

    const LOOP_ID: DatumHandle = DatumHandle::new(0xa100, 0);
    const LSND_ID: DatumHandle = DatumHandle::new(0xa200, 0);
    const INTRO: DatumHandle = DatumHandle::new(0xa300, 0);
    const LOOP: DatumHandle = DatumHandle::new(0xa301, 1);
    const INTRO_TAG: DatumHandle = DatumHandle::new(0xa400, 1);
    const LOOP_TAG: DatumHandle = DatumHandle::new(0xa401, 2);

    fn array(
        g: &mut FakeGuest,
        global: &points::Global,
        header: u32,
        data: u32,
        count: u32,
        size: u32,
    ) {
        let bitmap = header + layout::HEADER_SIZE;
        g.put_u32(global.base, header)
            .put_u32(header + layout::MAXIMUM_COUNT, count)
            .put_u32(header + layout::HIGH_WATER, count)
            .put_u32(header + layout::ELEMENT_SIZE, size)
            .put_u32(header + layout::SIGNATURE, guest_layout::datum::SIGNATURE)
            .put_u8(header + layout::VALID, 1)
            .put_u32(header + layout::DATA, data)
            .put_u32(header + layout::BITMAP, bitmap)
            .put_u32(bitmap, (1 << count) - 1)
            .map(data, count * size);
    }

    fn sound(g: &mut FakeGuest, handle: DatumHandle, tag: DatumHandle, next: DatumHandle) {
        let addr = SOUND_ROWS + u32::from(handle.index()) * SoundInstance::SIZE;
        g.put_u16(addr, handle.salt())
            .put_u32(addr + SoundInstance::TAG.offset, tag.raw())
            .put_u32(addr + SoundInstance::LOOP_OWNER.offset, LOOP_ID.raw())
            .put_u8(addr + SoundInstance::TRACK.offset, 0)
            .put_u32(addr + SoundInstance::NEXT_ON_TRACK.offset, next.raw())
            .put_u8(addr + SoundInstance::PITCH_RANGE.offset, 2)
            .put_u8(addr + SoundInstance::PERMUTATION.offset, 3)
            .put_u16(addr + SoundInstance::CHUNK.offset, 7)
            .put_u16(addr + SoundInstance::CHANNEL.offset, u16::MAX);
    }

    fn fixture() -> FakeGuest {
        let mut g = FakeGuest::new();
        array(
            &mut g,
            &points::LOOPING_SOUNDS,
            LOOP_POOL,
            LOOP_ROW,
            1,
            LoopingSound::SIZE,
        );
        array(
            &mut g,
            &points::SOUND_INSTANCES,
            SOUND_POOL,
            SOUND_ROWS,
            2,
            SoundInstance::SIZE,
        );
        let track = LoopingSound::at(LOOP_ROW).track(0).unwrap();
        g.put_u16(LOOP_ROW, LOOP_ID.salt())
            .put_u32(LOOP_ROW + LoopingSound::TAG.offset, LSND_ID.raw())
            .put_u32(track.addr, INTRO.raw())
            .put_u8(track.addr + guest_layout::sound::LoopingTrack::SEGMENT.offset, 1)
            .put_u32(points::TAG_INDEX.base, TAG_INDEX)
            .put_u32(TAG_INDEX + index_layout::TAG_COUNT, 1)
            .put_u32(points::TAG_ENTRIES.base, TAG_ENTRY)
            .put_u32(TAG_ENTRY + entry_layout::GROUP, LSND.0)
            .put_u32(TAG_ENTRY + entry_layout::DATUM, LSND_ID.raw())
            .put_u32(TAG_ENTRY + entry_layout::ADDRESS, DEFINITION)
            .put_u32(DEFINITION + LoopingSoundTag::TRACK_COUNT.offset, 1)
            .put_u32(DEFINITION + LoopingSoundTag::TRACKS.offset, TRACK_TAG)
            .put_u32(TRACK_TAG + LoopingTrackTag::IN.offset, INTRO_TAG.raw())
            .put_u32(TRACK_TAG + LoopingTrackTag::LOOP.offset, LOOP_TAG.raw());
        sound(&mut g, INTRO, INTRO_TAG, LOOP);
        sound(&mut g, LOOP, LOOP_TAG, DatumHandle::NONE);
        g
    }

    #[test]
    fn overlapping_segments_keep_their_own_tags_and_permutations() {
        let mut g = fixture();
        let channel = CHANNEL_ROWS + SoundChannel::SIZE;
        let stream = DRIVER + SoundStream::ARRAY_OFFSET + 2 * SoundStream::SIZE;
        g.put_u16(SOUND_ROWS + SoundInstance::CHANNEL.offset, 1)
            .put_u32(points::SOUND_CHANNELS.base, CHANNEL_ROWS)
            .put_u32(channel + SoundChannel::OWNER.offset, INTRO.raw())
            .put_u16(channel + SoundChannel::STREAM.offset, 2)
            .put_u32(points::SOUND_DRIVER.base, DRIVER)
            .put_u32(stream + SoundStream::OBJECT.offset, 0x30000);

        let rows = read(&g).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            Track {
                loop_datum: LOOP_ID.raw(),
                tag_datum: LSND_ID.raw(),
                track: 0,
                segment: Segment::In,
                sound_instance: INTRO.raw(),
                sound_datum: INTRO_TAG.raw(),
                pitch_range: Some(2),
                permutation: Some(3),
                chunk: Some(7),
                ds_stream: Some(0x30000),
                play_packet: None,
                play_position_frames: None,
            },
        );
        assert_eq!(rows[1].segment, Segment::Loop);
        assert_eq!(rows[1].sound_instance, LOOP.raw());
        assert!(rows[1].ds_stream.is_none());
        assert!(
            serde_json::to_value(&rows[0])
                .unwrap()
                .get("ds_stream")
                .is_none()
        );
    }

    #[test]
    fn shared_tag_uses_the_selected_segment_and_stale_stream_owner_is_ignored() {
        let mut g = fixture();
        let track = LoopingSound::at(LOOP_ROW).track(0).unwrap();
        g.put_u32(TRACK_TAG + LoopingTrackTag::ALT_LOOP.offset, LOOP_TAG.raw())
            .put_u8(track.addr + guest_layout::sound::LoopingTrack::SEGMENT.offset, 4)
            .put_u16(SOUND_ROWS + SoundInstance::CHANNEL.offset, 0)
            .put_u32(points::SOUND_CHANNELS.base, CHANNEL_ROWS)
            .put_u32(CHANNEL_ROWS + SoundChannel::OWNER.offset, LOOP.raw());
        let rows = read(&g).unwrap();
        assert_eq!(rows[0].segment, Segment::In);
        assert!(rows[0].ds_stream.is_none());
        assert_eq!(rows[1].segment, Segment::Alt);
    }

    #[test]
    fn a_reused_sound_handle_or_a_cyclic_link_stops_the_walk() {
        let mut g = fixture();
        let next = SOUND_ROWS + SoundInstance::NEXT_ON_TRACK.offset;
        g.put_u32(next, DatumHandle::new(0xa3ff, 1).raw());
        assert_eq!(read(&g).unwrap().len(), 1);
        g.put_u32(next, INTRO.raw());
        assert_eq!(read(&g).unwrap().len(), 1);
    }

    #[test]
    fn a_null_loop_array_returns_no_rows() {
        let mut g = FakeGuest::new();
        g.put_u32(points::LOOPING_SOUNDS.base, 0);
        assert!(read(&g).unwrap().is_empty());
    }
}
