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

/// Adds the feature to `registry`.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry
        .add(&Pogo)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("pogo: {e}"))
}
