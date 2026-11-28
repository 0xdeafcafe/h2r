//! The host's side of the sound events. A [`TableHost`] hears each event
//! after the sim has made it and can change nothing the sim reads: the
//! table's state is the same with any host.

use crate::events::TableEvent;
use crate::instances::SourceParams;
use crate::spatial::SourceSpace;
use sim_world::Handle;

/// One local listener's frame (sound globals `+0x88`, `0x48` bytes a
/// user): forward `+0xc`, left `+0x18`, up `+0x24`, position `+0x30`, in
/// world units with z up.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ear {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub left: [f32; 3],
    pub up: [f32; 3],
}

/// What the host reads once a frame beside the events: the tick, the
/// first listener and where each running sound plays now
/// ([`crate::instances::Sounds::sources`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub tick: u32,
    pub ear: Option<Ear>,
    pub sources: Vec<(Handle, SourceParams)>,
}

/// The host's side of the table's events (docs/engine/sound-instances.md,
/// "Mixer feed"). Each frame the service hands it the events since the
/// last frame, in the order the table made them, then the scene: the
/// guest frame's order (`0x125b40`: the loops' update, the instances'
/// ends, the sources, the channels and streams). Nothing it does reaches
/// the table.
pub trait TableHost: Send {
    fn event(&mut self, event: &TableEvent);
    fn frame(&mut self, scene: &Scene);
    /// The table was emptied with no events (a map's create or drop):
    /// every voice goes at once.
    fn clear(&mut self) {}
    /// The host's `feed.*` counters, by name.
    fn counters(&self) -> Vec<(&'static str, u64)> {
        Vec::new()
    }
}

/// The headless table host: hears nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullTableHost;

impl TableHost for NullTableHost {
    fn event(&mut self, _: &TableEvent) {}
    fn frame(&mut self, _: &Scene) {}
}

/// Where a source plays, in the listener's right, up and front axes
/// (`0x129aa0`'s spaces, `0x12abe0`'s position). `None` for a sound that
/// is not positioned, heard as it is: space 0, an unknown space, or a
/// world source with no listener.
///
/// | Space (word 0 bits 24 to 27) | Position |
/// |---|---|
/// | 1, world | words 3 to 5 (word 0 byte 2 is 0), or words 3 and 4 at the listener's height (byte 2 is 1), less the listener |
/// | 2, listener | words 3 to 5 as forward, left and up |
pub fn heard_at(source: &SourceParams, ear: Option<&Ear>) -> Option<[f32; 3]> {
    let w = |i: usize| f32::from_bits(source.0[i]);
    match SourceSpace::decode((source.0[0] >> 24) as u8) {
        SourceSpace::World => {
            let ear = ear?;
            let p = match (source.0[0] >> 16) as u8 as i8 {
                0 => [w(3), w(4), w(5)],
                1 => [w(3), w(4), ear.position[2]],
                _ => return None,
            };
            let r = [p[0] - ear.position[0], p[1] - ear.position[1], p[2] - ear.position[2]];
            let dot = |a: [f32; 3]| r[0] * a[0] + r[1] * a[1] + r[2] * a[2];
            Some([-dot(ear.left), dot(ear.up), dot(ear.forward)])
        }
        SourceSpace::Listener => Some([-w(4), w(5), w(3)]),
        SourceSpace::Plain | SourceSpace::Other(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(space: u32, kind: u32, p: [f32; 3]) -> SourceParams {
        let mut s = SourceParams::default();
        s.0[0] = (space << 24) | (kind << 16);
        for (i, v) in p.into_iter().enumerate() {
            s.0[3 + i] = v.to_bits();
        }
        s
    }

    const EAR: Ear = Ear { position: [10.0, 0.0, 2.0], forward: [1.0, 0.0, 0.0], left: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0] };

    #[test]
    fn a_world_source_is_heard_where_it_is_from_the_listener() {
        // Ahead 5, to the left 3, above 1: right is minus left.
        assert_eq!(heard_at(&source(1, 0, [15.0, 3.0, 3.0]), Some(&EAR)), Some([-3.0, 1.0, 5.0]));
        // Byte 2 is 1: at the listener's height.
        assert_eq!(heard_at(&source(1, 1, [15.0, 3.0, 9.0]), Some(&EAR)), Some([-3.0, 0.0, 5.0]));
        assert_eq!(heard_at(&source(1, 0, [15.0, 3.0, 3.0]), None), None, "no listener: not positioned");
    }

    #[test]
    fn a_listener_source_is_forward_left_up_and_space_0_is_not_positioned() {
        assert_eq!(heard_at(&source(2, 0, [4.0, 1.0, -1.0]), None), Some([-1.0, -1.0, 4.0]));
        assert_eq!(heard_at(&source(0, 0, [4.0, 1.0, -1.0]), Some(&EAR)), None);
        assert_eq!(heard_at(&source(3, 0, [4.0, 1.0, -1.0]), Some(&EAR)), None);
    }
}
