//! **Generate nav** in a Forge session (docs/game/forge-nav.md).
//!
//! The session asks for a generation ([`forge::Session::generate_nav`])
//! and the app supplies the generator ([`install`]): `forge-nav` over the
//! materialised play form, run off the game thread. `game` may not use
//! `tools`, so this module holds no generator; with none installed the row
//! says so and changes nothing. Nothing here runs while placing.
//!
//! The generation runs on a worker, as a file job of the editor
//! (`editing::Editor::start_nav`), over a copy of the session the editor
//! takes back when it is done, as the AI ground's build does
//! (`forge_glue::ground`).
use std::sync::{Arc, Mutex, TryLockError};

use bots::cache::MapData;

use forge::{NavBuilder, NavReport, Session};

/// What the row says when no generator is installed.
pub const NO_GENERATOR: &str = agent::forge::nav::NO_GENERATOR;

/// Tests that install the process-wide generator run one at a time.
#[cfg(test)]
pub(super) static TEST_GUARD: Mutex<()> = Mutex::new(());

type Builder = Box<dyn NavBuilder + Send>;

static BUILDER: Mutex<Option<Builder>> = Mutex::new(None);

/// Supply the generator the row uses, replacing any.
pub fn install(builder: Builder) {
    *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(builder);
}

/// The generator is gone: the row refuses again.
pub fn uninstall() {
    *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Whether a generator is installed. A generation in progress holds the
/// lock, and the generator is then there, so this never waits for one.
pub fn available() -> bool {
    match BUILDER.try_lock() {
        Ok(guard) => guard.is_some(),
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Poisoned(e)) => e.into_inner().is_some(),
    }
}

/// The mesh the last generation made, for the bots of a match
/// (docs/game/forge-nav.md, "The bots").
struct Staged {
    map: String,
    mesh: Arc<MapData>,
}

static STAGED: Mutex<Option<Staged>> = Mutex::new(None);

/// Keep `mesh`, made for the base map `map`, for the bots that load it. The
/// generator calls this when a generation ends; the editor drops it
/// ([`unstage`]) when the document moves on.
pub fn stage(map: &str, mesh: MapData) {
    *STAGED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Staged {
        map: map.to_owned(),
        mesh: Arc::new(mesh),
    });
}

/// The staged mesh when it is for `map`, however the name is cased.
pub fn staged(map: &str) -> Option<Arc<MapData>> {
    let staged = STAGED.lock().unwrap_or_else(|e| e.into_inner());
    staged
        .as_ref()
        .filter(|s| s.map.eq_ignore_ascii_case(map))
        .map(|s| s.mesh.clone())
}

/// The document moved on or the session ended: no bot takes the mesh.
pub fn unstage() {
    *STAGED.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Put the staged mesh under the bots of the loaded map `map` now. False
/// when none is staged for it or another map is loaded.
pub fn offer(map: &str) -> bool {
    staged(map).is_some_and(|mesh| bots::autonav::offer_loaded(map, mesh))
}

/// **Generate nav** on the worker's copy of the session.
pub(super) fn run(session: &mut Session) -> Result<(), String> {
    let mut guard = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    let builder = guard.as_mut().ok_or(NO_GENERATOR)?;
    session.generate_nav(&mut **builder).map(|_| ())
}

/// The sentence the HUD says when a generation is done.
pub(super) fn notice(report: Option<&NavReport>) -> String {
    let Some(report) = report else {
        return "Nav generated.".into();
    };
    let problems = match report.problems.len() {
        0 => String::new(),
        1 => ", 1 problem".into(),
        n => format!(", {n} problems"),
    };
    format!(
        "Nav generated: {} sectors, {} out of the map{problems}.",
        report.sectors, report.out_of_map
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge::{NavProblem, NavState};
    use forge_doc::{BlockKind, ForgeDoc};
    use std::collections::BTreeMap;

    struct Gen;

    impl NavBuilder for Gen {
        fn generate(
            &mut self,
            _: &ForgeDoc,
            _: &BTreeMap<BlockKind, u32>,
        ) -> Result<NavReport, String> {
            Ok(NavReport {
                sectors: 9,
                ..Default::default()
            })
        }
    }

    fn session() -> Session {
        use forge_doc::{BuilderVersions, Limits, MapRef, Sha256, WorldBounds};
        let base = ForgeDoc {
            format: forge_doc::FORMAT,
            builder: BuilderVersions::CURRENT,
            base: MapRef {
                map: "court".into(),
                sha256: Sha256::of(b"an invented court"),
            },
            sources: vec![],
            title: "Ramps".into(),
            author: "Builder".into(),
            description: String::new(),
            recipes: vec![],
            imports: vec![],
            ops: vec![],
            expect: None,
        };
        let limits = Limits {
            base_counts: BTreeMap::from([(BlockKind::NetgameFlags, 3)]),
            world: Some(WorldBounds {
                min: [-30.; 3],
                max: [30.; 3],
            }),
            ..Default::default()
        };
        Session::new(base, limits, vec![], 4).unwrap()
    }

    #[test]
    fn the_row_refuses_with_no_generator_and_generates_with_one() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        uninstall();
        let mut s = session();
        assert_eq!(run(&mut s), Err(NO_GENERATOR.to_string()));
        assert!(!available());
        assert_eq!(s.nav_state(), NavState::NotBuilt);

        install(Box::new(Gen));
        assert!(available());
        run(&mut s).unwrap();
        assert_eq!(s.nav_state(), NavState::Current);
        assert_eq!(s.nav_report().map(|r| r.sectors), Some(9));
        uninstall();
    }

    #[test]
    fn a_staged_mesh_is_for_its_map_until_it_is_dropped() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        unstage();
        assert!(staged("court").is_none());
        stage("court", MapData::default());
        assert!(staged("COURT").is_some());
        assert!(staged("lockout").is_none());
        // Nothing is loaded in the bots: the offer is refused, the stage kept.
        assert!(!offer("court"));
        assert!(staged("court").is_some());
        unstage();
        assert!(staged("court").is_none());
    }

    #[test]
    fn the_notice_says_what_was_generated() {
        let report = NavReport {
            sectors: 40,
            out_of_map: 2,
            ..Default::default()
        };
        assert_eq!(
            notice(Some(&report)),
            "Nav generated: 40 sectors, 2 out of the map."
        );
        let problem = NavProblem {
            kind: "start_off_mesh".into(),
            what: "start 0".into(),
            position: [0.; 3],
            reason: "no floor under it".into(),
        };
        let two = NavReport {
            problems: vec![problem.clone(), problem],
            ..report
        };
        assert_eq!(
            notice(Some(&two)),
            "Nav generated: 40 sectors, 2 out of the map, 2 problems."
        );
    }
}
