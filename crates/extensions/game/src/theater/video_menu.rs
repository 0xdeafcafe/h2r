//! The Video screen during a film: the film's settings are applied to the
//! run, the guest draws at the viewer's size, and a bounce-search fork
//! carries the Resolution choice.
use super::Session;
use crate::run_opts::NativeTarget as ReadTarget;
use crate::setting_defs::RESOLUTION;
use anyhow::{Result, ensure};
use settings::Store;
use std::sync::Arc;

impl Session {
    pub fn video_menu_size(
        self: &Arc<Self>,
        native: ReadTarget,
        store: &Arc<Store>,
    ) -> Result<ReadTarget> {
        let state = self.lock();
        if let Some(play) = &state.play {
            // The simulation settings the film was recorded with are
            // applied to this run, and the guest draws at the viewer's size.
            let values = play.start().settings.values.clone();
            drop(state);
            let _ = self.video_menu.set(store.clone());
            apply_settings(store, &values);
            return Ok(native);
        }
        if state.header.version < 2 || state.header.meta.source != film::Source::Local {
            return Ok(native);
        }
        drop(state);
        ensure!(
            self.video_menu.set(store.clone()).is_ok(),
            "duplicate film Video menu"
        );
        Ok(native)
    }
}

/// The film's simulation-scope settings, forced for this run: the run
/// decides them (`Store::force`) and the player's file is not written.
fn apply_settings(store: &Store, values: &std::collections::BTreeMap<String, i64>) {
    for def in store.defs() {
        if def.scope != settings::Scope::Sim {
            continue;
        }
        let Some(&bits) = values.get(def.key) else {
            continue;
        };
        let value = match store.get(def.key) {
            Some(settings::Value::Bool(_)) => settings::Value::Bool(bits != 0),
            _ => settings::Value::Int(bits),
        };
        if let Err(error) = store.force(def.key, value, "the film's settings") {
            eprintln!("theater: could not set {} to the film's value: {error}", def.key);
        }
    }
}

// A bounce search fork can jump over a guest Resolution change: carry the
// current saved choice across it.
pub(super) fn snapshot(
    store: Option<&Arc<Store>>,
    live: impl Fn() -> Option<String> + Send + Sync + 'static,
) -> hooks::SnapshotPolicy {
    let Some(store) = store else {
        return hooks::SnapshotPolicy::forbid_while(live);
    };
    let (save, load) = (store.clone(), store.clone());
    hooks::SnapshotPolicy::section_forbid_while(
        live,
        move || {
            RESOLUTION
                .name(
                    save.state(RESOLUTION.key)
                        .expect("registered Resolution")
                        .saved,
                )
                .into_bytes()
        },
        move |bytes, _| restore_setting(&load, bytes),
    )
}

fn restore_setting(store: &Store, bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid keyframe Video setting")?;
    let saved = RESOLUTION.parse(text)?;
    let outcome = store
        .set(RESOLUTION.key, saved)
        .map_err(|e| e.to_string())?;
    if outcome.state.saved != saved {
        return Err("keyframe differs from the locked Video setting".into());
    }
    if let Some(error) = outcome.not_saved {
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use settings::Resolution;

    #[test]
    fn keyframe_restores_an_editable_guest_choice() {
        let store = Store::defaults(crate::setting_defs::ALL);
        restore_setting(&store, b"3440x1440").unwrap();
        assert_eq!(
            store.get(RESOLUTION.key),
            Some(Resolution::Fixed(3440, 1440).value())
        );
        assert!(restore_setting(&store, b"9000x1080").is_err());
    }
}
