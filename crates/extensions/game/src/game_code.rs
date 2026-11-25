//! Which game code a start runs (docs/lifter/cranelift-tier.md): clang's
//! full-speed module, or the Cranelift fast tier while clang's build runs
//! in the background for the next start. `game.code` says which, where the
//! module is, and how the background build is going; `game.code.restart`
//! starts the game again so the next start picks the full-speed module.
//! The debug menu's Game > Game code page lists both
//! (docs/runtime/debug-menu.md); `h2 play`'s banner reads the same build
//! state ([`build_state`], [`banner_text`]).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use agent::Access;
use agent_proto::{RpcError, code};
use serde_json::{Value, json};

pub use debug_menu::sources::method::{GAME_CODE, GAME_CODE_RESTART};

/// How long clang's background build usually takes from cold, for the
/// banner's estimate (docs/lifter/BUILD-CACHE.md: about 16 min at
/// `H2_OPT=2` without the object cache; much less with it).
pub const TYPICAL_BUILD: Duration = Duration::from_secs(15 * 60);

/// Starts the game again in place (the same executable and arguments).
/// Only returns on failure.
pub type Restart = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// What `h2 play` knows about this start's game code.
pub struct GameCode {
    /// `clang`, `fast` or `slim host` (`boot::tier::Tier::name`).
    pub tier: &'static str,
    /// The module this start loaded.
    pub module: PathBuf,
    /// The module's stamp file, read for its contents.
    pub stamp: PathBuf,
    /// clang's background build log, when this start began one.
    pub background_log: Option<PathBuf>,
    /// The module clang's build writes, for the restart check.
    pub full_speed_module: PathBuf,
    /// How to start again; `None` where a restart in place isn't possible.
    pub restart: Option<Restart>,
}

/// Where clang's background build is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildState {
    /// No background build: this start already runs the full-speed module
    /// (or a tier was forced).
    None,
    Building,
    Ready,
    Failed,
}

impl BuildState {
    pub fn name(self) -> &'static str {
        match self {
            BuildState::None => "none",
            BuildState::Building => "building",
            BuildState::Ready => "ready",
            BuildState::Failed => "failed",
        }
    }
}

/// Where clang's background build is, from its log: its summary line
/// (`clang (...): N compiled, ...`) ends a good build, `Error:` a failed
/// one.
pub fn build_state(log: &str) -> BuildState {
    if log.lines().any(|l| l.starts_with("Error:") || l.starts_with("error:")) {
        BuildState::Failed
    } else if log.lines().any(|l| l.starts_with("clang (") && l.contains(" compiled, ")) {
        BuildState::Ready
    } else {
        BuildState::Building
    }
}

/// The last line of the log that says what the build is doing, for a
/// progress hint (`lift: 0.2s ...`, `emit: 2.6s ...`).
pub fn last_step(log: &str) -> Option<&str> {
    log.lines().rev().map(str::trim).find(|l| !l.is_empty())
}

/// The banner's text for a build `elapsed` in: one read for a player,
/// short enough for one line of the debug menu, which shows the banner
/// inside its panel while it is open (`debug_menu::layout::lines_with`).
pub fn banner_text(state: BuildState, elapsed: Duration) -> Option<String> {
    Some(match state {
        BuildState::None => return None,
        BuildState::Building => {
            let left = TYPICAL_BUILD.saturating_sub(elapsed).as_secs().div_ceil(60).max(1);
            format!("Running slower: full-speed version in about {left} min.")
        }
        BuildState::Ready => {
            "Full-speed ready. Restart from debug menu > Game > Game code.".into()
        }
        BuildState::Failed => {
            "Full-speed build failed (clang-build.log); running slower.".into()
        }
    })
}

/// Seconds since `path` was last written, if it exists.
fn age(path: &std::path::Path) -> Option<Duration> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.created().or_else(|_| m.modified()).ok()?;
    SystemTime::now().duration_since(t).ok()
}

impl GameCode {
    /// The background build's state now and how long it has run.
    pub fn build(&self) -> (BuildState, Option<Duration>, String) {
        let Some(log) = &self.background_log else {
            return (BuildState::None, None, String::new());
        };
        let text = std::fs::read_to_string(log).unwrap_or_default();
        (build_state(&text), age(log), text)
    }

    /// `game.code`'s answer.
    pub fn answer(&self) -> Value {
        let (state, elapsed, log_text) = self.build();
        let stamp = std::fs::read_to_string(&self.stamp)
            .ok()
            .map(|s| s.lines().next().unwrap_or_default().trim().to_owned());
        let restart = self.restart_refusal(state);
        json!({
            "tier": self.tier,
            "full_speed": self.tier != "fast",
            "module": self.module.display().to_string(),
            "stamp": stamp,
            "background": {
                "state": state.name(),
                "log": self.background_log.as_ref().map(|p| p.display().to_string()),
                "elapsed_s": elapsed.map(|d| d.as_secs()),
                "typical_s": TYPICAL_BUILD.as_secs(),
                "step": last_step(&log_text),
            },
            "restart": {
                "ok": restart.is_none(),
                "why": restart,
            },
        })
    }

    /// Why a restart now wouldn't reach the full-speed module, if it
    /// wouldn't.
    pub fn restart_refusal(&self, state: BuildState) -> Option<String> {
        if self.tier != "fast" {
            return Some("already running the full-speed version".into());
        }
        match state {
            BuildState::Building => return Some("the full-speed version is still building".into()),
            BuildState::Failed => return Some("the full-speed build failed; see its log".into()),
            BuildState::None => return Some("no full-speed build was started (a tier was forced)".into()),
            BuildState::Ready => {}
        }
        if !self.full_speed_module.exists() {
            return Some(format!("{} is missing", self.full_speed_module.display()));
        }
        if self.restart.is_none() {
            return Some("quit and start h2 play again".into());
        }
        None
    }
}

/// Adds `game.code` and `game.code.restart` to `agent`.
pub fn install(agent: &agent::Agent, code: Arc<GameCode>) {
    let c = Arc::clone(&code);
    agent.add_method(GAME_CODE, Access::Read, move |_| Ok(c.answer()));
    agent.add_method(GAME_CODE_RESTART, Access::Act, move |_| {
        let (state, _, _) = code.build();
        if let Some(why) = code.restart_refusal(state) {
            return Err(RpcError::new(code::UNAVAILABLE, why));
        }
        let restart = code.restart.as_ref().expect("restart_refusal checks it");
        eprintln!("game code: restarting into the full-speed version");
        restart().map_err(|e| RpcError::new(code::UNAVAILABLE, format!("restart failed: {e}")))?;
        Ok(json!({"restarting": true}))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(tier: &'static str, log: Option<PathBuf>, module: PathBuf, restart: bool) -> GameCode {
        GameCode {
            tier,
            module: PathBuf::from("/out/fast/libh2game.dylib"),
            stamp: PathBuf::from("/nonexistent/stamp"),
            background_log: log,
            full_speed_module: module,
            restart: restart.then(|| Arc::new(|| Ok(())) as Restart),
        }
    }

    #[test]
    fn the_build_state_follows_clang_s_log() {
        assert_eq!(build_state(""), BuildState::Building);
        assert_eq!(build_state("lift: 0.2s\nemit: 2.6s, 18662 owners\n"), BuildState::Building);
        assert_eq!(
            build_state("emit: 2.6s\nclang (O1): 965 compiled, 42 reused, 1.0s keying\n"),
            BuildState::Ready
        );
        assert_eq!(build_state("emit: 2.6s\nError: clang failed\n"), BuildState::Failed);
        assert_eq!(last_step("lift: 0.2s\nemit: 2.6s\n\n"), Some("emit: 2.6s"));
    }

    #[test]
    fn the_banner_reads_in_one_go_and_counts_down() {
        let t = banner_text(BuildState::Building, Duration::from_secs(5 * 60)).unwrap();
        assert_eq!(t, "Running slower: full-speed version in about 10 min.");
        let late = banner_text(BuildState::Building, Duration::from_secs(60 * 60)).unwrap();
        assert!(late.contains("about 1 min"), "{late}");
        assert!(banner_text(BuildState::None, Duration::ZERO).is_none());
        for s in [BuildState::Building, BuildState::Ready, BuildState::Failed] {
            let t = banner_text(s, Duration::ZERO).unwrap();
            assert!(
                t.chars().count() <= debug_menu::layout::COLS,
                "fits one line of the debug menu: {t}"
            );
        }
    }

    #[test]
    fn a_restart_waits_for_a_ready_build_and_a_module() {
        let dir = std::env::temp_dir().join(format!("h2-game-code-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("clang-build.log");
        let module = dir.join("libh2game.dylib");

        std::fs::write(&log, "lift: 0.2s\n").unwrap();
        let c = code("fast", Some(log.clone()), module.clone(), true);
        assert_eq!(c.answer()["background"]["state"], "building");
        assert_eq!(c.answer()["restart"]["ok"], false);

        std::fs::write(&log, "clang (O1): 965 compiled, 0 reused\n").unwrap();
        assert!(c.restart_refusal(BuildState::Ready).unwrap().contains("missing"));
        std::fs::write(&module, b"").unwrap();
        assert_eq!(c.answer()["restart"]["ok"], true);
        assert_eq!(c.answer()["full_speed"], false);

        let no_exec = code("fast", Some(log.clone()), module.clone(), false);
        assert_eq!(no_exec.restart_refusal(BuildState::Ready).unwrap(), "quit and start h2 play again");

        let clang = code("clang", None, module, true);
        assert_eq!(clang.answer()["background"]["state"], "none");
        assert_eq!(clang.answer()["full_speed"], true);
        assert!(clang.restart_refusal(BuildState::None).unwrap().contains("already"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
