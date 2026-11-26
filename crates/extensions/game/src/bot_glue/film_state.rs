//! Restore phantom membership and the render hook clock in a bounce-search
//! fork of a recorded session.
use super::*;
use serde::{Deserialize, Serialize};

/// The phantoms' remote membership, as the bounce search saves it.
#[derive(Clone, Serialize, Deserialize)]
struct Books {
    remote: bots::phantom::film::Roster,
}
impl Books {
    fn capture() -> Self {
        Self {
            remote: bots::phantom::global().film_roster(),
        }
    }
    fn load(&self) -> Result<(), String> {
        bots::phantom::global().restore_film_roster(&self.remote)
    }
}

#[derive(Serialize, Deserialize)]
struct State {
    books: Books,
    frame: u64,
}

pub(super) fn install(b: &mut Builder, frame: &Arc<AtomicU64>) {
    let (save, load) = (frame.clone(), frame.clone());
    b.snapshot(hooks::SnapshotPolicy::section(
        move || {
            if !crate::sim_bounces::in_fork() {
                return Vec::new();
            }
            serde_json::to_vec(&State {
                books: Books::capture(),
                frame: save.load(Ordering::Relaxed),
            })
            .expect("fixed film bot state is serialisable")
        },
        move |bytes, _| {
            if !crate::sim_bounces::in_fork() {
                return if bytes.is_empty() {
                    Ok(())
                } else {
                    Err("bot state requires a bounce-search fork".into())
                };
            }
            let state: State = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            state.books.load()?;
            load.store(state.frame, Ordering::Relaxed);
            Ok(())
        },
    ));
}
