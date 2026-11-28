//! Draws of the flare passes. The queue, state changes and visibility
//! tickets remain guest-owned. See the retail trace linked from
//! docs/graphics/native-renderer.md.
use crate::native_particles::{HOST, Producer};
use guest_layout::points::scene_effects::*;
use native_render::flares::Site;

pub(super) fn reflection_caller(caller: Option<u32>) -> bool {
    caller == Some(0x0002_f24c)
}

const LATE_RETURN: u32 = 0x0002_e1ca;

// At BEGIN, where ESP equals ESP at the End call, before the CPU pushes its
// return address (for MaskSample and Composite, BEGIN is the last vertex
// call's first push, `scene_effects::FLARE_*`).
// Shared filter helpers have their own locals, saved registers and arguments
// between ESP and the calling 0x46ea0 frame.
fn owns_late_fan(site: Site, word: impl Fn(u32) -> Option<u32>) -> bool {
    match site {
        Site::Mask | Site::MaskSample | Site::Composite => word(0x54) == Some(LATE_RETURN),
        Site::FilterSource => {
            matches!(word(0x90), Some(0x0004_7523 | 0x0004_752f)) && word(0xf0) == Some(LATE_RETURN)
        }
        Site::Filter => word(0x9c) == Some(0x0004_753a) && word(0x100) == Some(LATE_RETURN),
        Site::Visibility | Site::Reflection => false, // separate hooks
    }
}

static LEAVES: [(Site, EffectLeaf); 5] = [
    (Site::Mask, FLARE_MASK),
    (Site::MaskSample, FLARE_MASK_SAMPLE),
    (Site::FilterSource, FLARE_FILTER_SOURCE),
    (Site::Filter, FLARE_FILTER),
    (Site::Composite, FLARE_COMPOSITE),
];

pub(super) fn build(b: &mut hooks::Builder) {
    b.raw()
        .observe_before(&EFFECT_QUERY, "native_flare_query", |o| {
            let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(host) = host {
                host.arm_flare(&mut *o.guest(), Site::Visibility);
            }
        })
        .context_only()
        .optional();
    b.raw()
        .observe_before(&guest_layout::points::render_aspect::FLARE_QUERY_END, "native_flare_query_return", |o| {
            let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(host) = host {
                host.finish(&mut *o.guest(), Producer::Flare(Site::Visibility));
            }
        })
        .context_only();
    for (site, leaf) in &LEAVES {
        let site = *site;
        b.raw()
            .observe_before(&leaf.begin, "native_flare_draw", move |o| {
                let mut mem = o.guest();
                if owns_late_fan(site, |offset| {
                    o.cpu
                        .esp
                        .checked_add(offset)
                        .and_then(|at| mem.u32(at).ok())
                }) {
                    let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let Some(host) = host {
                        host.arm_flare(&mut *mem, site);
                    }
                }
            })
            .context_only()
            .optional();
        b.raw()
            .observe_before(&leaf.end, "native_flare_return", move |o| {
                let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(host) = host {
                    host.finish(&mut *o.guest(), Producer::Flare(site));
                }
            })
            .context_only();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_filters_need_both_return_addresses() {
        let words = |stack: [(u32, u32); 2]| {
            move |at| {
                stack
                    .iter()
                    .find(|(offset, _)| *offset == at)
                    .map(|(_, value)| *value)
            }
        };
        for caller in [0x47523, 0x4752f] {
            assert!(owns_late_fan(
                Site::FilterSource,
                words([(0x90, caller), (0xf0, LATE_RETURN)])
            ));
            assert!(!owns_late_fan(
                Site::FilterSource,
                words([(0x90, caller), (0xf0, 0)])
            ));
        }
        assert!(owns_late_fan(
            Site::Filter,
            words([(0x9c, 0x4753a), (0x100, LATE_RETURN)])
        ));
        assert!(!owns_late_fan(
            Site::Filter,
            words([(0x9c, 0x47523), (0x100, LATE_RETURN)])
        ));
        assert!(!owns_late_fan(Site::Composite, |_| None));
        assert!(reflection_caller(Some(0x2f24c)));
        assert!(!reflection_caller(Some(0x4fddd)));
    }
}
