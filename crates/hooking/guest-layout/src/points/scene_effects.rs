//! Interior sites the native render stages hook. Each must be an entry of
//! the generated image, so both image tiers expose them however the builder
//! was configured (a Steam Machine module is built on another machine).
use super::{
    Point, Sig, OBJECT_RECORD_APPEND, TRANSPARENT_GEOMETRY_ENTRY, TRANSPARENT_GEOMETRY_QUEUED,
    TRANSPARENT_ORDER_BEGIN, TRANSPARENT_ORDER_READY, ui_render::*,
};

const RENDERER: &str = "docs/graphics/native-renderer.md";
/// The merged node's window call and the first instruction that uses AL.
pub const PORTAL_WINDOW_CALL: Point = Point::new("PORTAL_WINDOW_CALL", 0x0017_2f6c, Sig::Site, RENDERER);
pub const PORTAL_WINDOW_READY: Point = Point::new("PORTAL_WINDOW_READY", 0x0017_2f71, Sig::Site, RENDERER);
const PARTICLES: &str = "docs/graphics/native-particles.md";
const DECALS: &str = "docs/graphics/native-decals-research.md";

/// The portal region writer has returned; its frame and scratch tables are
/// intact, and the caller has yet to sort the ordinal table.
pub const PORTAL_REGIONS_READY: Point = Point::new("PORTAL_REGIONS_READY", 0x0017_32fe, Sig::Site, RENDERER);

/// Ordered layer entries are complete. EAX holds the count and the caller
/// owns the sort buffer at ESP+8, before the first geometry handler.
pub const LAYER_BUILT: Point = Point::new("LAYER_BUILT", 0x0003_bc4f, Sig::Site, RENDERER);

/// End of one complete BSP material walk, before the record cursor advances.
pub const WORLD_SECTION_END: Point = Point::new("WORLD_SECTION_END", 0x0004_4c6c, Sig::Site, RENDERER);

pub const DECAL_RENDER_END: Point = Point::new("DECAL_RENDER_END", 0x0004_41aa, Sig::Site, DECALS);

// The contiguous-range branch of the standard geometry emitter, after
// state commit. EDI holds the index array, ESI its count, BL its topology.
pub const GEOMETRY_RANGE_READY: Point = Point::new("SCENE_GEOMETRY_RANGE_READY", 0x0004_dad0, Sig::Site, RENDERER);
/// Subpart branches of the geometry emitter, at DrawIndexedVertices
/// after the texture flush and the three argument pushes. Each successful
/// replacement continues immediately after its own call.
pub const SUBPART_STRIP_DRAW: Point = Point::new("SUBPART_STRIP_DRAW", 0x0004_db40, Sig::Site, RENDERER);
pub const SUBPART_LIST_DRAW: Point = Point::new("SUBPART_LIST_DRAW", 0x0004_dbdc, Sig::Site, RENDERER);
pub const SUBPART_LIST_LAST: Point = Point::new("SUBPART_LIST_LAST", 0x0004_dc3a, Sig::Site, RENDERER);
pub const SUBPART_DRAWS: [(&Point, u32); 3] = [
    (&SUBPART_STRIP_DRAW, 0x0004_db45),
    (&SUBPART_LIST_DRAW, 0x0004_dbe1),
    (&SUBPART_LIST_LAST, 0x0004_dc3f),
];
// All exits from the sorted loop join here, before the guest view cleanup.
pub const SCENE_TRANSPARENTS_END: Point = Point::new("SCENE_TRANSPARENTS_END", 0x0001_e406, Sig::Site, RENDERER);

/// The shared indexed wrapper, just after its state commit. The water
/// composite is identified by the wrapper's own return address.
/// The commit (`call 0x0001cf50` at 0x0001c8a0) returns to 0x0001c8a5,
/// where a return resumes the wrapper's generated code and no hook runs;
/// the site is the next instruction. 0x0001c8a5 (`mov eax, [esp+8]`)
/// writes only EAX and falls through, so `[esp]` is still the wrapper's
/// return address and its three arguments are at `esp+4..+0xc`.
pub const WATER_INDEX_READY: Point = Point::new("WATER_INDEX_READY", 0x0001_c8a9, Sig::Site, RENDERER);
pub const WATER_INDEX_END: u32 = 0x0001_c8b9;

pub const HUD_QUAD_READY: Point = Point::new("HUD_QUAD_READY", 0x0022_a7f6, Sig::Site, RENDERER);

/// The screen strip's BEGIN is published and its current row is known.
pub const SCREEN_STRIP_READY: Point = Point::new("SCREEN_STRIP_READY", 0x0002_d565, Sig::Site, RENDERER);

// After the allocator succeeds and the caller's preserved ESI/EDI have
// been popped. ESP is still one word below the emitter's entry frame.
pub const PARTICLE_EMIT_READY: Point = Point::new("PARTICLE_EMIT_READY", 0x0023_ed5c, Sig::Site, PARTICLES);
// Publication boundaries, observed before the indicated instruction. These
// keep every hook context-only: no Rust continuation around a guest body.
pub const PARTICLE_PUBLISH: Point = Point::new("PARTICLE_PUBLISH", 0x0023_ef77, Sig::Site, PARTICLES);
pub const PARTICLE_END: Point = Point::new("PARTICLE_END", 0x0023_ef79, Sig::Site, PARTICLES);
// Shared with the UI rectangle hook. The planner keys points by identity.
pub static EFFECT_QUAD_READY: Point = Point::new("EFFECT_QUAD_READY", 0x0004_b252, Sig::Site, PARTICLES);
pub const EFFECT_PUBLISH: Point = Point::new("EFFECT_PUBLISH", 0x0004_b28c, Sig::Site, PARTICLES);
pub const EFFECT_END: Point = Point::new("EFFECT_END", 0x0004_b2a3, Sig::Site, PARTICLES);

/// One researched immediate-mode effect leaf: its BEGIN and its END.
#[derive(Debug)]
pub struct EffectLeaf {
    pub begin: Point,
    pub end: Point,
}

const fn leaf(begin: u32, end: u32) -> EffectLeaf {
    EffectLeaf {
        begin: Point::new("EFFECT_IMMEDIATE_BEGIN", begin, Sig::Site, PARTICLES),
        end: Point::new("EFFECT_IMMEDIATE_END", end, Sig::Site, PARTICLES),
    }
}

pub const EFFECT_BILLBOARD: EffectLeaf = leaf(0x0004_8497, 0x0004_849d);
pub const EFFECT_DEPTH_BILLBOARD: EffectLeaf = leaf(0x0004_8ae5, 0x0004_8aeb);
// Its BEGIN (the End call) is the continuation of the last
// `SetVertexData4f` call, so it never runs, as with two flare leaves below.
pub const EFFECT_WEATHER_LAYER: EffectLeaf = leaf(0x0003_8342, 0x0003_8349);
pub const EFFECT_WEATHER_PARTICLES: EffectLeaf = leaf(0x0003_96f7, 0x0003_96f9);
pub const EFFECT_MATERIAL_STRIP: EffectLeaf = leaf(0x0019_fc3e, 0x0019_fc40);

// Immediately before the XDK End call, and the first instruction after
// the one End returns to: a return resumes the caller's generated code at
// the continuation and never runs a hook there. Each continuation writes
// no stack slot or register the RETURN hook reads and falls through to
// it: mask `xor edx, edx` (0x0004730b), mask sample `lea eax, [esp+0x24]`
// (0x00047517), filter source `xor eax, eax` (0x000467a0), filter
// `inc ebx` (0x00046e49), composite `dec dword [esp+0x20]` (0x00047859,
// the loop count, before its `jnz`). The shared reflection billboard
// already has a pair above.
//
// The mask sample's and the composite's End calls are themselves the
// continuations of the last `SetVertexData4f` call, so their BEGIN never
// runs: no instruction lies between that return and End. The plan
// refuses those hooks at install (`hooks::verify`).
const fn flare_leaf(begin: u32, end: u32) -> EffectLeaf {
    EffectLeaf {
        begin: Point::new("FLARE_DRAW_END", begin, Sig::Site, RENDERER),
        end: Point::new("FLARE_DRAW_RETURN", end, Sig::Site, RENDERER),
    }
}
pub const FLARE_MASK: EffectLeaf = flare_leaf(0x0004_7306, 0x0004_730d);
pub const FLARE_MASK_SAMPLE: EffectLeaf = flare_leaf(0x0004_7512, 0x0004_751b);
pub const FLARE_FILTER_SOURCE: EffectLeaf = flare_leaf(0x0004_679b, 0x0004_67a2);
pub const FLARE_FILTER: EffectLeaf = flare_leaf(0x0004_6e44, 0x0004_6e4a);
pub const FLARE_COMPOSITE: EffectLeaf = flare_leaf(0x0004_7854, 0x0004_785d);

pub const EFFECT_QUERY: Point = Point::new("EFFECT_QUERY", 0x0002_e9b6, Sig::Site, PARTICLES);
pub const EFFECT_LAYERED_A: Point = Point::new("EFFECT_LAYERED_A", 0x0004_1cc0, Sig::Site, PARTICLES);
pub const EFFECT_LAYERED_B: Point = Point::new("EFFECT_LAYERED_B", 0x0004_23c0, Sig::Site, PARTICLES);

/// Every site above, the scene list sites and the UI walk's interior
/// sites, each once.
pub fn instruction_entries() -> impl Iterator<Item = u32> {
    let leaves = [
        &EFFECT_BILLBOARD,
        &EFFECT_DEPTH_BILLBOARD,
        &EFFECT_WEATHER_LAYER,
        &EFFECT_WEATHER_PARTICLES,
        &EFFECT_MATERIAL_STRIP,
        &FLARE_MASK,
        &FLARE_MASK_SAMPLE,
        &FLARE_FILTER_SOURCE,
        &FLARE_FILTER,
        &FLARE_COMPOSITE,
    ];
    [
        PORTAL_REGIONS_READY.base,
        LAYER_BUILT.base,
        DECAL_RENDER_END.base,
        WORLD_SECTION_END.base,
        GEOMETRY_RANGE_READY.base,
        SCENE_TRANSPARENTS_END.base,
        WATER_INDEX_READY.base,
        WATER_INDEX_END,
        PORTAL_WINDOW_CALL.base,
        PORTAL_WINDOW_READY.base,
        HUD_QUAD_READY.base,
        SCREEN_STRIP_READY.base,
        PARTICLE_EMIT_READY.base,
        PARTICLE_PUBLISH.base,
        PARTICLE_END.base,
        EFFECT_QUAD_READY.base,
        EFFECT_PUBLISH.base,
        EFFECT_END.base,
        EFFECT_QUERY.base,
        EFFECT_LAYERED_A.base,
        EFFECT_LAYERED_B.base,
        OBJECT_RECORD_APPEND.base,
        TRANSPARENT_GEOMETRY_ENTRY.base,
        TRANSPARENT_GEOMETRY_QUEUED.base,
        TRANSPARENT_ORDER_BEGIN.base,
        TRANSPARENT_ORDER_READY.base,
        LAYERS_RETURN.base,
        ITEM_CALL.base,
        ITEM_RETURN.base,
        LIST_READY.base,
        LIST_DONE.base,
        HUD_RETURN.base,
        HUD_HIDDEN_RETURN.base,
        LOADING_RETURN.base,
        LOADING_EMPTY_RETURN.base,
        SCREEN_QUAD_READY.base,
        FADE_FAN_READY.base,
        TAIL_COPY.base,
        TAIL_COPY_END.base,
        TAIL_COMPOSITE.base,
        TAIL_COMPOSITE_END.base,
        FADE_RETURN.base,
        OVERLAY_RECT_RETURN.base,
        LOADING_SPINNER_READY.base,
        LOADING_BITMAP_READY.base,
        OUTLINE_VERTICES.base,
        OUTLINE_VERTICES_TAIL,
        BACKDROP_WASH.base,
        BACKDROP_TINT.base,
        BACKDROP_COMPOSITE.base,
        MODEL_VIEW_CALL.base,
        MODEL_VIEW_RETURN.base,
        MODEL_GEOMETRY_CALL.base,
    ]
    .into_iter()
    .chain(SUBPART_DRAWS.into_iter().flat_map(|(point, resume)| [point.base, resume]))
    .chain(leaves.into_iter().flat_map(|l| [l.begin.base, l.end.base]))
}

#[cfg(test)]
mod tests {
    #[test]
    fn instruction_entries_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for address in super::instruction_entries() {
            assert!(seen.insert(address), "duplicate entry {address:#010x}");
        }
        assert!(seen.contains(&super::EFFECT_QUAD_READY.base));
        assert!(seen.contains(&super::DECAL_RENDER_END.base));
    }
}
