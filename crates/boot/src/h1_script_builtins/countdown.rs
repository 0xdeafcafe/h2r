use std::{fs::File, io::BufReader};

use h1convert::shad::COUNTDOWN_DIGITS;

const TICKS_PER_SECOND: u64 = 30;

#[derive(Default)]
pub(super) struct Countdown {
    pub remaining: i32,
    pub running: bool,
    last_ms: u64,
    ids: [u32; COUNTDOWN_DIGITS.len()],
}

impl Countdown {
    /// Time follows the simulation clock, including ticks advanced in a
    /// cinematic. Halo 1 derives elapsed whole milliseconds at 0x56e40.
    pub fn tick(&mut self, tick: u32) {
        if self.running {
            let now = u64::from(tick) * 1000 / TICKS_PER_SECOND;
            if let Some(delta) = now.checked_sub(self.last_ms) {
                let delta = i32::try_from(delta).unwrap_or(i32::MAX);
                self.remaining = self.remaining.saturating_sub(delta).max(0);
            }
            self.last_ms = now;
        }
    }

    /// Resolves the inputs written by the countdown shader rebuild. Each
    /// cache has its own packed StringIDs.
    pub fn load_ids(&mut self, name: &str) {
        self.ids.fill(0);
        if name != "d40" {
            return;
        }
        let Some(file) = crate::native_world_glue::locate(name) else {
            return;
        };
        let Ok(reader) = File::open(file.path) else {
            return;
        };
        let Ok(cache) = map::Map::from_region(BufReader::new(reader), file.offset, file.len) else {
            return;
        };
        for (index, entry) in cache.strings().iter().enumerate() {
            let Some(text) = entry.as_deref() else {
                continue;
            };
            if let Some(place) = COUNTDOWN_DIGITS.iter().position(|&name| name == text)
                && self.ids[place] == 0
                && let Ok(id) = map::build::string_id(index, text)
            {
                self.ids[place] = id;
            }
        }
    }

    pub fn shader_value(&self, id: u32) -> Option<f32> {
        self.ids.iter().position(|&key| id == key)
            .map(|place| f32::from(self.digit(place as i16)) * 0.1)
    }

    /// Halo 1 0x56aa0. The seconds and minutes tens roll over at 6.
    pub fn digit(&self, place: i16) -> i16 {
        const PLACES: [(i32, i32); 9] = [
            (1, 10), (10, 10), (100, 10), (1000, 10), (10_000, 6),
            (60_000, 10), (600_000, 6), (3_600_000, 10), (36_000_000, 10),
        ];
        match place {
            -1 => self.remaining as i16,
            0..=8 => {
                let (divisor, modulus) = PLACES[place as usize];
                (self.remaining / divisor % modulus) as i16
            }
            _ => 0,
        }
    }

    pub fn save(&self) -> [u8; 13] {
        let mut bytes = [0; 13];
        bytes[..4].copy_from_slice(&self.remaining.to_le_bytes());
        bytes[4] = u8::from(self.running);
        bytes[5..].copy_from_slice(&self.last_ms.to_le_bytes());
        bytes
    }

    pub fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
        let [a, b, c, d, running @ 0..=1, millis @ ..] = bytes else {
            return Err("invalid numeric countdown state".into());
        };
        self.last_ms = u64::from_le_bytes(millis.try_into().map_err(|_| "invalid countdown clock")?);
        self.remaining = i32::from_le_bytes([*a, *b, *c, *d]);
        self.running = *running != 0;
        Ok(())
    }
}
