//! Guest voice identity on the same ordered clock as DirectSound delivery.
//! A pooled instance can take over an already playing channel, or expire
//! without submitting any bytes. Both decisions precede the mixer API.

use std::sync::Arc;

use guest_layout::datum::DatumArray;
use guest_layout::sound::{SoundChannel, SoundInstance};
use guest_layout::{DatumHandle, GuestMemory, MemFault, points};

/// The salt distinguishes different lifetimes in the same pool slot. Tag
/// and permutation distinguish instances which submit identical PCM.
fn identity(m: &dyn GuestMemory, handle: u32) -> Result<Option<String>, MemFault> {
    let Some(pool) = points::SOUND_INSTANCES.pointer(m)? else { return Ok(None) };
    let Some(addr) = DatumArray::at(pool).try_get(m, DatumHandle(handle))? else {
        return Ok(None);
    };
    let instance = SoundInstance::at(addr);
    Ok(Some(format!(
        "{handle:x} {:x} {:x} {} {} {} {} {}",
        instance.tag(m)?.raw(),
        instance.loop_owner(m)?.raw(),
        instance.track(m)?,
        instance.pitch_range(m)?,
        instance.permutation(m)?,
        instance.chunk(m)?,
        instance.channel(m)?,
    )))
}

fn instance_event(hle: &dsound::Hle, guest: &dyn GuestMemory, event: &str, id: u32, value: u32) {
    let mut ds = hle.direct_sound();
    match identity(guest, id) {
        Ok(Some(identity)) => ds.voice_event(format_args!("{event} {identity} {value:x}")),
        _ => ds.voice_event(format_args!("unreadable {event} {id:x} {value:x}")),
    }
}

pub(crate) fn install(b: &mut hooks::Builder, audio: &Arc<dsound::Hle>) {
    let hle = Arc::clone(audio);
    b.raw().observe_before(&points::SOUND_IMPULSE_ALLOCATE, "voice request", move |o| {
        let g = o.guest();
        let sp = o.cpu.esp;
        if let (Ok(tag), Ok(channel), Ok(rule)) = (
            g.u32(sp.wrapping_add(4)),
            g.u32(sp.wrapping_add(8)),
            g.u32(sp.wrapping_add(16)),
        ) {
            hle.direct_sound().voice_event(format_args!("request {tag:x} {channel:x} {rule:x}"));
        } else {
            hle.direct_sound().voice_event(format_args!("unreadable request"));
        }
    });
    let hle = Arc::clone(audio);
    b.raw().observe_after(&points::SOUND_IMPULSE_ALLOCATE, "voice admission", move |o| {
        if o.cpu.eax == u32::MAX {
            hle.direct_sound().voice_event(format_args!("rejected"));
        } else {
            instance_event(&hle, &*o.guest(), "admit", o.cpu.eax, 0);
        }
    });
    let hle = Arc::clone(audio);
    b.raw().observe_before(&points::SOUND_CHOOSE_CHANNEL, "voice channel candidate", move |o| {
        let g = o.guest();
        match g.u32(o.cpu.esp.wrapping_add(4)) {
            Ok(id) => instance_event(&hle, &*g, "candidate", id, 0),
            Err(_) => hle.direct_sound().voice_event(format_args!("unreadable candidate")),
        }
    });
    let hle = Arc::clone(audio);
    b.raw().observe_after(&points::SOUND_CHOOSE_CHANNEL, "voice channel selection", move |o| {
        let channel = o.cpu.eax as i16;
        hle.direct_sound().voice_event(format_args!("selected {channel}"));
    });
    let hle = Arc::clone(audio);
    b.raw().observe_before(&points::SOUND_CHANNEL_UPDATE, "voice channel submission", move |o| {
        let g = o.guest();
        let owner = || {
            let channel = g.u16(o.cpu.esp.wrapping_add(4)).ok()?;
            let m: &dyn GuestMemory = &*g;
            let base = points::SOUND_CHANNELS.pointer(m).ok()??;
            let row = SoundChannel::at(base.wrapping_add(u32::from(channel) * SoundChannel::SIZE));
            Some((row.owner(m).ok()?.raw(), channel))
        };
        match owner() {
            Some((id, channel)) => instance_event(&hle, &*g, "channel", id, channel.into()),
            None => hle.direct_sound().voice_event(format_args!("unreadable channel")),
        }
    });
    let hle = Arc::clone(audio);
    b.raw().observe_before(&points::SOUND_FINISH_INSTANCE, "voice finish and owner callback", move |o| {
        let g = o.guest();
        match g.u32(o.cpu.esp.wrapping_add(4)) {
            Ok(reason) => instance_event(&hle, &*g, "finish", o.cpu.eax, reason),
            Err(_) => hle.direct_sound().voice_event(format_args!("unreadable finish")),
        }
    });
    let hle = Arc::clone(audio);
    b.raw().observe_before(&points::SOUND_FREE_INSTANCE, "voice release after callback", move |o| {
        instance_event(&hle, &*o.guest(), "free", o.cpu.edi, 0);
    });
}
