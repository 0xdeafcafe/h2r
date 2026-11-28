//! Supply the converted Halo 1 HUD screen effect with inputs from the active
//! view (docs/halo1/hud.md). The native post draws its copy, blur and pixel
//! passes, the blur at the radius these values carry
//! (`render_bridge::scope_zoom_hud::zoom_blur`).

use guest_layout::handle::DatumHandle;
use guest_layout::memory::GuestMemory;
use engine::render_loop::occlusion::guest::{CAMERA, VIEW_PLAYER, body};
use runtime::guest::Permissions;
use tags::scope_zoom::MAGIC;

// The profile reads native_post shares (src/scope_zoom_hud.rs there).
use render_bridge::scope_zoom_hud::{CONVOLUTION_RETURN, blur_words, hud_profile, profile, tag_def};

use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy, points};

// 0x0002d140 copies all four caller values here. 0x000338bf/0x000338c8
// supply the first two to the shader as scalar inputs 41 and 42.
const SCREEN_VALUES: u32 = 0x004c_19b0;
const PLAYER_CONTROL: u32 = 0x004e_d284;
const PLAYER_COUNT: u32 = 4;
const VISION_STATE_SIZE: u32 = 12;
const ZOOM_LEVEL: u32 = 0x240; // 0x000c8876, -1 when unzoomed
const VISION_ENABLED: u32 = 1;
const BUTTON_DOWN: u32 = 2;

pub struct ScopeZoom;

impl Feature for ScopeZoom {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-scope-zoom", "Halo 1 zoom filters").doc("docs/halo1/hud.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let states = b.guest_alloc(PLAYER_COUNT * VISION_STATE_SIZE, Permissions::RW)?;
        b.snapshot(SnapshotPolicy::Stateless);
        b.raw().before(
            &points::GAME_ACTIVATE,
            "clear scope night vision",
            move |c| {
                c.ctx
                    .memory()
                    .write(states, [0; (PLAYER_COUNT * VISION_STATE_SIZE) as usize])?;
                Ok(Flow::Continue)
            },
        ).first().context_only();
        b.raw().before(
            &points::UNIT_LIGHT_UPDATE,
            "update scope night vision",
            move |c| {
                update_vision(&mut *c.ctx.memory(), states, c.cpu.eax);
                Ok(Flow::Continue)
            },
        ).after_feature("h1-script-builtins").context_only();
        b.raw()
            .before(
                &points::SCREEN_EFFECT,
                "set scope shader inputs",
                move |c| {
                    prepare_hud(
                        &mut *c.ctx.memory(),
                        states,
                        c.cpu.eax,
                        c.cpu.esp,
                        c.cpu.ebp,
                    );
                    Ok(Flow::Continue)
                },
            )
            .first()
            .context_only();
        b.raw()
            .before(
                &points::SCREEN_CONVOLUTION_BLUR,
                "set scope blur radius",
                |c| {
                    prepare_blur(&mut *c.ctx.memory(), c.cpu.esp);
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        Ok(())
    }
}

fn local_state(mem: &dyn GuestMemory, states: u32, unit: u32) -> Option<u32> {
    if unit == u32::MAX {
        return None;
    }
    let control = mem.u32(PLAYER_CONTROL).ok()?;
    if control == 0 {
        return None;
    }
    (0..PLAYER_COUNT)
        .find(|&user| mem.u32(control + 0x94 * user + 0x14).ok() == Some(unit))
        .map(|user| states + VISION_STATE_SIZE * user)
}

fn weapon_has_vision(mem: &dyn GuestMemory, unit_body: u32) -> Option<bool> {
    let slot = mem.u8(unit_body + 0x212).ok()?;
    if slot >= 4 {
        return None;
    }
    let weapon = mem.u32(unit_body + 0x218 + 4 * u32::from(slot)).ok()?;
    let weapon_body = body(mem, DatumHandle::from_raw(weapon)?.raw()).ok()?;
    let (def, _) = tag_def(mem, mem.u32(weapon_body).ok()?, *b"weap")?;
    let (hud, _) = tag_def(mem, mem.u32(def + 0x2b4).ok()?, *b"nhdt")?;
    let count = mem.u32(hud + 0x20).ok()?.min(64);
    let entries = mem.u32(hud + 0x24).ok()?;
    Some((0..count).any(|index| {
        let entry = entries + index * 0x50;
        [0x2c, 0x34, 0x3c]
            .into_iter()
            .filter_map(|offset| profile(mem, mem.u32(entry + offset).ok()?))
            .any(|p| {
                [p.desaturation, p.tint]
                    .into_iter()
                    .any(|filter| filter.enabled() && filter.uses_night_vision())
            })
    }))
}

#[derive(Clone, Copy, Default)]
struct Vision {
    strength: f32,
    state: u32,
}

impl Vision {
    fn read(mem: &dyn GuestMemory, at: u32, unit: u32) -> Option<Self> {
        if mem.u32(at).ok()? != unit {
            return None;
        }
        Some(Self {
            strength: mem.f32(at + 4).ok()?,
            state: mem.u32(at + 8).ok()?,
        })
    }

    fn tick(self, down: bool, scope: bool, seconds: f32) -> Self {
        let mut enabled = self.state & VISION_ENABLED != 0;
        if scope && down && self.state & BUTTON_DOWN == 0 {
            enabled = !enabled;
        }
        // Halo 1 0x00045c6c / 0x00045cae: +1/12 or -1/24 per 30 Hz tick.
        let rate = if enabled && scope { 2.5 } else { -1.25 };
        Self {
            strength: (self.strength + seconds * rate).clamp(0.0, 1.0),
            state: (enabled as u32 * VISION_ENABLED) | (down as u32 * BUTTON_DOWN),
        }
    }
}

fn update_vision(mem: &mut dyn GuestMemory, states: u32, unit: u32) -> Option<()> {
    let state = local_state(mem, states, unit)?;
    let unit_body = body(mem, unit).ok()?;
    // The same flashlight action tested by Halo 2 at 0x000c5115 and
    // 0x000c52be. The converted scope tracks its own button edge so it
    // works in multiplayer, where the original need not emit a light.
    let down = mem.u32(unit_body + 0x148).ok()? & 4 != 0;
    let scope = mem.u8(unit_body + ZOOM_LEVEL).ok()? != u8::MAX
        && weapon_has_vision(mem, unit_body).unwrap_or(false);
    let time = mem.u32(points::GAME_TIME.base).ok()?;
    let seconds = mem.f32(time + 4).ok()?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let next = Vision::read(mem, state, unit)
        .unwrap_or_default()
        .tick(down, scope, seconds);
    mem.set_u32(state, unit).ok()?;
    mem.set_u32(state + 4, next.strength.to_bits()).ok()?;
    mem.set_u32(state + 8, next.state).ok()
}

fn prepare_hud(
    mem: &mut dyn GuestMemory,
    states: u32,
    datum: u32,
    esp: u32,
    ebp: u32,
) -> Option<()> {
    let profile = hud_profile(mem, datum, esp)?;
    let unit = mem.u32(VIEW_PLAYER).ok()?;
    let zoomed = if unit == u32::MAX {
        false
    } else {
        mem.u8(body(mem, unit).ok()? + ZOOM_LEVEL).ok()? != u8::MAX
    };
    let vision = local_state(mem, states, unit)
        .and_then(|at| Vision::read(mem, at, unit))
        .map_or(0.0, |state| state.strength);
    let fov = mem.f32(CAMERA + 0x28).ok()?;
    let values = [
        profile.desaturation.amount(zoomed, vision).to_bits(),
        profile.tint.amount(zoomed, vision).to_bits(),
        profile.blur.at(fov, zoomed).to_bits(),
        MAGIC,
    ];
    // 0x0022bbbb supplies the common HUD values by pointer. The HUD draw's
    // four locals at EBP+0x5c..0x68 (effect, layout, anchor x/y) are dead
    // after the call, so use those for a per-effect copy.
    let scratch = ebp.wrapping_add(0x5c);
    for (i, word) in values.into_iter().enumerate() {
        mem.set_u32(scratch + 4 * i as u32, word).ok()?;
    }
    mem.set_u32(esp + 4, scratch).ok()
}

/// The guest blur's arms of [`blur_words`], for a `d3d-reference` build
/// whose `0x0002d140` is the guest's.
fn prepare_blur(mem: &mut dyn GuestMemory, esp: u32) -> Option<()> {
    if mem.u32(esp).ok()? != CONVOLUTION_RETURN {
        return None;
    }
    let values: [u32; 4] = std::array::from_fn(|i| mem.u32(SCREEN_VALUES + 4 * i as u32).unwrap_or(0));
    let (steps, scale) = (mem.u32(esp + 4).ok()?, mem.u32(esp + 8).ok()?);
    let (new_steps, new_scale) = blur_words(values, steps, scale)?;
    mem.set_u32(esp + 4, new_steps).ok()?;
    mem.set_u32(esp + 8, new_scale).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;
    use render_bridge::scope_zoom_hud::HUD_SCREEN_RETURN;
    use guest_layout::tags::{TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, entry_layout, index_layout};
    use tags::scope_zoom::{Blur, Filter, Profile};

    #[test]
    fn flashlight_edges_fade_once_per_simulation_tick() {
        let off = Vision::default();
        let tick = 1.0 / 30.0;
        let rising = off.tick(true, true, tick);
        assert_eq!(rising.state, VISION_ENABLED | BUTTON_DOWN);
        assert!((rising.strength - 1.0 / 12.0).abs() < 1.0e-6);
        let held = rising.tick(true, true, tick);
        assert_eq!(held.state, rising.state);
        let released = held.tick(false, true, tick);
        let falling = released.tick(true, true, tick);
        assert_eq!(falling.state, BUTTON_DOWN);
        assert!((released.strength - falling.strength - 1.0 / 24.0).abs() < 1.0e-6);
        assert_eq!(off.tick(true, false, tick).strength, 0.0);
    }

    #[test]
    fn hud_and_blur_hooks_use_the_scoped_view_without_overwriting_hud_values() {
        const TAG: u32 = 0xe123_0000;
        const TABLE: u32 = 0x70000;
        const INDEX: u32 = 0x70100;
        const DEF: u32 = 0x70200;
        const STATES: u32 = 0x70300;
        const CONTROL: u32 = 0x70400;
        const OBJECTS: u32 = 0x70800;
        const HEADERS: u32 = 0x70900;
        const BODY: u32 = 0x70a00;
        const ESP: u32 = 0x71000;
        const EBP: u32 = 0x71100;
        const ORIGINAL: u32 = 0x71200;
        const UNIT: u32 = 0xe001_0000;
        let profile = Profile {
            blur: Blur {
                zoom_only: true,
                fov: [1.0, 0.5],
                radius: [0.0, 12.0],
            },
            desaturation: Filter {
                flags: 3,
                channel: 1,
                strength: 1.0,
            },
            tint: Filter {
                flags: 1,
                channel: 0,
                strength: 0.25,
            },
            ..Profile::default()
        };
        let mut mem = FlatMemory::new(0x70000, 0x0055_0000 - 0x70000);
        for (addr, value) in [
            (TAG_ENTRIES_GLOBAL, TABLE),
            (TAG_INDEX_GLOBAL, INDEX),
            (INDEX + index_layout::TAG_COUNT, 1),
            (TABLE, u32::from_be_bytes(*b"egor")),
            (TABLE + entry_layout::DATUM, TAG),
            (TABLE + entry_layout::ADDRESS, DEF),
            (TABLE + entry_layout::SIZE, 0x90),
            (PLAYER_CONTROL, CONTROL),
            (CONTROL + 0x14 + 0x94, UNIT),
            (VIEW_PLAYER, UNIT),
            (points::OBJECTS.base, OBJECTS),
            (OBJECTS + 0x44, HEADERS),
            (HEADERS + 8, BODY),
            (STATES + VISION_STATE_SIZE, UNIT),
            (ORIGINAL, 0xfeed_beef),
            (ESP + 4, ORIGINAL),
        ] {
            mem.set_u32(addr, value).unwrap();
        }
        for (i, word) in profile.words().into_iter().enumerate() {
            mem.set_u32(DEF + i as u32 * 4, word).unwrap();
        }
        mem.set_f32(STATES + VISION_STATE_SIZE + 4, 0.5).unwrap();
        mem.set_f32(CAMERA + 0x28, 0.75).unwrap();
        mem.set_u8(BODY + ZOOM_LEVEL, 0).unwrap();
        assert!(prepare_hud(&mut mem, STATES, TAG, ESP, EBP).is_none());

        mem.set_u32(ESP, HUD_SCREEN_RETURN).unwrap();
        assert!(prepare_hud(&mut mem, STATES, TAG + 0x1_0000, ESP, EBP).is_none());
        assert_eq!(prepare_hud(&mut mem, STATES, TAG, ESP, EBP), Some(()));
        let values = mem.u32(ESP + 4).unwrap();
        assert_eq!(values, EBP + 0x5c);
        assert_eq!(mem.u32(ORIGINAL).unwrap(), 0xfeed_beef);
        assert_eq!(mem.f32(values).unwrap(), 0.5);
        assert_eq!(mem.f32(values + 4).unwrap(), 0.25);
        assert_eq!(mem.f32(values + 8).unwrap(), 6.0);
        assert_eq!(mem.u32(values + 12).unwrap(), MAGIC);

        mem.set_f32(SCREEN_VALUES + 8, 6.0).unwrap();
        mem.set_u32(SCREEN_VALUES + 12, MAGIC).unwrap();
        assert!(prepare_blur(&mut mem, ESP).is_none());
        mem.set_u32(ESP, CONVOLUTION_RETURN).unwrap();
        assert_eq!(prepare_blur(&mut mem, ESP), Some(()));
        assert_eq!(mem.f32(ESP + 8).unwrap(), 3.0);
        mem.set_f32(SCREEN_VALUES + 8, 0.0).unwrap();
        assert_eq!(prepare_blur(&mut mem, ESP), Some(()));
        assert_eq!(mem.f32(ESP + 4).unwrap(), 0.0);
    }
}
