//! Machine snapshots (docs/runtime/snapshots.md, phase 4): the
//! [`DirectSound`] half of the `dsound` section ([`crate::register::Hle`]
//! adds the arena, event pins and queued callbacks around it).
//!
//! Every struct is destructured without `..`, so a field added later does
//! not compile until someone decides it:
//!
//! * **Save**: every guest object under its guest pointer (buffers with
//!   their data pointers, formats, volumes, 3D and effect settings, mix
//!   bins, pause gates and mixer ids; streams with their packet records,
//!   gates and mixer ids), the root, mix-bin headroom, the effects image
//!   and the descriptors `XAudioSetEffectData` accepted, the listener, the
//!   packet token counter, the reverb's effect index and the XHV voice
//!   rings (their write offsets and rate converters).
//! * **Check**: the [`Layout`] (stream vtable, effect count), fixed by the
//!   build.
//! * **Keep**: the loading process's wiring: the mixer handle (its
//!   contents are the `audio` section), who consumes the mixer and the
//!   lease counter, `frames_per_do_work`, the controller-speaker router.
//! * **Ignore**: the counts of ignored calls.
use super::*;
use audio::snap::{
    get_f32, get_format, get_opt, get_output, get_voice_fx, put_f32, put_format, put_opt,
    put_output, put_voice_fx,
};
use snapshot::codec::{Reader, Writer};

use crate::voice_rings::{Converter, Ring};

type Error = snapshot::Error;

fn corrupt(what: impl Into<String>) -> Error {
    Error::Corrupt("dsound", what.into())
}

fn put_i32(w: &mut Writer, v: i32) {
    w.u32(v as u32);
}

fn get_i32(r: &mut Reader<'_>) -> Result<i32, Error> {
    Ok(r.u32()? as i32)
}

fn put_u32_opt(w: &mut Writer, v: &Option<u32>) {
    put_opt(w, v, |w, v| {
        w.u32(*v);
    });
}

fn get_u32_opt(r: &mut Reader<'_>) -> Result<Option<u32>, Error> {
    get_opt(r, |r| r.u32())
}

fn put_vec3(w: &mut Writer, v: &[f32; 3]) {
    for f in v {
        put_f32(w, *f);
    }
}

fn get_vec3(r: &mut Reader<'_>) -> Result<[f32; 3], Error> {
    Ok([get_f32(r)?, get_f32(r)?, get_f32(r)?])
}

fn put_gate(w: &mut Writer, g: Gate) {
    w.u8(match g {
        Gate::Open => 0,
        Gate::Paused => 1,
        Gate::Held => 2,
    });
}

fn get_gate(r: &mut Reader<'_>) -> Result<Gate, Error> {
    match r.u8()? {
        0 => Ok(Gate::Open),
        1 => Ok(Gate::Paused),
        2 => Ok(Gate::Held),
        g => Err(corrupt(format!("gate {g}"))),
    }
}

fn put_bins(w: &mut Writer, b: &VoiceBins) {
    let VoiceBins { list, volumes } = b;
    w.list(list, |w, bin| {
        w.u32(*bin);
    });
    w.u32(volumes.len() as u32);
    for (bin, mb) in volumes {
        w.u32(*bin);
        put_i32(w, *mb);
    }
}

fn get_bins(r: &mut Reader<'_>) -> Result<VoiceBins, Error> {
    let list = r.list(4, |r| r.u32())?;
    if list.len() > crate::mixbins::MAX_BINS {
        return Err(corrupt("more than eight mix bins"));
    }
    let volumes = r.list(8, |r| Ok((r.u32()?, get_i32(r)?)))?;
    Ok(VoiceBins {
        list,
        volumes: volumes.into_iter().collect(),
    })
}

fn put_buffer(w: &mut Writer, b: &BufferObj) {
    let BufferObj {
        refs,
        flags,
        mix_in,
        mix_in_playing,
        format,
        data,
        audio,
        volume_mb,
        frequency,
        fx,
        mix_bins,
        input_mix_bin,
        gate,
        resume,
        looping,
        position,
        min_distance,
        max_distance,
        rolloff,
        doppler,
        cone_outside_mb,
        bus,
        output,
    } = b;
    w.u32(*refs).u32(*flags).bool(*mix_in).bool(*mix_in_playing);
    put_opt(w, format, put_format);
    put_opt(w, data, |w, d| {
        let GuestData { ptr, size, alloc } = d;
        w.u32(*ptr).u32(*size);
        put_opt(w, alloc, |w, &(base, len)| {
            w.u32(base).u32(len);
        });
    });
    put_u32_opt(w, &audio.map(|id| id.raw()));
    put_i32(w, *volume_mb);
    put_u32_opt(w, frequency);
    put_voice_fx(w, fx);
    put_bins(w, mix_bins);
    w.u32(*input_mix_bin);
    put_gate(w, *gate);
    put_opt(w, resume, |w, r| {
        let Resume {
            looping,
            from_start,
        } = r;
        w.bool(*looping).bool(*from_start);
    });
    w.bool(*looping);
    put_opt(w, position, put_vec3);
    for v in [min_distance, max_distance, rolloff, doppler] {
        put_f32(w, *v);
    }
    put_i32(w, *cone_outside_mb);
    put_u32_opt(w, &bus.map(|id| id.raw()));
    put_output(w, *output);
}

fn get_buffer(r: &mut Reader<'_>) -> Result<BufferObj, Error> {
    Ok(BufferObj {
        refs: r.u32()?,
        flags: r.u32()?,
        mix_in: r.bool()?,
        mix_in_playing: r.bool()?,
        format: get_opt(r, get_format)?,
        data: get_opt(r, |r| {
            Ok(GuestData {
                ptr: r.u32()?,
                size: r.u32()?,
                alloc: get_opt(r, |r| Ok((r.u32()?, r.u32()?)))?,
            })
        })?,
        audio: get_u32_opt(r)?.map(BufferId::from_raw),
        volume_mb: get_i32(r)?,
        frequency: get_u32_opt(r)?,
        fx: get_voice_fx(r)?,
        mix_bins: get_bins(r)?,
        input_mix_bin: r.u32()?,
        gate: get_gate(r)?,
        resume: get_opt(r, |r| {
            Ok(Resume {
                looping: r.bool()?,
                from_start: r.bool()?,
            })
        })?,
        looping: r.bool()?,
        position: get_opt(r, get_vec3)?,
        min_distance: get_f32(r)?,
        max_distance: get_f32(r)?,
        rolloff: get_f32(r)?,
        doppler: get_f32(r)?,
        cone_outside_mb: get_i32(r)?,
        bus: get_u32_opt(r)?.map(MixInId::from_raw),
        output: get_output(r)?,
    })
}

fn put_slot(w: &mut Writer, s: &Slot) {
    let Slot { audio, rec, done } = s;
    put_opt(w, audio, |w, id| {
        w.u64(id.0);
    });
    let PacketRecord {
        token,
        size,
        completed_size_ptr,
        status_ptr,
        event_or_context,
    } = rec;
    w.u64(*token);
    for v in [size, completed_size_ptr, status_ptr, event_or_context] {
        w.u32(*v);
    }
    put_u32_opt(w, done);
}

fn get_slot(r: &mut Reader<'_>) -> Result<Slot, Error> {
    Ok(Slot {
        audio: get_opt(r, |r| Ok(PacketId(r.u64()?)))?,
        rec: PacketRecord {
            token: r.u64()?,
            size: r.u32()?,
            completed_size_ptr: r.u32()?,
            status_ptr: r.u32()?,
            event_or_context: r.u32()?,
        },
        done: get_u32_opt(r)?,
    })
}

fn put_stream(w: &mut Writer, s: &StreamObj) {
    let StreamObj {
        refs,
        audio,
        max_packets,
        callback,
        callback_context,
        packets,
        gate,
        started,
        volume_mb,
        mix_bins,
        output_buffer,
        discontinuities,
        voice_active,
        flags,
        info,
    } = s;
    w.u32(*refs).u32(audio.raw()).u32(*max_packets);
    w.u32(*callback).u32(*callback_context);
    w.u32(packets.len() as u32);
    for p in packets {
        put_slot(w, p);
    }
    put_gate(w, *gate);
    w.bool(*started);
    put_i32(w, *volume_mb);
    put_bins(w, mix_bins);
    put_u32_opt(w, output_buffer);
    w.u32(*discontinuities).bool(*voice_active).u32(*flags);
    let StreamInfoFormat {
        channels,
        bits,
        block_align,
    } = info;
    w.u8(*channels).u8(*bits).u8(*block_align);
}

fn get_stream(r: &mut Reader<'_>) -> Result<StreamObj, Error> {
    Ok(StreamObj {
        refs: r.u32()?,
        audio: StreamId::from_raw(r.u32()?),
        max_packets: r.u32()?,
        callback: r.u32()?,
        callback_context: r.u32()?,
        packets: r.list(26, get_slot)?.into(),
        gate: get_gate(r)?,
        started: r.bool()?,
        volume_mb: get_i32(r)?,
        mix_bins: get_bins(r)?,
        output_buffer: get_u32_opt(r)?,
        discontinuities: r.u32()?,
        voice_active: r.bool()?,
        flags: r.u32()?,
        info: StreamInfoFormat {
            channels: r.u8()?,
            bits: r.u8()?,
            block_align: r.u8()?,
        },
    })
}

fn put_object(w: &mut Writer, o: &Object) {
    match o {
        Object::Root(Root { refs }) => {
            w.u8(0).u32(*refs);
        }
        Object::Buffer(b) => {
            w.u8(1);
            put_buffer(w, b);
        }
        Object::Stream(s) => {
            w.u8(2);
            put_stream(w, s);
        }
    }
}

fn get_object(r: &mut Reader<'_>) -> Result<Object, Error> {
    match r.u8()? {
        0 => Ok(Object::Root(Root { refs: r.u32()? })),
        1 => Ok(Object::Buffer(get_buffer(r)?)),
        2 => Ok(Object::Stream(get_stream(r)?)),
        k => Err(corrupt(format!("object kind {k}"))),
    }
}

fn put_rings(w: &mut Writer, v: &VoiceRings) {
    let VoiceRings {
        rings,
        // Scratch.
        bytes: _,
    } = v;
    w.u32(rings.len() as u32);
    for ring in rings {
        let Ring {
            bin,
            base,
            size,
            state,
            write,
            conv,
        } = ring;
        for v in [bin, base, size, state, write] {
            w.u32(*v);
        }
        put_opt(w, conv, |w, (rate, c)| {
            let Converter { from, to, pos, last } = c;
            w.u32(*rate).u64(*from).u64(*to).u64(*pos);
            put_f32(w, *last);
        });
    }
}

fn get_rings(r: &mut Reader<'_>) -> Result<VoiceRings, Error> {
    let rings = r.list(21, |r| {
        let ring = Ring {
            bin: r.u32()?,
            base: r.u32()?,
            size: r.u32()?,
            state: r.u32()?,
            write: r.u32()?,
            conv: get_opt(r, |r| {
                let rate = r.u32()?;
                let c = Converter {
                    from: r.u64()?,
                    to: r.u64()?,
                    pos: r.u64()?,
                    last: get_f32(r)?,
                };
                Ok((rate, c))
            })?,
        };
        if ring.size == 0 || ring.write >= ring.size {
            return Err(corrupt("voice ring write offset outside the ring"));
        }
        if ring.conv.as_ref().is_some_and(|(_, c)| c.to == 0) {
            return Err(corrupt("voice ring converter to 0 Hz"));
        }
        Ok(ring)
    })?;
    Ok(VoiceRings {
        rings,
        bytes: Vec::new(),
    })
}

fn put_listener(w: &mut Writer, l: &Listener3d) {
    let Listener3d {
        position,
        front,
        top,
        distance_factor,
        doppler_factor,
        rolloff_factor,
    } = l;
    for v in [position, front, top] {
        put_vec3(w, v);
    }
    for v in [distance_factor, doppler_factor, rolloff_factor] {
        put_f32(w, *v);
    }
}

fn get_listener(r: &mut Reader<'_>) -> Result<Listener3d, Error> {
    Ok(Listener3d {
        position: get_vec3(r)?,
        front: get_vec3(r)?,
        top: get_vec3(r)?,
        distance_factor: get_f32(r)?,
        doppler_factor: get_f32(r)?,
        rolloff_factor: get_f32(r)?,
    })
}

/// The [`DirectSound`] part of the section, decoded and not yet applied.
pub(crate) struct SavedDs {
    layout: (u32, u32),
    objects: Vec<(u32, Object)>,
    root: Option<u32>,
    mix_bin_headroom: [u32; 32],
    effect_desc: BTreeMap<u32, EffectDesc>,
    effects: Option<EffectsImage>,
    listener: Listener3d,
    light_hrtf_4_channel: bool,
    frames_rendered: u64,
    next_token: u64,
    reverb_index: Option<u32>,
    voice_rings: Option<VoiceRings>,
}

impl DirectSound {
    /// Writes the saved fields, every one decided.
    pub(crate) fn put_snapshot(&self, w: &mut Writer) {
        let DirectSound {
            // Keep: the handle; the mixer's contents are the `audio` section.
            mixer: _,
            layout,
            objects,
            root,
            mix_bin_headroom,
            effect_desc,
            effects,
            listener,
            light_hrtf_4_channel,
            // Keep: host wiring.
            frames_per_do_work: _,
            frames_rendered,
            consumer: _,
            next_lease: _,
            next_token,
            // Diagnostics.
            ignored: _,
            reverb_index,
            voice_rings,
            // Keep: a host setting.
            pad_router: _,
        } = self;
        let Layout {
            stream_vtable,
            effect_count,
        } = layout;
        w.u32(*stream_vtable).u32(*effect_count);
        let mut ptrs: Vec<u32> = objects.keys().copied().collect();
        ptrs.sort_unstable();
        w.u32(ptrs.len() as u32);
        for p in ptrs {
            w.u32(p);
            put_object(w, &objects[&p]);
        }
        put_u32_opt(w, root);
        for v in mix_bin_headroom {
            w.u32(*v);
        }
        w.u32(effect_desc.len() as u32);
        for (index, d) in effect_desc {
            let EffectDesc { ty, words } = d;
            w.u32(*index).u32(*ty);
            w.list(words, |w, v| {
                w.u32(*v);
            });
        }
        put_opt(w, effects, |w, e| {
            let EffectsImage {
                desc,
                count,
                state,
                rings,
            } = e;
            for v in [desc, count, state, rings] {
                w.u32(*v);
            }
        });
        put_listener(w, listener);
        w.bool(*light_hrtf_4_channel);
        w.u64(*frames_rendered).u64(*next_token);
        put_u32_opt(w, reverb_index);
        put_opt(w, voice_rings, put_rings);
    }

    pub(crate) fn get_snapshot(r: &mut Reader<'_>) -> Result<SavedDs, Error> {
        let layout = (r.u32()?, r.u32()?);
        let objects = r.list(5, |r| Ok((r.u32()?, get_object(r)?)))?;
        let root = get_u32_opt(r)?;
        let mut mix_bin_headroom = [0u32; 32];
        for v in &mut mix_bin_headroom {
            *v = r.u32()?;
        }
        let effect_desc = r.list(12, |r| {
            Ok((
                r.u32()?,
                EffectDesc {
                    ty: r.u32()?,
                    words: r.list(4, |r| r.u32())?,
                },
            ))
        })?;
        let effects = get_opt(r, |r| {
            Ok(EffectsImage {
                desc: r.u32()?,
                count: r.u32()?,
                state: r.u32()?,
                rings: r.u32()?,
            })
        })?;
        let saved = SavedDs {
            layout,
            objects,
            root,
            mix_bin_headroom,
            effect_desc: effect_desc.into_iter().collect(),
            effects,
            listener: get_listener(r)?,
            light_hrtf_4_channel: r.bool()?,
            frames_rendered: r.u64()?,
            next_token: r.u64()?,
            reverb_index: get_u32_opt(r)?,
            voice_rings: get_opt(r, get_rings)?,
        };
        let mut ptrs: Vec<u32> = saved.objects.iter().map(|(p, _)| *p).collect();
        ptrs.sort_unstable();
        ptrs.dedup();
        if ptrs.len() != saved.objects.len() {
            return Err(corrupt("an object pointer appears twice"));
        }
        let root_ok = saved.root.is_none_or(|p| {
            saved
                .objects
                .iter()
                .any(|(q, o)| *q == p && matches!(o, Object::Root(_)))
        });
        if !root_ok {
            return Err(corrupt("the root is not a saved IDirectSound"));
        }
        Ok(saved)
    }

    /// Whether `s` fits this DirectSound: built from the same layout.
    pub(crate) fn check_snapshot(&self, s: &SavedDs) -> Result<(), String> {
        let ours = (self.layout.stream_vtable, self.layout.effect_count);
        if s.layout != ours {
            return Err(format!(
                "saved with stream vtable and effect count {:#x?}, this one has {:#x?}",
                s.layout, ours
            ));
        }
        Ok(())
    }

    /// Replaces every saved field; `s` was checked by
    /// [`DirectSound::check_snapshot`].
    pub(crate) fn apply_snapshot(&mut self, s: SavedDs) {
        let DirectSound {
            mixer: _,
            layout: _,
            objects,
            root,
            mix_bin_headroom,
            effect_desc,
            effects,
            listener,
            light_hrtf_4_channel,
            frames_per_do_work: _,
            frames_rendered,
            consumer: _,
            next_lease: _,
            next_token,
            ignored: _,
            reverb_index,
            voice_rings,
            pad_router: _,
        } = self;
        *objects = s.objects.into_iter().collect();
        *root = s.root;
        *mix_bin_headroom = s.mix_bin_headroom;
        *effect_desc = s.effect_desc;
        *effects = s.effects;
        *listener = s.listener;
        *light_hrtf_4_channel = s.light_hrtf_4_channel;
        *frames_rendered = s.frames_rendered;
        *next_token = s.next_token;
        *reverb_index = s.reverb_index;
        *voice_rings = s.voice_rings;
    }
}
