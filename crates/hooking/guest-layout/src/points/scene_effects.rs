//! Interior sites the native render stages hook. Each must be an entry of
//! the generated image, so both image tiers expose them however the builder
//! was configured (a Steam Machine module is built on another machine).
use super::{
    Point, Sig, OBJECT_RECORD_APPEND, TRANSPARENT_GEOMETRY_ENTRY, TRANSPARENT_GEOMETRY_QUEUED,
    TRANSPARENT_ORDER_BEGIN, TRANSPARENT_ORDER_READY, ui_render::*,
};

const RENDERER: &str = "docs/graphics/native-renderer.md";
/// The merged node's window call (`call 0x00163db0`, returning to
/// `PORTAL_WINDOW_RESUME`), and the site after its return. A return resumes
/// the caller's code at 0x00172f71 (`test al, al`) and never runs a hook
/// there; the site is the `jz` after it, which reads only ZF and falls
/// through. A native window jumps to the resume address, so the test runs.
pub const PORTAL_WINDOW_CALL: Point = Point::new("PORTAL_WINDOW_CALL", 0x0017_2f6c, Sig::Site, RENDERER);
pub const PORTAL_WINDOW_RESUME: u32 = 0x0017_2f71;
pub const PORTAL_WINDOW_READY: Point = Point::new("PORTAL_WINDOW_READY", 0x0017_2f73, Sig::Site, RENDERER);
const PARTICLES: &str = "docs/graphics/native-particles.md";
const DECALS: &str = "docs/graphics/native-decals-research.md";

/// The portal region writer has returned; its frame and scratch tables are
/// intact, and the caller has yet to sort the ordinal table. The writer's
/// call (0x001732f9) returns to 0x001732fe, which a return resumes without
/// a hook; the site is the next instruction (`mov edx, [esp+0xc]`, after
/// the `mov eax, [esp+0x10]` that writes only EAX).
pub const PORTAL_REGIONS_READY: Point = Point::new("PORTAL_REGIONS_READY", 0x0017_3302, Sig::Site, RENDERER);

/// The address the layer build (`0x0003bab0`) returns to: a return resumes
/// the caller there without a hook, so it is the return address the
/// native build compares and returns to, not a site.
pub const LAYER_BUILD_RETURN: u32 = 0x0003_bc4f;

/// Ordered layer entries are complete. EAX holds the count and the caller
/// owns the sort buffer at ESP+8, before the first geometry handler. The
/// site is the `jle` after the build's return (`test eax, eax` at
/// `LAYER_BUILD_RETURN`, which writes only flags).
pub const LAYER_BUILT: Point = Point::new("LAYER_BUILT", 0x0003_bc51, Sig::Site, RENDERER);

/// The entry loop's step after an entry (`0x0003bc94`, the `add esi, 18h`):
/// the instruction after the handler call's return `0x0003bc90` (a call's
/// continuation, where no hook runs) and its counter load, reached by every
/// entry, the passed-over ones included. ESI is still the entry just
/// dispatched; EAX holds the loop's counter (docs/graphics/layer-draw.md).
pub const LAYER_ENTRY_DONE: Point = Point::new("LAYER_ENTRY_DONE", 0x0003_bc94, Sig::Site, RENDERER);

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

/// The sorted loop's per-entry join, after the call to the entry's handler
/// (`0x0001e3f1`, `call ecx`, returning to `0x0001e3f3`) or the skip of a
/// null one (`je 0x0001e3f3`): `esi` is the ordinal just done. `0x0001e3f3`
/// is a call's continuation, where a return resumes the generated code and
/// no hook runs, so the site is `0x0001e3f9`, the next instruction, which
/// both paths fall to.
pub const TRANSPARENT_DISPATCH_NEXT: Point = Point::new("TRANSPARENT_DISPATCH_NEXT", 0x0001_e3f9, Sig::Site, RENDERER);

/// The shared indexed wrapper, just after its state commit. The water
/// composite is identified by the wrapper's own return address.
/// The commit (`call 0x0001cf50` at 0x0001c8a0) returns to 0x0001c8a5,
/// where a return resumes the wrapper's generated code and no hook runs;
/// the site is the next instruction. 0x0001c8a5 (`mov eax, [esp+8]`)
/// writes only EAX and falls through, so `[esp]` is still the wrapper's
/// return address and its three arguments are at `esp+4..+0xc`.
pub const WATER_INDEX_READY: Point = Point::new("WATER_INDEX_READY", 0x0001_c8a9, Sig::Site, RENDERER);
/// The wrapper's tail, past its `DrawIndexedVertices` call at 0x0001c8b4, is
/// `mov al, 1; ret 0xc` (0x0001c8b9). A native draw returns from the
/// wrapper at the site itself, popping this many argument bytes after the
/// return address, so the tail needs no entry or resume of its own.
pub const WATER_INDEX_ARG_BYTES: u32 = 0xc;

pub const HUD_QUAD_READY: Point = Point::new("HUD_QUAD_READY", 0x0022_a7f6, Sig::Site, RENDERER);
/// In the shader quad `0x0022a664`, the call to `0x0001be50` that starts
/// the widget's state setup (`0x0001bd50` twice, the shader begin
/// `0x0001bbf0`, `0x0001cf50`, `0x0001cc30`, `0x0001cd90`, then the draw at
/// [`HUD_QUAD_READY`]): the four vertices are in the frame (`[ebp - 0x70]`)
/// and the bitmap is set (`[0x004e6978]`, `[0x004e697c]`), but no setup
/// has run. ESP is the tail's (`0x0022a854` pops EDI, ESI and EBX and
/// leaves), so a quad drawn here resumes there with nothing to pop
/// (docs/graphics/native-hud-overlay.md, "The material in Rust").
pub const HUD_QUAD_SETUP: Point = Point::new("HUD_QUAD_SETUP", 0x0022_a7b3, Sig::Site, RENDERER);

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
/// The weather layer's End call (0x00038342) is the continuation of its
/// last vertex call (`call 0x003fb2d0` at 0x0003833d), so no hook runs
/// there. BEGIN is that vertex call's first argument push (`push esi`,
/// 0x00038336), reached by falling through from `mov eax, [esp+0xf0]`.
/// The callee pops its four arguments (`ret 0x10`), so ESP there equals
/// ESP at End, and the only code between is the four pushes, `or edi, -1`
/// and the last vertex. Arming drains with the strip still open, as the
/// flares below do.
pub const EFFECT_WEATHER_LAYER: EffectLeaf = leaf(0x0003_8336, 0x0003_8349);
pub const EFFECT_WEATHER_PARTICLES: EffectLeaf = leaf(0x0003_96f7, 0x0003_96f9);
pub const EFFECT_MATERIAL_STRIP: EffectLeaf = leaf(0x0019_fc3e, 0x0019_fc40);

// Immediately before the XDK End call (or, below, before the vertex call
// End is the continuation of), and the first instruction after the one
// End returns to: a return resumes the caller's generated code at
// the continuation and never runs a hook there. Each continuation writes
// no stack slot or register the RETURN hook reads and falls through to
// it: mask `xor edx, edx` (0x0004730b), mask sample `lea eax, [esp+0x24]`
// (0x00047517), filter source `xor eax, eax` (0x000467a0), filter
// `inc ebx` (0x00046e49), composite `dec dword [esp+0x20]` (0x00047859,
// the loop count, before its `jnz`). The shared reflection billboard
// already has a pair above.
//
// The mask sample's and the composite's End calls (0x00047512, 0x00047854)
// are themselves the continuations of the last vertex call (`call
// 0x003fb2d0`, `ret 0x10`), so a hook there never runs (`hooks::verify`
// refuses it). Their BEGIN is that vertex call's first argument push,
// reached by falling through: `push 0x4b7fffff` at 0x00047501 (after `mov
// edx, [esp+0x40]`) and at 0x00047843 (after `mov eax, [esp+0x1c]`). The
// callee pops its pushes, so ESP there equals ESP at End and the
// late-pass return word at ESP+0x54 reads the same. Only the pushes and
// the last vertex lie between; arming drains with the fan still open.
const fn flare_leaf(begin: u32, end: u32) -> EffectLeaf {
    EffectLeaf {
        begin: Point::new("FLARE_DRAW_END", begin, Sig::Site, RENDERER),
        end: Point::new("FLARE_DRAW_RETURN", end, Sig::Site, RENDERER),
    }
}
pub const FLARE_MASK: EffectLeaf = flare_leaf(0x0004_7306, 0x0004_730d);
pub const FLARE_MASK_SAMPLE: EffectLeaf = flare_leaf(0x0004_7501, 0x0004_751b);
pub const FLARE_FILTER_SOURCE: EffectLeaf = flare_leaf(0x0004_679b, 0x0004_67a2);
pub const FLARE_FILTER: EffectLeaf = flare_leaf(0x0004_6e44, 0x0004_6e4a);
pub const FLARE_COMPOSITE: EffectLeaf = flare_leaf(0x0004_7843, 0x0004_785d);

pub const EFFECT_QUERY: Point = Point::new("EFFECT_QUERY", 0x0002_e9b6, Sig::Site, PARTICLES);
pub const EFFECT_LAYERED_A: Point = Point::new("EFFECT_LAYERED_A", 0x0004_1cc0, Sig::Site, PARTICLES);
pub const EFFECT_LAYERED_B: Point = Point::new("EFFECT_LAYERED_B", 0x0004_23c0, Sig::Site, PARTICLES);

/// Every immediate effect and flare leaf. Both of each leaf's sites are
/// hooked as observers, so neither may be a call's continuation
/// (`crates/rewrites/engine/tests/point_shapes.rs` checks them against the
/// image).
pub static EFFECT_LEAVES: [&EffectLeaf; 10] = [
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

/// Every site above, the scene list sites and the UI walk's interior
/// sites, each once.
pub fn instruction_entries() -> impl Iterator<Item = u32> {
    [
        PORTAL_REGIONS_READY.base,
        LAYER_BUILT.base,
        LAYER_ENTRY_DONE.base,
        DECAL_RENDER_END.base,
        WORLD_SECTION_END.base,
        GEOMETRY_RANGE_READY.base,
        SCENE_TRANSPARENTS_END.base,
        TRANSPARENT_DISPATCH_NEXT.base,
        WATER_INDEX_READY.base,
        PORTAL_WINDOW_CALL.base,
        PORTAL_WINDOW_RESUME,
        PORTAL_WINDOW_READY.base,
        HUD_QUAD_READY.base,
        HUD_QUAD_SETUP.base,
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
    .chain(EFFECT_LEAVES.into_iter().flat_map(|l| [l.begin.base, l.end.base]))
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
        // The water wrapper's tail is not an entry: a native draw returns
        // from the wrapper at the site.
        assert!(seen.contains(&super::WATER_INDEX_READY.base));
        assert!(!seen.contains(&0x0001_c8b9));
        // The sorted loop's join is the instruction after the handler's
        // return, never the return itself.
        assert!(seen.contains(&super::TRANSPARENT_DISPATCH_NEXT.base));
        assert_eq!(super::TRANSPARENT_DISPATCH_NEXT.base, 0x0001_e3f9);
        assert!(!seen.contains(&0x0001_e3f3));
        assert_eq!(super::WATER_INDEX_ARG_BYTES, 0xc);
        // Every leaf's two sites are entries.
        for leaf in super::EFFECT_LEAVES {
            assert!(seen.contains(&leaf.begin.base), "{leaf:?}");
            assert!(seen.contains(&leaf.end.base), "{leaf:?}");
        }
    }

    /// The three End calls that are the continuation of the last vertex
    /// call are not BEGIN sites: a hook there never runs. Their BEGIN is
    /// the vertex call's first push, before End and in the same function.
    #[test]
    fn begin_is_never_an_end_call_that_follows_a_call() {
        for (leaf, end_call, begin) in [
            (&super::EFFECT_WEATHER_LAYER, 0x0003_8342, 0x0003_8336),
            (&super::FLARE_MASK_SAMPLE, 0x0004_7512, 0x0004_7501),
            (&super::FLARE_COMPOSITE, 0x0004_7854, 0x0004_7843),
        ] {
            assert_eq!(leaf.begin.base, begin, "{leaf:?}");
            assert!(leaf.begin.base < end_call && end_call < leaf.end.base, "{leaf:?}");
        }
        let entries: std::collections::HashSet<u32> = super::instruction_entries().collect();
        for end_call in [0x0003_8342, 0x0004_7512, 0x0004_7854] {
            assert!(!entries.contains(&end_call), "{end_call:#010x}");
        }
    }
}
