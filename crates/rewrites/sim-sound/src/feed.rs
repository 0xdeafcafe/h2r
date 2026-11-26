//! The host's side of the sound events. A [`Host`] hears each event after
//! the sim has made it and can change nothing the sim reads: the sim's sound
//! state ([`crate::schedule::Schedule::hash`]) is the same with any host.

use crate::events::SoundEvent;

pub trait Host {
    fn event(&mut self, event: &SoundEvent);
}

/// The headless host: hears nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullHost;

impl Host for NullHost {
    fn event(&mut self, _: &SoundEvent) {}
}

/// Hands the events of one tick to a host, in order.
pub fn deliver(host: &mut dyn Host, events: &[SoundEvent]) {
    for e in events {
        host.event(e);
    }
}
