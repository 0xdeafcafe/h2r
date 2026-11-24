//! Install the sound pass for playback and clock-only sessions. The guest
//! records carry all persistent state across mixer callbacks and checkpoints.

use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

pub const SOUND: &str = "sim.native.sound";
const ID: &str = "impl.sim.native.sound";

pub struct SimSound;

impl Feature for SimSound {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native sound simulation").doc("docs/engine/sim-rewrite.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        if b.switches().implementation(SOUND, false) {
            crate::sim_objects::install_table(
                b,
                &engine::sim_sound::TABLE,
                SOUND,
                engine::sim_sound::generated_sites,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn switch_installs_each_sound_entry_and_its_callback_returns() {
        for (spec, enabled) in [
            ("", false),
            ("-impl.sim.native", false),
            ("impl.sim.native", true),
            ("impl.sim.native,-impl.sim.native.sound", false),
            ("-impl.sim.native,impl.sim.native.sound", true),
        ] {
            let mut registry = Registry::new(Switches::parse(spec).unwrap());
            registry.add(&SimSound).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert!(plan.save_sections().is_empty());
            let table = &*engine::sim_sound::TABLE;
            assert_eq!(plan.chains.len(), if enabled { table.bodies().len() } else { 0 });
            if enabled {
                for body in table.bodies() {
                    let hook = plan.chains[&body.point.base].implement.as_ref().unwrap();
                    assert_eq!(hook.label, SOUND);
                    assert_eq!(hook.continuation_sites(), (body.sites, engine::sim_sound::generated_sites(body.point.base)));
                }
            }
        }
    }

    #[test]
    fn callbacks_must_return_to_a_supported_float_environment() {
        let table = &*engine::sim_sound::TABLE;
        let mut cpu = runtime::CpuAbi::default();
        cpu.mxcsr = 0x1f80;
        cpu.x87.control = 0x37f;
        cpu.x87.status = 0;
        assert!(table.supports(&cpu));
        for rc in [0x400, 0x800, 0xc00] {
            cpu.x87.control = 0x37f | rc;
            assert!(!table.supports(&cpu));
        }
    }
}
