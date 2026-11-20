//! The gamertag pool shared by Live and host-made Clankers.
use crate::Gamertag;

const FIRST: [&str; 32] = [
    "Rusty", "Bolt", "Clank", "Sprocket", "Gear", "Rivet", "Spanner", "Copper", "Chrome", "Static",
    "Dynamo", "Piston", "Widget", "Gizmo", "Tin", "Servo", "Crank", "Flux", "Volt", "Cog",
    "Ratchet", "Socket", "Diode", "Pixel", "Turbo", "Sparky", "Nickel", "Zinc", "Magnet", "Coil",
    "Fuse", "Relay",
];

const SECOND: [&str; 32] = [
    "Lugnut", "Bucket", "Gasket", "Washer", "Bearing", "Sprung", "Wrench", "Spindle", "Toaster",
    "Kettle", "Grinder", "Hinge", "Sprog", "Bracket", "Flange", "Nozzle", "Valve", "Lever",
    "Pulley", "Dial", "Gauge", "Bulb", "Switch", "Fender", "Axle", "Clamp", "Tube", "Rotor",
    "Pinion", "Shim", "Stud", "Grommet",
];

/// SplitMix64 stream used when generating a roster.
pub struct Draw(u64);

impl Draw {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        (z ^ (z >> 31)) % n
    }

    /// The legacy roster fields use this stream after drawing the name.
    pub fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * (self.below(10_001) as f32 / 10_000.0)
    }

    pub fn gamertag(&mut self, attempt: u32) -> Gamertag {
        loop {
            let first = FIRST[self.below(FIRST.len() as u64) as usize];
            let second = SECOND[self.below(SECOND.len() as u64) as usize];
            let text = if attempt < 4 {
                format!("{first} {second}")
            } else {
                format!("{first}{second}{}", self.below(100))
            };
            if let Ok(tag) = Gamertag::new(&text) {
                return tag;
            }
        }
    }

    /// Fill the host's seats once, retaining each name as the count changes.
    pub fn local_roster(&mut self) -> Vec<String> {
        let mut tags = Vec::with_capacity(super::MAX_BOTS);
        while tags.len() < super::MAX_BOTS {
            let name = self.gamertag(0).to_string();
            // The System Link roster reserves one of its 15 units for the bot glyph.
            if name.len() <= 14 && !tags.contains(&name) {
                tags.push(name);
            }
        }
        tags
    }
}
