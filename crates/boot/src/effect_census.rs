//! `H2_EFFECT_CENSUS=1`: a read-only census of the effect, particle and
//! light datum arrays at each present, so a headless run can show that an
//! explosion happened and when (the grenade A/B in docs/perf/frame-budget.md).
//!
//! Every present it reads the arrays' headers and live slots (salt word
//! non-zero, below the high-water mark) and counts the datums that are new
//! since the last present. A new timed light (`light::flags::TIMED`, made
//! by an effect part, not a placement) prints one line; one with a
//! bounding radius of at least [`EXPLOSION_RADIUS`] is counted as
//! explosion-class (frag and plasma grenades: radius 2, 0.8 s; muzzle
//! flashes are under 2). The exit line gives the totals and the presents
//! of the explosion-class lights. Nothing is written to guest memory.

use std::collections::HashSet;

use engine::datum::DatumArray;
use engine::memory::{GuestMemory, MemFault};
use engine::render::light;

/// The switch.
pub const ENV: &str = "H2_EFFECT_CENSUS";

/// Timed lights at least this large are counted as explosions.
pub const EXPLOSION_RADIUS: f32 = 1.5;

/// The arrays: name and the guest global holding the header address
/// (docs/engine/engine-map.md, "Effects"; e3-lighting.md, "The light record").
pub const EFFECT: (&str, u32) = ("effect", 0x004e_a93c);
pub const PARTICLE_SYSTEM: (&str, u32) = ("particle_system", 0x0051_0c74);
pub const PARTICLES: (&str, u32) = ("particles", 0x0051_ec84);
pub const LIGHTS: (&str, u32) = ("lights", light::ARRAY_POINTER);

/// Most per-light lines printed.
const MAX_LINES: u32 = 400;

/// Whether `value` (of [`ENV`]) asks for the census.
pub fn requested_by(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// One array's live handles and what was created so far.
#[derive(Default)]
struct Tracked {
    live: HashSet<u32>,
    created: u64,
    peak: usize,
}

/// One new timed light.
#[derive(Clone, Debug, PartialEq)]
pub struct NewLight {
    pub handle: u32,
    pub radius: f32,
    pub position: [f32; 3],
    pub explosion: bool,
}

/// What one present found new.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub new_effects: usize,
    pub new_particle_systems: usize,
    pub new_particles: usize,
    pub live: [usize; 4],
    pub lights: Vec<NewLight>,
}

#[derive(Default)]
pub struct Census {
    arrays: [Tracked; 4],
    explosions: Vec<u64>,
    timed_lights: u64,
    lines: u32,
    errors: u64,
}

/// Live handles of the array whose header address is at `pointer`.
fn live_handles(m: &dyn GuestMemory, pointer: u32) -> Result<Vec<(u32, u32)>, MemFault> {
    let at = m.u32(pointer)?;
    if at == 0 {
        return Ok(Vec::new());
    }
    let array = DatumArray::at(at);
    let h = array.header(m)?;
    let n = h.high_water.clamp(0, h.maximum_count.max(0));
    let mut out = Vec::new();
    for i in 0..n {
        let addr = array.element_addr(m, i)?;
        let salt = m.u16(addr)?;
        if salt != 0 {
            out.push((((salt as u32) << 16) | i as u32, addr));
        }
    }
    Ok(out)
}

impl Census {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads the arrays; what is new since the last call.
    pub fn sample(&mut self, m: &dyn GuestMemory) -> Result<Sample, MemFault> {
        let mut s = Sample::default();
        for (k, (_, pointer)) in [EFFECT, PARTICLE_SYSTEM, PARTICLES, LIGHTS]
            .into_iter()
            .enumerate()
        {
            let handles = live_handles(m, pointer)?;
            let t = &mut self.arrays[k];
            let now: HashSet<u32> = handles.iter().map(|h| h.0).collect();
            let fresh: Vec<(u32, u32)> = handles
                .into_iter()
                .filter(|h| !t.live.contains(&h.0))
                .collect();
            t.created += fresh.len() as u64;
            t.peak = t.peak.max(now.len());
            s.live[k] = now.len();
            t.live = now;
            match k {
                0 => s.new_effects = fresh.len(),
                1 => s.new_particle_systems = fresh.len(),
                2 => s.new_particles = fresh.len(),
                _ => {
                    for (handle, addr) in fresh {
                        let mut b = [0u8; light::RECORD_SIZE];
                        m.read(addr, &mut b)?;
                        let r = light::LightRecord::parse(&b);
                        let effect_part = r.flags & light::flags::TIMED != 0
                            && r.flags & light::flags::PLACED == 0;
                        if !effect_part {
                            continue;
                        }
                        s.lights.push(NewLight {
                            handle,
                            radius: r.bounds_radius,
                            position: r.bounds_centre,
                            explosion: r.bounds_radius >= EXPLOSION_RADIUS,
                        });
                    }
                }
            }
        }
        Ok(s)
    }

    /// [`sample`](Self::sample) at present `present`, with its lines.
    pub fn at_present(&mut self, m: &dyn GuestMemory, present: u64) -> Vec<String> {
        let s = match self.sample(m) {
            Ok(s) => s,
            Err(_) => {
                // Before the arrays exist (menus, loads) the pointers may
                // name unmapped memory: not an error worth a line.
                self.errors += 1;
                return Vec::new();
            }
        };
        let mut lines = Vec::new();
        for l in &s.lights {
            self.timed_lights += 1;
            if l.explosion {
                self.explosions.push(present);
            }
            if self.lines < MAX_LINES {
                self.lines += 1;
                lines.push(format!(
                    "effects: present {present}: new timed light {:#010x} radius {:.2} at ({:.1}, {:.1}, {:.1}){}; new this present: {} effects, {} particle systems, {} particles; live: {} effects, {} particle systems, {} particles, {} lights",
                    l.handle,
                    l.radius,
                    l.position[0],
                    l.position[1],
                    l.position[2],
                    if l.explosion { " EXPLOSION" } else { "" },
                    s.new_effects,
                    s.new_particle_systems,
                    s.new_particles,
                    s.live[0],
                    s.live[1],
                    s.live[2],
                    s.live[3],
                ));
            }
        }
        lines
    }

    /// The exit line.
    pub fn summary(&self) -> String {
        let presents: Vec<String> = self.explosions.iter().map(|p| p.to_string()).collect();
        format!(
            "effects: {} explosion-class lights (timed, radius >= {EXPLOSION_RADIUS}) at presents [{}]; {} timed lights; created {} effects, {} particle systems, {} particles; peak live {} effects, {} particle systems, {} particles, {} lights",
            self.explosions.len(),
            presents.join(" "),
            self.timed_lights,
            self.arrays[0].created,
            self.arrays[1].created,
            self.arrays[2].created,
            self.arrays[0].peak,
            self.arrays[1].peak,
            self.arrays[2].peak,
            self.arrays[3].peak,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::datum::layout;
    use engine::memory::FlatMemory;

    const BASE: u32 = 0x0040_0000;
    const HEADERS: u32 = 0x0060_0000;

    fn mem() -> FlatMemory {
        FlatMemory::new(BASE, 0x0040_0000)
    }

    /// A header for `count` slots of `size` bytes at `data`, pointed at by
    /// `pointer`, with `high_water` slots in use.
    fn array(
        m: &mut FlatMemory,
        pointer: u32,
        header: u32,
        size: i32,
        count: i32,
        data: u32,
        high_water: i32,
    ) {
        m.set_u32(pointer, header).unwrap();
        m.set_i32(header + layout::MAXIMUM_COUNT, count).unwrap();
        m.set_i32(header + layout::ELEMENT_SIZE, size).unwrap();
        m.set_i32(header + layout::HIGH_WATER, high_water).unwrap();
        m.set_u32(header + layout::DATA, data).unwrap();
    }

    fn arrays(m: &mut FlatMemory) {
        array(m, EFFECT.1, HEADERS, 0x190, 0x100, HEADERS + 0x1000, 4);
        array(
            m,
            PARTICLE_SYSTEM.1,
            HEADERS + 0x100,
            0x54,
            0x80,
            HEADERS + 0x20000,
            8,
        );
        array(
            m,
            PARTICLES.1,
            HEADERS + 0x200,
            0x40,
            0x400,
            HEADERS + 0x30000,
            16,
        );
        array(
            m,
            LIGHTS.1,
            HEADERS + 0x300,
            0x110,
            350,
            HEADERS + 0x40000,
            4,
        );
    }

    fn light_at(m: &mut FlatMemory, i: u32, salt: u16, flags: u16, radius: f32) {
        let at = HEADERS + 0x40000 + i * 0x110;
        m.set_u16(at, salt).unwrap();
        m.set_u16(at + 2, flags).unwrap();
        m.write(
            at + light::layout::BOUNDS_RADIUS as u32,
            &radius.to_le_bytes(),
        )
        .unwrap();
    }

    #[test]
    fn a_timed_light_of_grenade_size_is_an_explosion_once() {
        let mut m = mem();
        arrays(&mut m);
        let mut c = Census::new();
        // A placed light and a muzzle flash are there from the start.
        light_at(
            &mut m,
            0,
            0x8001,
            light::flags::LIVE | light::flags::PLACED | light::flags::TIMED,
            4.0,
        );
        light_at(
            &mut m,
            1,
            0x8002,
            light::flags::LIVE | light::flags::TIMED,
            0.85,
        );
        let first = c.at_present(&m, 100);
        assert_eq!(
            first.len(),
            1,
            "the muzzle flash is a new timed light: {first:?}"
        );
        assert!(!first[0].contains("EXPLOSION"));
        // Nothing new: no lines.
        assert!(c.at_present(&m, 101).is_empty());
        // The grenade: a frag light (radius 2) and six particle systems.
        light_at(
            &mut m,
            2,
            0x8003,
            light::flags::LIVE | light::flags::TIMED,
            2.0,
        );
        for i in 0..6 {
            m.set_u16(HEADERS + 0x20000 + i * 0x54, 0x9000 + i as u16)
                .unwrap();
        }
        let lines = c.at_present(&m, 4452);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with("effects: present 4452: new timed light 0x80030002 radius 2.00"),
            "{}",
            lines[0]
        );
        assert!(
            lines[0].contains("EXPLOSION; new this present: 0 effects, 6 particle systems"),
            "{}",
            lines[0]
        );
        assert!(
            c.at_present(&m, 4453).is_empty(),
            "the same light is not counted twice"
        );
        let s = c.summary();
        assert!(s.starts_with("effects: 1 explosion-class lights (timed, radius >= 1.5) at presents [4452]; 2 timed lights"), "{s}");
        assert!(s.contains("created 0 effects, 6 particle systems"), "{s}");
    }

    #[test]
    fn a_reused_slot_with_a_new_salt_is_new() {
        let mut m = mem();
        arrays(&mut m);
        let mut c = Census::new();
        light_at(
            &mut m,
            0,
            0x8001,
            light::flags::LIVE | light::flags::TIMED,
            2.0,
        );
        assert_eq!(c.at_present(&m, 1).len(), 1);
        light_at(
            &mut m,
            0,
            0x8004,
            light::flags::LIVE | light::flags::TIMED,
            2.0,
        );
        assert_eq!(c.at_present(&m, 2).len(), 1);
        assert!(c.summary().starts_with("effects: 2 explosion-class lights"));
    }

    #[test]
    fn arrays_not_made_yet_are_empty_not_errors() {
        let m = mem();
        let mut c = Census::new();
        assert!(c.at_present(&m, 1).is_empty());
        assert_eq!(c.errors, 0);
        assert!(c.summary().starts_with("effects: 0 explosion-class lights"));
    }

    #[test]
    fn the_switch() {
        assert!(requested_by(Some("1")));
        assert!(!requested_by(Some("0")));
        assert!(!requested_by(Some("")));
        assert!(!requested_by(None));
    }
}
