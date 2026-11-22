//! Additional entry boundaries shared by fast and Clang emission.
//!
//! The retail image's hooked render sites must be instruction boundaries
//! in both tiers, so discovery exposes them before lifting. Which sites
//! those are is the hook layer's knowledge (`guest_layout::points`), not
//! the image builder's: the composition root registers them once
//! ([`register_retail_entries`], `boot::register_image_entries`) before
//! anything builds, loads or analyses the retail image. Analysing the
//! retail image with nothing registered is refused loudly, never built
//! without them, and the build stamps name the registered list
//! ([`entries_hash`]), so a module built with other entries is not current.

use std::sync::OnceLock;

static RETAIL_ENTRIES: OnceLock<Box<[u32]>> = OnceLock::new();

/// Whether named instruction sites in the base retail image apply. Other
/// titles and title updates keep their own discovered instruction boundaries.
pub fn has_retail_render_sites(x: &xbe::Xbe) -> bool {
    x.certificate.title_id == 0x4d53_0064 && x.entry == 0x002d_0aee
}

/// Register the retail image's extra instruction entries, in the order
/// discovery exposes them. The first list registered stays; registering it
/// again is a no-op, and registering a different list panics, since
/// modules built from either would disagree.
pub fn register_retail_entries(entries: impl IntoIterator<Item = u32>) {
    let list: Box<[u32]> = entries.into_iter().collect();
    let held = RETAIL_ENTRIES.get_or_init(|| list.clone());
    assert!(
        *held == list,
        "recomp: a second, different list of retail instruction entries was registered"
    );
}

/// The registered retail entries, if any are.
pub fn retail_entries() -> Option<&'static [u32]> {
    RETAIL_ENTRIES.get().map(|e| &e[..])
}

/// FNV-1a of the registered entries (none: of nothing), for the build
/// stamps.
pub fn entries_hash() -> u64 {
    let bytes: Vec<u8> = retail_entries()
        .unwrap_or_default()
        .iter()
        .flat_map(|e| e.to_le_bytes())
        .collect();
    crate::fallback::fnv64(&bytes)
}

pub(super) fn expose(x: &xbe::Xbe, analysis: &mut analyze::Analysis) {
    if has_retail_render_sites(x) {
        let entries = retail_entries().expect(
            "recomp: the retail image's render entries are not registered; the composition root calls \
             boot::register_image_entries (recomp::register_retail_entries) before building, loading or \
             analysing it",
        );
        analysis.expose_instructions(entries.iter().copied());
    }
}
