//! A fake sim for the runner's tests: it takes the batch, ends matches
//! when the test says and answers with invented traces.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use sim_bench::metrics::KillKind;
use sim_bench::spatial::{Event, Header, Party, Place, TICK_RATE, Trace};

use super::{Batch, Built, Finished, JobId, Source, Versions};

/// What the fake saw and what it will answer; shared with the test.
#[derive(Default)]
pub struct World {
    pub dir: PathBuf,
    pub build_key: String,
    pub map: String,
    pub parallel: usize,
    pub image: String,
    pub bots: String,
    pub fail_build: Option<String>,
    pub fail_nav: Option<String>,
    pub builds: u32,
    pub navs: Vec<String>,
    pub batches: Vec<Batch>,
    pub cancels: u32,
    /// Matches that end at the next poll.
    pub ended: Vec<Finished>,
    /// Kills in each match's trace.
    pub kills: usize,
    /// A seed adds its remainder by three to the kills, so the seeds
    /// have a spread.
    pub jitter: bool,
}

#[derive(Clone)]
pub struct Fake(Arc<Mutex<World>>);

impl Fake {
    pub fn new(dir: PathBuf) -> Fake {
        Fake(Arc::new(Mutex::new(World {
            dir,
            build_key: "b1".into(),
            map: "invented".into(),
            parallel: 2,
            image: "img1".into(),
            bots: "bots1".into(),
            kills: KILLS,
            ..World::default()
        })))
    }

    /// The shared state.
    pub fn w(&self) -> MutexGuard<'_, World> {
        self.0.lock().unwrap()
    }

    /// Ends `job` with a trace.
    pub fn end(&self, job: &JobId) {
        let (kills, jitter) = {
            let w = self.w();
            (w.kills, w.jitter)
        };
        self.w().ended.push(Finished {
            job: job.clone(),
            wall_s: 10.0,
            trace: Ok(trace(job, kills, jitter)),
        });
    }

    /// Ends `job` with a failure.
    pub fn fail(&self, job: &JobId, why: &str) {
        self.w().ended.push(Finished {
            job: job.clone(),
            wall_s: 1.0,
            trace: Err(why.into()),
        });
    }

    /// Ends every job of the newest batch.
    pub fn end_all(&self) {
        let jobs = self.w().batches.last().unwrap().jobs.clone();
        for j in &jobs {
            self.end(j);
        }
    }

    pub fn jobs(&self) -> Vec<JobId> {
        self.w().batches.last().unwrap().jobs.clone()
    }
}

impl Source for Fake {
    fn dir(&self) -> PathBuf {
        self.w().dir.clone()
    }
    fn versions(&self) -> Versions {
        let w = self.w();
        Versions {
            bots: w.bots.clone(),
            image: w.image.clone(),
        }
    }
    fn parallel(&self) -> usize {
        self.w().parallel
    }
    fn build(&mut self) -> Result<Built, String> {
        let mut w = self.w();
        if let Some(e) = &w.fail_build {
            return Err(e.clone());
        }
        w.builds += 1;
        Ok(Built {
            key: w.build_key.clone(),
            map: w.map.clone(),
        })
    }
    fn nav(&mut self, build: &str) -> Result<(), String> {
        let mut w = self.w();
        if let Some(e) = &w.fail_nav {
            return Err(e.clone());
        }
        w.navs.push(build.to_owned());
        Ok(())
    }
    fn start(&mut self, batch: &Batch) -> Result<(), String> {
        self.w().batches.push(batch.clone());
        Ok(())
    }
    fn poll(&mut self) -> Vec<Finished> {
        std::mem::take(&mut self.w().ended)
    }
    fn cancel(&mut self) {
        self.w().cancels += 1;
    }
}

fn place(x: f32, poly: u32) -> Place {
    Place {
        pos: [x, 1.0, 0.0],
        poly: Some(poly),
    }
}

fn party(player: u8, at: Place) -> Party {
    Party {
        player,
        team: None,
        at: Some(at),
        weapon: None,
    }
}

/// `KILLS` kills of a match two game minutes long; `seed` moves the
/// victims; the sightlines have a segment per kill. Every other measure has
/// too few events.
pub const KILLS: usize = 12;

pub fn trace(job: &JobId, kills: usize, jitter: bool) -> Trace {
    let kills = kills + if jitter { (job.seed % 3) as usize } else { 0 };
    let events = (0..kills)
        .map(|i| Event::Kill {
            tick: 100 + 10 * i as u32,
            killer: Some(party(0, place(0.0, 0))),
            victim: party(
                1,
                place(
                    (i as u64 + job.seed) as f32,
                    ((i as u64 + job.seed) % 3) as u32,
                ),
            ),
            kind: KillKind::Kill,
        })
        .collect();
    Trace {
        header: Header {
            map: "invented".into(),
            game_type: job.game_type.clone(),
            seed: job.seed,
            start_tick: 0,
            end_tick: (120.0 * TICK_RATE) as u32,
            ..Header::default()
        },
        events,
    }
}
