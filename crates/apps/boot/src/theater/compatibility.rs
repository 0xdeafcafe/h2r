//! Compatibility is a guest contract. Host UI, codecs, dependency versions and
//! snapshot serializers have their own versioning and do not identify a match.
use super::{Mode, Session};
use anyhow::{Result, ensure};
use engine::points::{self, Loc, Sig};
use std::collections::BTreeSet;

/// Bump for a backwards-incompatible change in scheduler, input or HLE
/// semantics that is not already identified by the image or installed hooks.
pub(super) const SIMULATION_ABI: &str = "h2-sim/1";

fn location(loc: Loc) -> [u8; 2] {
    match loc {
        Loc::Eax => [1, 0],
        Loc::Al => [2, 0],
        Loc::Ecx => [3, 0],
        Loc::Edx => [4, 0],
        Loc::Ebx => [5, 0],
        Loc::Esi => [6, 0],
        Loc::Edi => [7, 0],
        Loc::Xmm0 => [8, 0],
        Loc::Stack(index) => [9, index],
    }
}

fn signature(sig: Sig) -> Vec<u8> {
    // Preserve the existing encodings of function entries (at least four
    // bytes) and opaque points (empty). A site resumes within a function.
    let (args, ret, pop) = match sig {
        Sig::Opaque => return Vec::new(),
        Sig::Site => return vec![1],
        Sig::Fn { args, ret, pop } => (args, ret, pop),
    };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&pop.to_le_bytes());
    bytes.extend_from_slice(&ret.map_or([0, 0], location));
    for &(_, loc) in args {
        bytes.extend_from_slice(&location(loc));
    }
    bytes
}

fn hook_digest(plan: &hooks::Plan, installed: &hooks::Installed) -> String {
    let placed = installed.placed.iter().copied().collect::<BTreeSet<_>>();
    let mut bytes = Vec::new();
    for global in [
        &points::GAME_ENGINE,
        &points::GAME_TIME,
        &points::PLAYERS,
        &points::OBJECTS,
    ] {
        bytes.extend_from_slice(&global.base.to_le_bytes());
    }
    // Chains iterate in address order. Preserve run order within each kind.
    // Labels, docs and pointers to native closures are not part of the ABI.
    for chain in plan.chains.values().filter(|c| placed.contains(&c.addr)) {
        for hook in chain
            .hooks()
            .filter(|h| installed.off_reason(h.feature).is_none())
        {
            let row = (
                "hook",
                chain.addr,
                chain.shadow.filter(|addr| placed.contains(addr)),
                signature(chain.point.sig),
                hook.feature,
                hook.kind().as_str(),
                hook.context_only,
            );
            serde_json::to_writer(&mut bytes, &row).expect("fixed primitive hook signature");
            bytes.push(b'\n');
            let (native, generated) = hook.continuation_sites();
            if chain.addr == chain.point.base
                && hook.kind() == hooks::Kind::Implement
                && !(native.is_empty() && generated.is_empty())
            {
                let row = (
                    "continuations",
                    native.iter().collect::<BTreeSet<_>>(),
                    generated.iter().collect::<BTreeSet<_>>(),
                );
                serde_json::to_writer(&mut bytes, &row).expect("fixed hook continuation addresses");
                bytes.push(b'\n');
            }
        }
    }
    for entry in plan.entries.iter().filter(|e| placed.contains(&e.addr)) {
        serde_json::to_writer(&mut bytes, &("entry", entry.addr, entry.feature))
            .expect("fixed guest entry signature");
        bytes.push(b'\n');
    }
    for alloc in plan
        .allocs
        .iter()
        .filter(|a| installed.off_reason(a.feature).is_none())
    {
        let row = (
            "allocation",
            alloc.addr,
            alloc.len,
            alloc.perms.bits(),
            alloc.feature,
        );
        serde_json::to_writer(&mut bytes, &row).expect("fixed guest allocation signature");
        bytes.push(b'\n');
    }
    film::xbe_digest(&bytes)
}

impl Session {
    pub(crate) fn check_simulation_hooks(
        &self,
        plan: &hooks::Plan,
        installed: &hooks::Installed,
    ) -> Result<()> {
        let mut state = self.lock();
        if state.header.version < 3 {
            return Ok(());
        }
        let actual = hook_digest(plan, installed);
        if matches!(&state.mode, Mode::Record(_)) {
            state.header.simulation_hooks = Some(actual);
        } else {
            ensure!(
                state.header.simulation_hooks.as_ref() == Some(&actual),
                "installed simulation hooks differ from the film"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Observer(&'static str);

    impl hooks::Feature for Observer {
        fn manifest(&self) -> hooks::Manifest {
            hooks::Manifest::new("variant-test", "Simulation fixture")
        }

        fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
            b.frame().on_tick(self.0, |_| {});
            Ok(())
        }
    }

    #[test]
    fn hook_contract_tracks_placed_guest_dispatch_and_ignores_diagnostic_labels() {
        let plan = |label| {
            let mut registry = hooks::Registry::new(hooks::Switches::defaults());
            registry.add(&Observer(label)).unwrap();
            registry.build(&hooks::Base).unwrap()
        };
        let original = plan("sample");
        let renamed = plan("renamed sample");
        let mut installed = hooks::Installed::default();
        let omitted = hook_digest(&original, &installed);
        installed.placed.push(points::GAME_TICK.base);
        let active = hook_digest(&original, &installed);
        assert_ne!(omitted, active);
        assert_eq!(active, hook_digest(&renamed, &installed));
        installed.off.push(("variant-test", "unavailable".into()));
        assert_eq!(omitted, hook_digest(&original, &installed));
    }

    #[test]
    fn hook_contract_distinguishes_native_and_generated_resumes() {
        let point = engine::sim_physics::PORTS[0].point;
        let digest = |native: &'static [u32], generated: &'static [u32]| {
            let mut registry = hooks::Registry::new(hooks::Switches::defaults());
            let port = hooks::FnFeature {
                manifest: hooks::Manifest::new("port-test", "Resume fixture"),
                build: move |b: &mut hooks::Builder| {
                    b.raw()
                        .implement(point, "port", |_, _, _| unreachable!())
                        .resumes(native)
                        .generated_resumes(generated);
                    Ok(())
                },
            };
            registry.add(&port).unwrap();
            let plan = registry.build(&hooks::Base).unwrap();
            let mut installed = hooks::Installed::default();
            installed.placed.push(point.base);
            hook_digest(&plan, &installed)
        };
        assert_ne!(digest(&[], &[]), digest(&[0x1000], &[]));
        assert_ne!(digest(&[0x1000], &[]), digest(&[], &[0x1000]));
        assert_eq!(
            digest(&[], &[0x1000, 0x2000]),
            digest(&[], &[0x2000, 0x1000])
        );
    }

    #[test]
    fn simulation_signature_uses_the_guest_calling_convention() {
        let sig = |args| Sig::Fn {
            args,
            ret: Some(Loc::Al),
            pop: 4,
        };
        assert_eq!(
            signature(sig(&[("update", Loc::Stack(0))])),
            signature(sig(&[("renamed", Loc::Stack(0))]))
        );
        assert_ne!(
            signature(sig(&[("update", Loc::Stack(0))])),
            signature(sig(&[("update", Loc::Stack(1))]))
        );
        assert_ne!(signature(sig(&[])), signature(Sig::Opaque));
        assert_ne!(signature(Sig::Site), signature(Sig::Opaque));
        assert_ne!(signature(Sig::Site), signature(sig(&[])));
    }
}
