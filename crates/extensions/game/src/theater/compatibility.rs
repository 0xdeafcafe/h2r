//! Compatibility is a guest contract. Host UI, codecs, dependency versions and
//! snapshot serializers have their own versioning and do not identify a match.
use super::{Mode, Session};
use agent_proto::theater::{
    Compatibility, CompatibilityState, FilmContract, InstalledImage, MonitorLookSource,
};
use anyhow::{Result, ensure};
use guest_layout::points::{self, Loc, Sig};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Bump for a backwards-incompatible change in scheduler, input or HLE
/// semantics that is not already identified by the image or installed hooks.
/// `h2-sim/2`: the call tape is the binary stream, the pure network
/// operations take no call index, and a game that has made no socket holds
/// no socket call (docs/game/input-film.md, "What the replay depends on").
pub(super) const SIMULATION_ABI: &str = "h2-sim/2";
/// Why a film recorded before game starts does not open.
pub const NO_GAME_START: &str =
    "This film predates game starts, so it cannot be played: it was recorded before films began at the game's start.";
const BREAKING_CHANGE: &str = "Film predates a breaking simulation change.";

fn check_abi(version: u32, runtime: &str) -> Result<()> {
    ensure!(
        version == 1 || runtime == SIMULATION_ABI,
        "{BREAKING_CHANGE} Simulation ABI differs (recorded {runtime}, installed {SIMULATION_ABI})."
    );
    Ok(())
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

    /// The viewer's data directory.
    pub fn data(&self) -> &std::path::Path {
        &self.data
    }

    pub fn compatibility(&self, entry: &film::LibraryEntry) -> Compatibility {
        // A film plays from its game start: it needs no file of the
        // viewer's disk, and a film without a game start does not play.
        let start = film::file::game_start_of(&entry.path);
        let mut report = self.check(entry, || match &start {
            Ok(Some(_)) => Ok(()),
            Ok(None) => Err(anyhow::anyhow!(NO_GAME_START)),
            Err(error) => Err(anyhow::anyhow!("The film's game start cannot be read: {error:#}")),
        });
        // A film of a Forge session names the monitor looks this machine
        // draws as markers; they are not reasons the film is unavailable.
        if entry.version >= 2 {
            note_monitor_looks(
                &mut report,
                &super::forge_film::unseen_in(&entry.path, &crate::forge_glue::can_draw()),
            );
        }
        report
    }

    /// A film is playable when it was recorded on this XBE and simulation,
    /// an image is installed to play it on, and it has a game start
    /// (`game_start`). Every installed image plays a film from its game
    /// start: no machine part of the film is restored.
    fn check(
        &self,
        entry: &film::LibraryEntry,
        game_start: impl FnOnce() -> Result<()>,
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
        } else if let Err(err) = game_start() {
            reasons.push(err.to_string());
        } else {
            for image in &mut installed {
                image.state = Playable;
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
            monitor_looks: Vec::new(),
        }
    }
}

/// Note the recorded monitor looks this machine cannot draw
/// (`monitor_look_source`). They do not make the film unavailable: the
/// simulation is whole and the seat draws the marker in the look's colours.
pub fn note_monitor_looks(report: &mut Compatibility, unseen: &[forge::look::film::Unseen]) {
    report.monitor_looks = monitor_look_sources(unseen);
}

/// The looks a machine cannot draw as the agent API names them.
pub(super) fn monitor_look_sources(
    unseen: &[forge::look::film::Unseen],
) -> Vec<MonitorLookSource> {
    unseen
        .iter()
        .map(|u| MonitorLookSource {
            seat: u.seat,
            map: u.map.clone(),
            reason: u.reason.clone(),
        })
        .collect()
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

/// The film's simulation hooks: what a replay's installed plan must equal
/// (`Session::check_simulation_hooks`). Features marked presentation-only,
/// and features install turned off, are left out.
pub fn hook_digest(plan: &hooks::Plan, installed: &hooks::Installed) -> String {
    let placed = installed.placed.iter().copied().collect::<BTreeSet<_>>();
    let presentation = plan.features.iter().filter(|f| f.presentation_only)
        .map(|f| f.id).collect::<BTreeSet<_>>();
    let presentation_pages = plan.allocs.iter().filter(|a| a.presentation_only)
        .map(|a| a.addr).collect::<BTreeSet<_>>();
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
            .filter(|h| !presentation.contains(h.feature) && installed.off_reason(h.feature).is_none())
        {
            let row = (
                "hook",
                chain.addr,
                chain.shadow.filter(|addr| placed.contains(addr) && !presentation_pages.contains(addr)),
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
    for entry in plan.entries.iter().filter(|e| placed.contains(&e.addr) && !presentation.contains(e.feature)) {
        serde_json::to_writer(&mut bytes, &("entry", entry.addr, entry.feature))
            .expect("fixed guest entry signature");
        bytes.push(b'\n');
    }
    for alloc in plan
        .allocs
        .iter()
        .filter(|a| !a.presentation_only && installed.off_reason(a.feature).is_none())
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
        let actual = hook_digest(plan, installed);
        self.set_simulation_hooks(actual)
    }

    /// Record mode takes `actual` as the film's digest. A film opened
    /// before the hooks were installed (a named recording, open from boot)
    /// takes it in a header appended to its stats container; its game
    /// start's stamp names it too. Replay requires the film's digest to
    /// equal it.
    pub(super) fn set_simulation_hooks(&self, actual: String) -> Result<()> {
        let mut state = self.lock();
        if state.header.version < 3 {
            return Ok(());
        }
        if matches!(&state.mode, Mode::Record(_)) {
            state.header.simulation_hooks = Some(actual.clone());
            if let Mode::Record(rec) = &mut state.mode
                && let Some(game) = rec.game.as_mut()
            {
                game.header.simulation_hooks = Some(actual);
                game.stats.chunk(film::HEAD, &game.header)?;
            }
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

    const LINUX: &str = "x86_64-unknown-linux-gnu";
    const STAMP: &str = "xbe A generator B passes P codegen C abi 1 flags [\"-fno-fast-math\",\"-ffp-contract=off\",\"-O2\"]\n";

    fn linux_stamp() -> String {
        format!("{} target {LINUX}\n", STAMP.trim_end())
    }

    fn entry(abi: &str) -> film::LibraryEntry {
        let xbe_sha256 = film::xbe_digest(b"synthetic XBE");
        let image_stamp = linux_stamp();
        film::LibraryEntry {
            guest_image: Some(film::image::GuestImage::capture(&image_stamp, LINUX, &xbe_sha256)),
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
    fn a_breaking_abi_or_no_game_start_is_unavailable_and_any_installed_image_plays_the_rest() {
        use CompatibilityState::{Playable, Unavailable};
        let mut installation =
            Installation::new(b"synthetic XBE", vec![STAMP.into()], PathBuf::new());
        let mut film = entry("h2-sim/0");
        let report = installation.check(&film, || panic!("unavailable at the header"));
        assert_eq!(report.state, Unavailable);
        assert_eq!(report.installed[0].state, Unavailable);
        assert!(report.reasons[0].starts_with(BREAKING_CHANGE));
        assert_eq!(report.film.abi, "h2-sim/0");
        assert_eq!(report.film.hooks, film.simulation_hooks);

        // Another build of the same guest plays it: the film is played from
        // its game start, not restored on the image it was recorded on.
        film.runtime_stamp = SIMULATION_ABI.into();
        installation.stamps.insert(0, "another build".into());
        let report = installation.check(&film, || Ok(()));
        assert_eq!(report.state, Playable);
        assert_eq!(report.installed[0].state, Playable);
        assert_eq!(report.installed[1].state, Playable);
        assert!(report.reasons.is_empty());

        let report = installation.check(&film, || anyhow::bail!(NO_GAME_START));
        assert_eq!(report.state, Unavailable);
        assert_eq!(report.installed[1].state, Unavailable);
        assert_eq!(report.reasons, [NO_GAME_START]);

        let empty = Installation::new(b"synthetic XBE", Vec::new(), PathBuf::new());
        let report = empty.check(&film, || Ok(()));
        assert_eq!(report.state, Unavailable);
        assert!(report.reasons[0].contains("Install a current game image"));

        // A look this machine cannot draw is listed beside the state, not
        // a reason against it.
        let mut report = installation.check(&film, || Ok(()));
        let state = report.state;
        note_monitor_looks(
            &mut report,
            &[forge::look::film::Unseen {
                seat: 2,
                map: "invented_map".into(),
                reason: forge::look::film::REASON.into(),
            }],
        );
        assert_eq!(report.state, state);
        assert_eq!(report.monitor_looks[0].seat, 2);
        assert_eq!(report.monitor_looks[0].reason, "monitor_look_source");

        installation.xbe = film::xbe_digest(b"different synthetic XBE");
        let report = installation.check(&film, || panic!("unavailable at the header"));
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
            inputs_in_film: false,
            role: Default::default(),
            live_service: false,
            features: String::new(),
            player_features: false,
        };
        header.meta.image_stamp = STAMP.into();
        // Each film is a pair: its film file, holding a game start, and its
        // stats container beside it.
        for (id, abi) in [("old", "h2-sim/0"), ("current", SIMULATION_ABI)] {
            header.meta.id = id.into();
            header.runtime_stamp = abi.into();
            let path = dir.0.join(format!("{id}.h2film"));
            let file = fs::File::create(film::file::stats_of(&path)).unwrap();
            let mut writer = film::Writer::new(file, &header).unwrap();
            writer
                .chunk(film::blam::SUMM, &film::blam::Summary::default())
                .unwrap();
            writer.finish().unwrap();
            let start = film::start::GameStart {
                stamp: Default::default(),
                game: film::start::Game { first_update: 1, ..Default::default() },
                options: vec![0; film::start::OPTIONS_BYTES],
                random: [0, 0],
                start_hash: 0,
                settings: Default::default(),
                variant: None,
                map_variant: None,
                hash_detail: None,
            };
            let seed = film::file::Seed { header: header.clone(), descriptor: Default::default() };
            let mut writer =
                film::file::FilmWriter::new(fs::File::create(&path).unwrap(), &seed, 30).unwrap();
            writer.game_start(&start).unwrap();
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
            None,
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
        assert_eq!(call(api::EXIT, json!({}))["error"]["code"], code::UNAVAILABLE);
        assert_eq!(call(api::STATE, json!({}))["result"]["active"], false);
        let films = call(api::LIST, json!({}));
        let rows = films["result"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row["id"] == "current" && row["saved"] == false));
        assert_eq!(call(api::LIST, json!({"unknown": 1}))["error"]["code"], code::INVALID_PARAMS);
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

    #[cfg(feature = "gpu")]
    #[test]
    fn editor_marks_preserve_the_v3_hook_digest() {
        use std::sync::Arc;

        use render_bridge::scene_step::{FrameSceneStep, PresentObserver, PresentPayload};

        struct Observer;

        impl PresentObserver for Observer {
            fn capture(&self, _: &dyn guest_layout::GuestMemory) -> Option<PresentPayload> {
                Some(Arc::new(()))
            }

            fn horizontal_factor(&self) -> f32 {
                1.0
            }
        }

        let digest = |present: Option<Arc<dyn PresentObserver>>| {
            let mut registry = hooks::Registry::new(hooks::Switches::defaults());
            assert!(registry.add(&FrameSceneStep { present }).unwrap());
            let plan = registry.build(&hooks::Base).unwrap();
            let installed = hooks::Installed {
                placed: plan
                    .chains
                    .values()
                    .flat_map(|chain| [Some(chain.addr), chain.shadow].into_iter().flatten())
                    .chain(plan.entries.iter().map(|entry| entry.addr))
                    .collect(),
                ..Default::default()
            };
            assert!(installed.placed.contains(&points::RENDER_VIEWS.base));
            hook_digest(&plan, &installed)
        };
        assert_eq!(digest(None), digest(Some(Arc::new(Observer))));
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
    fn theater_presentation_leaves_recorded_dispatch_addresses_and_digest_unchanged() {
        let plan = |with_camera| {
            let mut registry = hooks::Registry::new(hooks::Switches::defaults());
            let fixture = hooks::FnFeature {
                manifest: hooks::Manifest::new("render-aspect", "Recorded simulation fixture"),
                build: |b: &mut hooks::Builder| {
                    b.guest_alloc(8, runtime::guest::Permissions::RW)?;
                    b.raw().before(&points::UI_HUD_DRAW, "HUD observer", |_| Ok(hooks::Flow::Continue));
                    b.raw().after(&points::GAME_TICK, "recorded tick observer", |_| Ok(()));
                    Ok(())
                },
            };
            registry.add(&fixture).unwrap();
            if with_camera {
                registry.add(&super::super::camera::RenderHooks(Default::default())).unwrap();
            }
            let plan = registry.build(&hooks::Base).unwrap();
            let mut installed = hooks::Installed::default();
            installed.placed.extend(plan.chains.values().flat_map(|chain|
                std::iter::once(chain.addr).chain(chain.shadow)));
            (plan, installed)
        };
        let (recording, recorded) = plan(false);
        let (player, playback) = plan(true);
        assert_eq!(recording.allocs, player.allocs.iter().copied()
            .filter(|alloc| !alloc.presentation_only).collect::<Vec<_>>());
        assert!(player.chains[&points::UI_HUD_DRAW.base].shadow.is_some());
        assert_eq!(hook_digest(&recording, &recorded), hook_digest(&player, &playback));
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
