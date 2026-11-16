//! Cloud saves for one signed-in account, as every host starts them:
//! `h2 play` and the tvOS and iOS apps (docs/live/live-cloud-saves.md).
//! [`start`] merges the account's saves into the data root before the game
//! reads them, then runs the push [`Worker`] while it plays.

use std::path::Path;
use std::sync::{Arc, Mutex};

use live_proto::Xuid;
use live_proto::sign::KeyPair;

use crate::engine::{Engine, Log, Worker, WorkerConfig};
use crate::local::{Local, ProfileSettings};
use crate::remote::HttpRemote;
use crate::state::Places;

/// The account to sync, and where.
pub struct SignedIn {
    /// The server's base URL, for the lines it prints.
    pub server: String,
    /// The discovery document's API base, resolved (`https://host/v1`).
    pub api: String,
    pub server_id: String,
    pub xuid: Xuid,
    pub gamertag: String,
    pub key: KeyPair,
}

/// The running push worker; dropping it pushes what is still waiting.
pub struct Cloud {
    worker: Worker<HttpRemote>,
}

/// The hook a host hands the boot (`boot::VfsReady` is this type): it
/// tells the VFS's U: changes to the worker.
pub type OnVfs = Box<dyn FnOnce(&Arc<Mutex<vfs::Vfs>>) + Send>;

impl Cloud {
    pub fn on_vfs(&self) -> OnVfs {
        let observer = self.worker.observer();
        Box::new(move |vfs| {
            vfs.lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_change_observer(Some(observer));
        })
    }
}

/// Whether `H2_CLOUD_SAVES` (`value`) leaves cloud saves on.
pub fn enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

/// Syncs `account`'s saves into `data` and starts the worker. Every line
/// goes to `log`; every failure is a line and `None`: the game is played
/// with the saves this machine has.
pub fn start(
    account: SignedIn,
    data: &Path,
    settings: Option<Arc<dyn ProfileSettings>>,
    log: impl Fn(&str) + Send + Clone + 'static,
) -> Option<Cloud> {
    let remote = match HttpRemote::new(&account.api, &account.server_id, account.xuid, account.key)
    {
        Ok(r) => r,
        Err(e) => {
            log(&format!("off: {e}"));
            return None;
        }
    };
    let local = Local::new(data, settings);
    let places = Places::new(data, &account.server_id, &account.xuid.to_string());
    let mut engine = match Engine::new(local, remote, places) {
        Ok(e) => e,
        Err(e) => {
            log(&format!("off: {e}"));
            return None;
        }
    };
    let report = engine.sync();
    if report.unsupported {
        log(&format!(
            "{} keeps none; saves stay on this machine",
            account.server
        ));
        return None;
    }
    for line in &report.lines {
        log(line);
    }
    log(&format!(
        "{} for {} on {} (state in {})",
        report.summary(),
        account.gamertag,
        account.server,
        engine.places().dir.display()
    ));
    let worker_log: Log = Box::new(log.clone());
    match Worker::spawn(engine, WorkerConfig::default(), worker_log) {
        Ok(worker) => Some(Cloud { worker }),
        Err(e) => {
            log(&format!("no uploads while playing: {e}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_variable_turns_it_off() {
        assert!(enabled(None));
        assert!(enabled(Some("1")));
        for off in ["0", "off", "OFF", " no ", "false"] {
            assert!(!enabled(Some(off)), "{off}");
        }
    }

    #[test]
    fn plain_http_to_another_machine_is_refused_before_any_request() {
        let dir = std::env::temp_dir().join(format!("h2-cloud-session-{}", std::process::id()));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let seen = lines.clone();
        let cloud = start(
            SignedIn {
                server: "http://192.0.2.1".into(),
                api: "http://192.0.2.1/v1".into(),
                server_id: "s".into(),
                xuid: Xuid(0x0009_0000_0000_0001),
                gamertag: "Chief".into(),
                key: KeyPair::generate(),
            },
            &dir,
            None,
            move |l: &str| seen.lock().unwrap().push(l.to_string()),
        );
        assert!(cloud.is_none());
        let lines = lines.lock().unwrap();
        assert!(lines[0].starts_with("off: refusing"), "{lines:?}");
        assert!(!dir.exists(), "nothing written");
    }
}
