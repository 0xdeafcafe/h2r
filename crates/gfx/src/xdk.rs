//! Per-XDK-build render-state numbering.
//!
//! The D3D runtime keeps render state in a flat array of 32-bit words and the
//! *position* of each state depends on the library build: Microsoft added
//! states over time (and removed one), always keeping the three groups
//! ("pixel shader", "simple", "deferred", "complex") in order. Code that pokes
//! the array directly (inlined `SetRenderState` fast paths, which the LTCG
//! build of D3D8 produces heavily) therefore uses build-specific indices.
//!
//! Cxbx-Reloaded handles this with one canonical table of every state it
//! knows (`X_D3DRS_*`, `XbD3D8Types.h`), each annotated with the first build
//! that has it (and, for `MULTISAMPLETYPE`, the build that dropped it;
//! `DxbxRenderStateInfo` in `XbConvert.cpp`), and builds the per-build index by
//! counting the states present (`XboxRenderStateConverter::
//! BuildRenderStateMappingTable`, `RenderStates.cpp`), both at commit
//! `585c49a50af1255ab155099e06f24505f9c5a800`. This module implements the same
//! idea from that description: [`RenderStateTable::for_build`].
//!
//! Terminology: a *canonical* index is Cxbx's `X_D3DRS_*` value, which is what
//! [`crate::state::RenderStateType::index`] returns. A *guest* index is the
//! slot in the title's own `D3D__RenderState` array.
//!
//! Halo 2 links build 5849 throughout (from the symbol pass), the default
//! here. For every build from 4034 up the only difference from the canonical
//! numbering is that states after `MULTISAMPLETYPE` (canonical 154) sit one
//! slot lower, so all states [`crate::state::RenderState`] models (canonical
//! 57..=148) have identical guest and canonical indices on 5849.

/// Identifies an XDK build (the D3D8 library build number, e.g. 5849).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct XdkBuild(pub u32);

/// The build every library in Halo 2 was linked with.
pub const HALO2_BUILD: XdkBuild = XdkBuild(5849);

impl Default for XdkBuild {
    fn default() -> Self {
        HALO2_BUILD
    }
}

/// Highest canonical render-state index (`X_D3DRS_DONOTCULLUNCOMPRESSED`).
pub const CANONICAL_LAST: u32 = 166;

/// First build introducing a canonical state, and the build that removed it.
fn availability(canonical: u32) -> (u32, Option<u32>) {
    match canonical {
        82 => (4432, None),
        83..=91 | 127..=135 | 159 => (4627, None),
        126 | 155 | 156 => (4034, None),
        154 => (3424, Some(4034)),
        161..=166 => (3911, None),
        _ => (3424, None),
    }
}

/// Mapping between canonical and guest render-state indices for one build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderStateTable {
    build: XdkBuild,
    /// `guest_of[canonical]`.
    guest_of: Vec<Option<u32>>,
    /// `canonical_of[guest]`.
    canonical_of: Vec<u32>,
}

impl RenderStateTable {
    /// Builds the table for `build`.
    pub fn for_build(build: XdkBuild) -> Self {
        let mut guest_of = Vec::with_capacity(CANONICAL_LAST as usize + 1);
        let mut canonical_of = Vec::new();
        for canonical in 0..=CANONICAL_LAST {
            let (since, removed) = availability(canonical);
            let present = build.0 >= since && removed.is_none_or(|r| build.0 < r);
            if present {
                guest_of.push(Some(canonical_of.len() as u32));
                canonical_of.push(canonical);
            } else {
                guest_of.push(None);
            }
        }
        Self { build, guest_of, canonical_of }
    }

    /// The build this table describes.
    pub fn build(&self) -> XdkBuild {
        self.build
    }

    /// Number of render-state slots in the guest array.
    pub fn len(&self) -> usize {
        self.canonical_of.len()
    }

    /// Whether the table is empty (never true).
    pub fn is_empty(&self) -> bool {
        self.canonical_of.is_empty()
    }

    /// Guest slot of a canonical state, `None` if this build lacks it.
    pub fn canonical_to_guest(&self, canonical: u32) -> Option<u32> {
        self.guest_of.get(canonical as usize).copied().flatten()
    }

    /// Canonical state of a guest slot, `None` if out of range.
    pub fn guest_to_canonical(&self, guest: u32) -> Option<u32> {
        self.canonical_of.get(guest as usize).copied()
    }
}

impl Default for RenderStateTable {
    fn default() -> Self {
        Self::for_build(HALO2_BUILD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo2_build_is_identity_below_multisample_type() {
        let t = RenderStateTable::default();
        assert_eq!(t.build(), XdkBuild(5849));
        for c in 0..154 {
            assert_eq!(t.canonical_to_guest(c), Some(c), "canonical {c}");
        }
        assert_eq!(t.canonical_to_guest(154), None); // MULTISAMPLETYPE removed
        assert_eq!(t.canonical_to_guest(155), Some(154)); // MULTISAMPLEMODE
        assert_eq!(t.canonical_to_guest(166), Some(165));
        assert_eq!(t.len(), 166);
    }

    #[test]
    fn modelled_states_have_same_index_on_5849_and_5933() {
        let a = RenderStateTable::for_build(XdkBuild(5849));
        let b = RenderStateTable::for_build(XdkBuild(5933));
        assert_eq!(a.guest_of, b.guest_of);
    }

    #[test]
    fn older_builds_shift_states() {
        // 4432 predates the 4627 additions (nine "simple" and nine
        // "deferred" slots, plus SAMPLEALPHA); DEPTHCLIPCONTROL is present.
        let t = RenderStateTable::for_build(XdkBuild(4432));
        assert_eq!(t.canonical_to_guest(82), Some(82));
        assert_eq!(t.canonical_to_guest(83), None);
        assert_eq!(t.canonical_to_guest(92), Some(83)); // FOGENABLE
        // CULLMODE: 9 simple + 9 deferred unused slots absent below it.
        assert_eq!(t.canonical_to_guest(147), Some(147 - 18));
        // 3424 additionally lacks DEPTHCLIPCONTROL and SWAPFILTER, but still
        // has MULTISAMPLETYPE (absent below it: 1 + 9 + 1 + 9 slots).
        let t = RenderStateTable::for_build(XdkBuild(3424));
        assert_eq!(t.canonical_to_guest(82), None);
        assert_eq!(t.canonical_to_guest(154), Some(154 - 20));
    }

    #[test]
    fn mapping_roundtrips() {
        for build in [3424, 3911, 4034, 4432, 4627, 5849, 5933] {
            let t = RenderStateTable::for_build(XdkBuild(build));
            for g in 0..t.len() as u32 {
                let c = t.guest_to_canonical(g).unwrap();
                assert_eq!(t.canonical_to_guest(c), Some(g));
            }
            assert_eq!(t.guest_to_canonical(t.len() as u32), None);
        }
    }
}
