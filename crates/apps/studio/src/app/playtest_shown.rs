//! The playtest result the viewport draws and the Playtest panel lists
//! (docs/editor/playtest.md, "What is measured"; slice PT5).
//!
//! The runner lives with the API provider; each call that moves a run
//! publishes it here ([`Shown::show`]): the measures of one game type
//! over the matches that ended, and the flags the layers dim by. The
//! measures are recomputed only when a match ends or another game type
//! is asked for.

use sim_bench::spatial::Measures;

use super::playtest::{Run, State};

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
}

impl Shown {
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
