//! `h2 play`'s cloud saves (docs/live/live-cloud-saves.md): signed in to
//! Live, the account's saves are merged into U: before the game starts,
//! and what the game writes there is pushed while it runs. `H2_CLOUD_SAVES=0`
//! turns it off.

use std::path::Path;
use std::sync::Arc;

use cloud_saves::local::ProfileSettings;
use cloud_saves::session::SignedIn;
pub use cloud_saves::session::{Cloud, enabled};

use crate::play_live::Session;

/// Syncs `session`'s account's saves into `data` and starts the worker
/// ([`cloud_saves::session::start`]). Every failure is a line on stderr
/// and `None`: the game is played with the saves this machine has.
pub fn start(session: &Session, data: &Path, settings: &Arc<settings::Store>) -> Option<Cloud> {
    if !enabled(std::env::var("H2_CLOUD_SAVES").ok().as_deref()) {
        eprintln!("cloud saves: off (H2_CLOUD_SAVES); saves stay on this machine");
        return None;
    }
    let say = |line: &str| eprintln!("cloud saves: {line}");
    let Some(account) = session.account.as_ref() else {
        say("off until the account is on this machine (signed in from the game; next start)");
        return None;
    };
    let key = match account.key_pair() {
        Ok(k) => k,
        Err(e) => {
            say(&format!("off: {e:#}"));
            return None;
        }
    };
    let settings: Arc<dyn ProfileSettings> = settings.clone();
    cloud_saves::session::start(
        SignedIn {
            server: session.server.clone(),
            api: session.api.clone(),
            server_id: session.server_id.clone(),
            xuid: account.xuid,
            gamertag: account.gamertag.clone(),
            key,
        },
        data,
        Some(settings),
        say,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play_live::{LiveOpts, Places as LivePlaces};

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("h2-play-cloud-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store(data: &Path) -> Arc<settings::Store> {
        Arc::new(boot::setting_defs::load(data, &[]).0)
    }

    /// Three machines on one embedded service, without the game: the
    /// first's save is pushed at sign-in, the second gets it and writes
    /// another through a VFS (as the game would), the third gets both.
    #[test]
    fn saves_follow_the_account_between_data_roots() {
        let dir = temp("session");
        let opts = LiveOpts {
            enabled: true,
            local: true,
            ..LiveOpts::default()
        };
        let session = crate::play_live::start(
            &opts,
            live_server::MEMORY_DB,
            &LivePlaces::under(&dir),
            Default::default(),
            "test.user",
        )
        .unwrap();
        let [a, b, c] = ["a", "b", "c"].map(|m| dir.join(m));
        let u = cloud_saves::local::udata_dir;
        std::fs::create_dir_all(u(&a).join("0123456789AB")).unwrap();
        std::fs::write(u(&a).join("0123456789AB/SaveMeta.xbx"), b"Name=Chief").unwrap();
        drop(start(&session, &a, &store(&a)).expect("cloud saves on"));

        let cloud = start(&session, &b, &store(&b)).unwrap();
        assert_eq!(
            std::fs::read(u(&b).join("0123456789AB/SaveMeta.xbx")).unwrap(),
            b"Name=Chief"
        );
        let config = vfs::Config::new(&b, 0x4D530064);
        let mut v = vfs::Vfs::with_storage(config.clone()).unwrap();
        v.create_standard_links(&config).unwrap();
        let v = Arc::new(std::sync::Mutex::new(v));
        (cloud.on_vfs())(&v);
        {
            let mut v = v.lock().unwrap();
            let h = v
                .open(
                    "U:\\0123456789AB\\profile",
                    vfs::OpenOptions {
                        access: vfs::Access::ALL,
                        disposition: vfs::CreateDisposition::Create,
                        directory: Some(false),
                        ..Default::default()
                    },
                )
                .unwrap()
                .handle;
            v.write(h, vfs::Offset::Absolute(0), b"progress").unwrap();
            v.close(h).unwrap();
        }
        // Quitting pushes what is waiting, quiet or not.
        drop(cloud);

        drop(start(&session, &c, &store(&c)).unwrap());
        assert_eq!(
            std::fs::read(u(&c).join("0123456789AB/profile")).unwrap(),
            b"progress"
        );
        drop(v);
        drop(session);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
