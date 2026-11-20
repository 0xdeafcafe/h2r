//! Republishes title files built before the game's header was traced
//! (docs/live/live-motd.md §2.4). Those carry 16 zero bytes where the game's
//! validator wants a CRC, the class version and the size, so it rejects
//! them and shows no MOTD / keeps its built-in playlists. At startup each
//! such file is republished with a real header and the same body, through
//! the same server-signed publish as any other.

use live_proto::hopper::{self, HALO_2_HOPPER_FILE, HopperFile};
use live_proto::motd::{self, Motd};
use live_proto::{ErrorCode, HALO_2_MOTD_FILE, HALO_2_TITLE_ID, title_file};

use crate::app::Live;

/// Returns the names republished.
pub async fn reseal_legacy(live: &Live) -> anyhow::Result<Vec<&'static str>> {
    let mut done = Vec::new();
    for (name, version) in [
        (HALO_2_MOTD_FILE, motd::VERSION),
        (HALO_2_HOPPER_FILE, hopper::VERSION),
    ] {
        let bytes = match live.file(HALO_2_TITLE_ID, name).await {
            Ok((_, b)) => b,
            Err(e) if e.code == ErrorCode::NotFound => continue,
            Err(e) => anyhow::bail!("reading {name}: {}", e.message),
        };
        if !title_file::is_legacy(&bytes) {
            continue;
        }
        // Only a body the game would take is worth a header.
        let parses = if name == HALO_2_MOTD_FILE {
            Motd::parse_legacy(&bytes).is_ok()
        } else {
            HopperFile::parse_legacy(&bytes).is_ok()
        };
        if !parses {
            continue;
        }
        let sealed = title_file::seal(version, &bytes[title_file::HEADER..]);
        crate::admin::publish(live, HALO_2_TITLE_ID, name, &sealed)
            .await
            .map_err(|e| anyhow::anyhow!("republishing {name}: {}", e.message))?;
        done.push(name);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ServerIdentity, admin, store::MemoryStore};
    use std::sync::Arc;

    #[tokio::test]
    async fn zero_headers_are_republished_with_the_same_body() {
        let dir = std::env::temp_dir().join(format!(
            "h2-reseal-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let identity = ServerIdentity::open(&dir, None).unwrap();
        let live = Live::start(Arc::new(MemoryStore::new()), identity, Default::default())
            .await
            .unwrap();
        let m = Motd {
            header: "old".into(),
            messages: vec!["published before the header".into()],
        };
        let mut old_motd = m.build().unwrap();
        old_motd[..16].fill(0);
        let mut old_hopper = HopperFile::blank().build();
        old_hopper[..16].fill(0);
        for (name, bytes) in [
            (HALO_2_MOTD_FILE, &old_motd),
            (HALO_2_HOPPER_FILE, &old_hopper),
        ] {
            admin::publish(&live, HALO_2_TITLE_ID, name, bytes)
                .await
                .unwrap();
        }
        assert_eq!(
            reseal_legacy(&live).await.unwrap(),
            vec![HALO_2_MOTD_FILE, HALO_2_HOPPER_FILE]
        );
        let (_, motd_now) = live.file(HALO_2_TITLE_ID, HALO_2_MOTD_FILE).await.unwrap();
        assert_eq!(Motd::parse(&motd_now).unwrap(), m);
        assert_eq!(&motd_now[16..], &old_motd[16..]);
        let (_, hopper_now) = live
            .file(HALO_2_TITLE_ID, HALO_2_HOPPER_FILE)
            .await
            .unwrap();
        assert!(HopperFile::parse(&hopper_now).is_ok());
        assert_eq!(&hopper_now[16..], &old_hopper[16..]);
        // Sealed files are left alone the next time.
        assert!(reseal_legacy(&live).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn bodies_that_are_not_the_format_are_left_alone() {
        let dir = std::env::temp_dir().join(format!(
            "h2-reseal-junk-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        let identity = ServerIdentity::open(&dir, None).unwrap();
        let live = Live::start(Arc::new(MemoryStore::new()), identity, Default::default())
            .await
            .unwrap();
        admin::publish(&live, HALO_2_TITLE_ID, HALO_2_HOPPER_FILE, &[0u8; 40])
            .await
            .unwrap();
        assert!(reseal_legacy(&live).await.unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
