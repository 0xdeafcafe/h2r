//! Superbounce's Pogo Stick (docs/game/pogo-stick.md): the feature.
//!
//! **When it is on.** The feature is on by default and `H2_FEATURES=-pogo`
//! leaves it out. Installed, it acts only in a game whose rules script is
//! Superbounce's with the `pogo` option on ([`crate::superbounce_options`]),
//! on a map the kit's rewrite added the sticks to (`pogo_kit::placed`).
//!
//! **What it holds.** The loadout ([`crate::pogo_loadout`]): the stick as
//! the secondary weapon at every spawn. That hook changes the simulation,
//! so the feature is not a presentation one: a film's hook digest names
//! it, and every machine in a game must run it.
//!
//! **What the player sees** is a second feature, [`PogoView`]
//! (`pogo-view`), a presentation one: the chase camera while the stick
//! is held ([`crate::pogo_camera`]), the riding pose and the tricks
//! ([`crate::pogo_pose_glue`]). With `pogo` off no stick is ever
//! held, so it does nothing; `H2_FEATURES=-pogo-view` alone leaves the
//! stick in the game, carried and seen in first person. The riders
//! themselves ([`crate::pogo_ride`]) are stepped by the superbounce mode.

/// The feature's name.
pub const FEATURE: &str = "pogo";

/// The feature.
pub struct Pogo;

impl hooks::Feature for Pogo {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Superbounce's Pogo Stick").doc("docs/game/pogo-stick.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // The loadout decides from guest RAM (the variant's block and the
        // script's option bits) and from the tags the map's bytes place,
        // which a boot fixes: a snapshot needs nothing from us.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        crate::pogo_loadout::install_hooks(b)?;
        Ok(())
    }
}

/// The presentation feature's name.
pub const VIEW_FEATURE: &str = "pogo-view";

/// What the player sees of the stick: the chase camera.
pub struct PogoView;

impl hooks::Feature for PogoView {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            VIEW_FEATURE,
            "The Pogo Stick's chase camera and riding pose",
        )
        .doc("docs/game/pogo-stick.md")
        // Render hooks on private views only: a recorded film's hook
        // digest does not include it.
        .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // The riders and the camera's boom restart from the next tick
        // after a load.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        crate::pogo_camera::install_hooks(b)?;
        // The riders' rows posed for the draw and put back after it.
        crate::pogo_pose_glue::install_hooks(b)?;
        Ok(())
    }
}

/// Adds both features to `registry`.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry
        .add(&Pogo)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("pogo: {e}"))?;
    registry
        .add(&PogoView)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("pogo-view: {e}"))
}
