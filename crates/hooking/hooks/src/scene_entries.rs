//! The retail instruction boundaries the native render stages hook
//! mid-function, which the emitter must route through entry dispatch.

/// Retail instruction boundaries that the emitter must route through entry
/// dispatch. Hook bodies keep working on images with only a subset present.
#[cfg(not(feature = "native-render"))]
pub fn scene_effect_instruction_entries() -> impl Iterator<Item = u32> {
    std::iter::empty()
}

/// Retail instruction boundaries that the emitter must route through entry
/// dispatch. Hook bodies keep working on images with only a subset present.
#[cfg(feature = "native-render")]
pub fn scene_effect_instruction_entries() -> impl Iterator<Item = u32> {
    let mut seen = std::collections::HashSet::new();
    [
        crate::native_decals::DECAL_ITEM_GEOMETRY.base,
        crate::native_decals::DECAL_RENDER_END.base,
        crate::native_transparents::GEOMETRY_RANGE_READY.base,
        crate::native_transparents::SCENE_TRANSPARENTS_END.base,
        crate::native_hud::HUD_QUAD_READY.base,
        crate::native_post::SCREEN_STRIP_READY.base,
        crate::native_particles::EMIT_READY.base,
        crate::native_particles::EMIT_PUBLISH.base,
        crate::native_particles::EMIT_END.base,
        crate::native_particles::DYNAMIC_QUAD_READY.base,
        crate::native_particles::DYNAMIC_PUBLISH.base,
        crate::native_particles::DYNAMIC_END.base,
    ].into_iter().chain(crate::native_effects::instruction_entries())
        .chain(crate::native_scene_lists::instruction_entries())
        .chain(crate::native_hud::instruction_entries())
        .filter(move |&address| seen.insert(address))
}
