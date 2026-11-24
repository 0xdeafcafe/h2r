//! The playtest result the viewport draws and the Playtest panel lists
//! (docs/editor/playtest.md, "What is measured"; slice PT5).
//!
//! The runner lives with the API provider; each call that moves a run
//! publishes it here ([`Shown::show`]): the measures of one game type
//! over the matches that ended, and the flags the layers dim by. The
//! measures are recomputed only when a match ends or another game type
//! is asked for.

use sim_bench::spatial::Measures;

use super::playtest::{Compared, Run, Runner, State, compare_runs};

/// What `Shown::show` last took from a run, to tell a new result from the
/// same one.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    run: String,
    game_type: String,
    done: usize,
}

/// The result on show.
#[derive(Default)]
pub struct Shown {
    key: Option<Key>,
    /// Bumped when the measures or the flags change, so the viewport
    /// uploads again.
    pub rev: u64,
    pub run: String,
    pub game_type: String,
    pub build: String,
    pub state: &'static str,
    /// A newer build exists: the layers keep their marks, dimmed.
    pub stale: bool,
    /// Under eight seeds: the spread is not worth trusting.
    pub coarse: bool,
    /// Matches are still playing.
    pub partial: bool,
    pub seeds: usize,
    pub measures: Option<Measures>,
    /// The session's runs, so a difference layer can be asked for with no
    /// runner in reach (`view.layers {diff}`).
    runs: Vec<Run>,
    /// The id of the baseline run (the first with a match that ended).
    pub baseline: Option<String>,
    /// What the layers are asked to be compared with: a run id, `baseline`,
    /// or `default` (the shown run's own `compare_to`, else the baseline).
    diff_want: Option<String>,
    /// The comparison drawn, or why there is none; `diff_key` tells it from
    /// a stale one.
    diff: Option<Result<Compared, String>>,
    diff_key: Option<(String, String, String, usize)>,
}

impl Shown {
    /// Takes the session's runs.
    pub fn sync(&mut self, runner: &Runner) {
        self.runs = runner.runs().to_vec();
        self.baseline = runner.baseline().map(|r| r.id.clone());
    }

    /// Asks for the layers to be drawn as a difference from `with` (a run
    /// id, `baseline` or `default`), or for the plain result (`None`).
    /// Refused when there is nothing to compare, so the caller says why.
    pub fn set_diff(&mut self, with: Option<&str>) -> Result<(), String> {
        self.diff_want = with.map(str::to_owned);
        self.diff_key = None;
        self.diff = None;
        self.refresh_diff();
        self.rev += 1;
        match &self.diff {
            Some(Err(why)) => Err(why.clone()),
            _ => Ok(()),
        }
    }

    /// What the layers are asked to be compared with.
    pub fn diff_with(&self) -> Option<&str> {
        self.diff_want.as_deref()
    }

    /// The comparison the layers draw, when one is asked for and stands.
    pub fn diff(&self) -> Option<&Compared> {
        self.diff.as_ref().and_then(|d| d.as_ref().ok())
    }

    /// Why a difference was asked for and none can be drawn.
    pub fn diff_refusal(&self) -> Option<&str> {
        self.diff
            .as_ref()
            .and_then(|d| d.as_ref().err().map(String::as_str))
    }

    /// Recomputes the comparison when its runs or their matches changed.
    fn refresh_diff(&mut self) -> bool {
        let Some(want) = self.diff_want.clone() else {
            return self.diff.take().is_some();
        };
        let Some(shown) = self.runs.iter().find(|r| r.id == self.run) else {
            self.diff = Some(Err("no result is shown".into()));
            return true;
        };
        let with = match want.as_str() {
            "default" => shown
                .compare_to
                .clone()
                .or_else(|| self.baseline.clone())
                .unwrap_or_else(|| "baseline".into()),
            "baseline" => self.baseline.clone().unwrap_or_else(|| "baseline".into()),
            id => id.to_owned(),
        };
        let key = (
            with.clone(),
            shown.id.clone(),
            self.game_type.clone(),
            shown.done(),
        );
        if self.diff_key.as_ref() == Some(&key) {
            return false;
        }
        let result = match self.runs.iter().find(|r| r.id == with) {
            None => Err(if with == "baseline" {
                "no baseline yet: no run has a match that ended".to_owned()
            } else {
                format!("no run {with}")
            }),
            Some(base) if base.id == shown.id => Err(format!(
                "{} is the run shown: nothing to compare it with",
                base.id
            )),
            Some(base) => compare_runs(base, shown, Some(self.game_type.as_str())),
        };
        self.diff = Some(result);
        self.diff_key = Some(key);
        true
    }

    /// Takes `run`'s result for `game_type`: by default the one already on
    /// show for this run, else its first played.
    pub fn show(&mut self, run: &Run, game_type: Option<&str>) {
        let played = run.played();
        let game_type = game_type
            .or_else(|| {
                played
                    .iter()
                    .copied()
                    .find(|g| self.run == run.id && *g == self.game_type)
            })
            .or_else(|| played.first().copied())
            .unwrap_or("")
            .to_owned();
        let key = Key {
            run: run.id.clone(),
            game_type: game_type.clone(),
            done: run.done(),
        };
        let mut changed = false;
        if self.key.as_ref() != Some(&key) {
            self.measures = run.measures(&game_type);
            self.key = Some(key);
            changed = true;
        }
        let state = run.state.name();
        let partial = run.state == State::Running || run.done() < run.total();
        changed |= self.state != state
            || self.stale != run.stale
            || self.partial != partial
            || self.coarse != run.coarse();
        self.run.clone_from(&run.id);
        self.build.clone_from(&run.build);
        self.game_type = game_type;
        self.state = state;
        self.stale = run.stale;
        self.coarse = run.coarse();
        self.partial = partial;
        self.seeds = run.seeds.len();
        changed |= self.refresh_diff();
        if changed {
            self.rev += 1;
        }
    }

    /// Drops the result.
    pub fn clear(&mut self) {
        if self.key.is_some() || self.measures.is_some() {
            *self = Shown {
                rev: self.rev + 1,
                ..Shown::default()
            };
        }
    }

    /// There is a result to draw or list.
    pub fn is_shown(&self) -> bool {
        self.key.is_some()
    }
}
