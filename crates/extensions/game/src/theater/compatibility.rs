//! Compatibility is a guest contract. Host UI, codecs, dependency versions and
//! snapshot serializers have their own versioning and do not identify a match.
use super::{Mode, Session};
use agent_proto::theater::{Compatibility, CompatibilityState, FilmContract, InstalledImage};
use anyhow::{Result, ensure};
use engine::points::{self, Loc, Sig};
use film::image::{GuestImage, ImageMatch};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Bump for a backwards-incompatible change in scheduler, input or HLE
/// semantics that is not already identified by the image or installed hooks.
pub(super) const SIMULATION_ABI: &str = "h2-sim/1";
const BREAKING_CHANGE: &str = "Film predates a breaking simulation change.";

/// Check an installed, current module against the recording. The launcher also
/// uses this result to choose the disk state matching the replay's starting RAM.
pub fn image_match(
    version: u32,
    original: &str,
    runtime: &str,
    guest: Option<&GuestImage>,
    candidate: &str,
    xbe_sha256: &str,
) -> Result<ImageMatch> {
    match_image(
        version,
        original,
        runtime,
        guest,
        candidate,
        xbe_sha256,
        recomp::host_triple(),
    )
}

fn check_abi(version: u32, runtime: &str) -> Result<()> {
    ensure!(
        version == 1 || runtime == SIMULATION_ABI,
        "{BREAKING_CHANGE} Simulation ABI differs (recorded {runtime}, installed {SIMULATION_ABI})."
    );
    Ok(())
}

fn match_image(
    version: u32,
    original: &str,
    runtime: &str,
    guest: Option<&GuestImage>,
    candidate: &str,
    xbe_sha256: &str,
    target: &str,
) -> Result<ImageMatch> {
    check_abi(version, runtime)?;
    if version >= 3
        && let Some(guest) = guest
    {
        let installed = GuestImage::capture(candidate, target, xbe_sha256);
        if let (Some(recorded), Some(installed)) =
            (&guest.contract_sha256, &installed.contract_sha256)
        {
            ensure!(
                recorded == installed,
                "{BREAKING_CHANGE} Guest image contract differs."
            );
        }
        return guest
            .compare(original, candidate, target, xbe_sha256)
            .map_err(|reason| anyhow::anyhow!("film image stamp differs: {reason}"));
    }
    ensure!(
        !candidate.is_empty() && original == candidate,
        "film requires its exact native image stamp"
    );
    Ok(ImageMatch::Native)
}

/// Preflight facts for the current installation, shared by Watch and the
/// library agent. The replay verifies the hook digest after feature placement.
#[derive(Clone)]
pub struct Installation {
    xbe: String,
    stamps: Vec<String>,
    data: PathBuf,
}

impl Installation {
    pub fn new(xbe: &[u8], stamps: Vec<String>, data: PathBuf) -> Self {
        Self {
            xbe: film::xbe_digest(xbe),
            stamps,
            data,
        }
    }

    pub fn compatibility(&self, entry: &film::LibraryEntry) -> Compatibility {
        self.check(entry, recomp::host_triple(), || {
            entry.check_initial_files(&self.data)
        })
    }

    fn check(
        &self,
        entry: &film::LibraryEntry,
        target: &str,
        initial_files: impl FnOnce() -> Result<()>,
    ) -> Compatibility {
        use CompatibilityState::{Playable, Unavailable};

        let film = FilmContract {
            xbe: entry.xbe_sha256.clone(),
            contract: entry
                .guest_image
                .as_ref()
                .and_then(|guest| guest.contract_sha256.clone()),
            hooks: entry.simulation_hooks.clone(),
            abi: entry.runtime_stamp.clone(),
        };
        let prerequisites = || -> Result<()> {
            ensure!(
                entry.version >= 2 || entry.meta.source == film::Source::Host,
                "No playback track was saved."
            );
            check_abi(entry.version, &film.abi)?;
            ensure!(
                film.xbe == self.xbe,
                "Install the film's original XBE to play it."
            );
            ensure!(
                entry.version < 3 || film.hooks.is_some(),
                "The film is missing its simulation hook digest."
            );
            Ok(())
        };
        let mut reasons = Vec::new();
        let mut installed = self
            .stamps
            .iter()
            .map(|stamp| InstalledImage {
                stamp: stamp.clone(),
                state: Unavailable,
            })
            .collect::<Vec<_>>();
        if let Err(err) = prerequisites() {
            reasons.push(err.to_string());
        } else if installed.is_empty() {
            reasons.push("Install a current game image to play this film.".into());
        } else {
            for image in &mut installed {
                match match_image(
                    entry.version,
                    &entry.meta.image_stamp,
                    &film.abi,
                    entry.guest_image.as_ref(),
                    &image.stamp,
                    &self.xbe,
                    target,
                ) {
                    Ok(_) => image.state = Playable,
                    Err(error) => {
                        let reason = error.to_string();
                        if !reasons.contains(&reason) {
                            reasons.push(reason);
                        }
                    }
                }
            }
            if installed.iter().any(|image| image.state == Playable) {
                reasons.clear();
                if let Err(err) = initial_files() {
                    reasons.push(err.to_string());
                    for image in &mut installed {
                        image.state = Unavailable;
                    }
                }
            }
        }
        let state = if reasons.is_empty() {
            Playable
        } else {
            Unavailable
        };
        Compatibility {
            state,
            reasons,
            film,
            installed,
        }
    }
}

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
    pub fn check_simulation_hooks(
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
                "{BREAKING_CHANGE} Installed simulation hooks differ from the film."
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: &str = "arm64-apple-macos11";
    const LINUX: &str = "x86_64-unknown-linux-gnu";
    const STAMP: &str = "xbe A generator B passes P codegen C abi 1 flags [\"-fno-fast-math\",\"-ffp-contract=off\",\"-O2\"]\n";

    fn linux_stamp() -> String {
        format!("{} target {LINUX}\n", STAMP.trim_end())
    }

    #[test]
    fn image_match_table_covers_playable_and_unavailable() {
        let linux = linux_stamp();
        let changed = STAMP.replace("-O2", "-O3");
        let xbe = film::xbe_digest(b"synthetic XBE");
        let guest = GuestImage::capture(&linux, LINUX, &xbe);
        assert!(guest.contract_sha256.is_some());
        for (name, version, original, abi, guest, candidate, target, expected) in [
            (
                "same native target",
                3,
                linux.as_str(),
                SIMULATION_ABI,
                Some(&guest),
                linux.as_str(),
                LINUX,
                Ok(ImageMatch::Native),
            ),
            (
                "different native target",
                3,
                linux.as_str(),
                SIMULATION_ABI,
                Some(&guest),
                STAMP,
                MAC,
                Ok(ImageMatch::FromBoot),
            ),
            (
                "guest contract changed",
                3,
                linux.as_str(),
                SIMULATION_ABI,
                Some(&guest),
                changed.as_str(),
                MAC,
                Err(BREAKING_CHANGE),
            ),
            (
                "simulation ABI changed",
                3,
                linux.as_str(),
                "h2-sim/0",
                Some(&guest),
                linux.as_str(),
                LINUX,
                Err(BREAKING_CHANGE),
            ),
            (
                "legacy exact stamp",
                1,
                "native",
                "",
                None,
                "native",
                MAC,
                Ok(ImageMatch::Native),
            ),
            (
                "legacy different stamp",
                1,
                "native",
                "",
                None,
                "other",
                MAC,
                Err("exact native image stamp"),
            ),
            (
                "no image",
                2,
                "",
                SIMULATION_ABI,
                None,
                "",
                MAC,
                Err("exact native image stamp"),
            ),
        ] {
            let result = match_image(version, original, abi, guest, candidate, &xbe, target);
            match expected {
                Ok(mode) => assert_eq!(result.unwrap(), mode, "{name}"),
                Err(reason) => assert!(
                    result.unwrap_err().to_string().contains(reason),
                    "{name}: expected {reason}"
                ),
            }
        }
    }

    fn entry(abi: &str) -> film::LibraryEntry {
        let xbe_sha256 = film::xbe_digest(b"synthetic XBE");
        let image_stamp = linux_stamp();
        film::LibraryEntry {
            guest_image: Some(GuestImage::capture(&image_stamp, LINUX, &xbe_sha256)),
            meta: film::FilmMeta {
                id: "synthetic-film".into(),
                map: "synthetic".into(),
                variant: "Slayer".into(),
                started_ms: 0,
                length_ticks: 1,
                players: Vec::new(),
                source: film::Source::Local,
                image_stamp,
            },
            version: 3,
            summary: None,
            ticks_per_second: None,
            xbe_sha256,
            runtime_stamp: abi.into(),
            simulation_hooks: Some(film::xbe_digest(b"synthetic hooks")),
            saved: false,
            lease_id: None,
            game_id: None,
            path: "synthetic.h2film".into(),
        }
    }

    #[test]
    fn image_match_reports_a_breaking_abi_and_accepts_an_equal_contract_across_targets() {
        use CompatibilityState::{Playable, Unavailable};
        let mut installation =
            Installation::new(b"synthetic XBE", vec![STAMP.into()], PathBuf::new());
        let mut film = entry("h2-sim/0");
        let report = installation.check(&film, MAC, || panic!("unavailable at the header"));
        assert_eq!(report.state, Unavailable);
        assert_eq!(report.installed[0].state, Unavailable);
        assert!(report.reasons[0].starts_with(BREAKING_CHANGE));
        assert_eq!(report.film.abi, "h2-sim/0");
        assert_eq!(report.film.hooks, film.simulation_hooks);

        film.runtime_stamp = SIMULATION_ABI.into();
        installation
            .stamps
            .insert(0, "incompatible candidate".into());
        let report = installation.check(&film, MAC, || Ok(()));
        assert_eq!(report.state, Playable);
        assert_eq!(report.installed[0].state, Unavailable);
        assert_eq!(report.installed[1].state, Playable);
        assert!(report.reasons.is_empty());

        let report = installation.check(&film, MAC, || anyhow::bail!("Install the original map."));
        assert_eq!(report.state, Unavailable);
        assert_eq!(report.installed[1].state, Unavailable);
        assert_eq!(report.reasons, ["Install the original map."]);

        installation.xbe = film::xbe_digest(b"different synthetic XBE");
        let report = installation.check(&film, MAC, || panic!("unavailable at the header"));
        assert_eq!(report.state, Unavailable);
        assert!(report.reasons[0].contains("original XBE"));
    }

    #[test]
    fn theater_compat_and_watch_use_the_same_breaking_change_reason() {
        use agent_proto::{Object, RpcError, code, theater as api};
        use serde_json::{Value, json};
        use std::fs;

        struct Temp(PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        struct Empty;
        impl agent::Reader for Empty {
            fn status(&mut self) -> agent::GameStatus {
                Default::default()
            }
            fn objects(&mut self) -> Result<Vec<Object>, String> {
                Ok(Vec::new())
            }
        }

        let dir = Temp(
            std::env::temp_dir().join(format!("film-compat-{}", live_proto::sign::random_id())),
        );
        fs::create_dir(&dir.0).unwrap();
        let old = entry("h2-sim/0");
        let mut header = film::Header {
            version: old.version,
            meta: old.meta,
            title_id: 1,
            xbe_sha256: old.xbe_sha256,
            runtime_stamp: old.runtime_stamp,
            simulation_hooks: old.simulation_hooks,
            guest_image: None,
            epoch_100ns: 0,
            lease_id: None,
            game_id: None,
            xnet_addr: String::new(),
            transfers_per_slice: 1,
            lobby: false,
            lobby_machine_start: false,
            host_bots: false,
            telemetry: false,
            telemetry_hooks: None,
            render_hooks: None,
            live_service: false,
            features: String::new(),
        };
        header.meta.image_stamp = STAMP.into();
        for (id, abi) in [("old", "h2-sim/0"), ("current", SIMULATION_ABI)] {
            header.meta.id = id.into();
            header.runtime_stamp = abi.into();
            let file = fs::File::create(dir.0.join(format!("{id}.h2film"))).unwrap();
            let mut writer = film::Writer::new(file, &header).unwrap();
            writer
                .chunk(film::DATA, &film::manifest::Manifest::default())
                .unwrap();
            writer
                .chunk(film::blam::SUMM, &film::blam::Summary::default())
                .unwrap();
            writer.finish().unwrap();
        }

        let a = agent::Agent::new();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let client = a.connect(tx);
        let install = Installation::new(b"synthetic XBE", vec![STAMP.into()], dir.0.clone());
        super::super::library_methods(
            &a,
            dir.0.clone(),
            dir.0.join("synthetic.xbe"),
            dir.0.clone(),
            dir.0.clone(),
            install,
        );
        let call = |method: &str, params: Value| {
            a.submit(
                client,
                &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
            );
            a.on_tick(&mut Empty);
            serde_json::from_str::<Value>(&rx.try_recv().unwrap()).unwrap()
        };
        let methods = call("session.methods", json!({}));
        assert!(
            methods["result"]
                .as_array()
                .unwrap()
                .contains(&json!(api::COMPAT))
        );
        let bad = call(api::COMPAT, json!({}));
        assert_eq!(bad["error"]["code"], code::INVALID_PARAMS);

        let result = call(api::COMPAT, json!({"id":"old"}));
        let report: Compatibility = serde_json::from_value(result["result"].clone()).unwrap();
        assert_eq!(report.state, CompatibilityState::Unavailable);
        assert!(report.reasons[0].starts_with(BREAKING_CHANGE));
        let refused = call(api::PLAY, json!({"id":"old"}));
        let error: RpcError = serde_json::from_value(refused["error"].clone()).unwrap();
        assert_eq!(error.code, code::UNAVAILABLE);
        assert_eq!(error.message, report.reasons[0]);

        let result = call(api::COMPAT, json!({"id":"current"}));
        assert_eq!(result["result"]["state"], "playable");
        assert_eq!(result["result"]["installed"][0]["state"], "playable");
    }

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
