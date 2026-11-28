//! Before the game picker, converted missions used the retail checkpoint
//! filename. Identify that file by its saved launch options, so choosing
//! Halo 2 first cannot make it resume the other game.
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use content::campaign_list::{Built, BuiltLevel};
use guest_layout::GuestMemory;

use super::{CampaignProgress, State};
use crate::skulls::{LOCAL_PLAYERS, SLOT_SIGNED_IN, SLOT_STRIDE, SLOTS};

pub fn filename(key: &str) -> String {
    let hash = content::Sha256::of(key.as_bytes()).to_string();
    format!("cp-{}.bin", &hash[..32])
}

// 0x18fc44 supplies the slot's file id. 0x216800 resolves it to a
// 64-byte save record; its first 20 bytes are the profile directory.
fn profile_directory(m: &dyn GuestMemory) -> Option<(String, String)> {
    let mut slots = (0..LOCAL_PLAYERS)
        .filter(|i| m.u32(SLOTS + i * SLOT_STRIDE).unwrap_or(0) & SLOT_SIGNED_IN != 0);
    let slot = slots.next()?;
    if slots.next().is_some() {
        return None;
    }
    let profile = crate::skulls::slot_name(&crate::layout_glue::ReadGm(m), slot)?;
    let id = m.u32(SLOTS + slot * SLOT_STRIDE + 0x1f8).ok()?;
    if id == u32::MAX || id & (1 << 21) != 0 {
        return None;
    }
    let cache = m.u32(0x0051_ea14).ok()?;
    let record = if cache != 0 {
        let bank = cache.checked_add(0xbef8 + ((id >> 4) & 15) * 0x40004)?;
        let i = (id >> 8) & 0x1fff;
        if i >= m.u32(bank).ok()? {
            return None;
        }
        bank.checked_add(4 + i * 64)?
    } else {
        if m.u8(0x0055_c14f).ok()? == 0 {
            return None;
        }
        let n = m.u32(0x0055_c160).ok()?.min(0x1000);
        (0..n)
            .map(|i| 0x0055_c164 + i * 0x44)
            .find(|&at| m.u32(at) == Ok(id))?
            + 4
    };
    let path = m.cstr(record, 19).ok()?;
    let path = std::str::from_utf8(&path).ok()?;
    let folder = path.get(3..)?.trim_end_matches('\\');
    if !path.get(..3)?.eq_ignore_ascii_case("U:\\")
        || folder.len() != 12
        || !folder.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some((profile, folder.to_owned()))
}

// 0x124520 takes the options at header+0x130. The menu's completion
// reads their map at +0x18 and difficulty at +0x12a (0x2b17c3).
fn identity<'a>(
    file: &mut File,
    built: &'a [Built],
) -> io::Result<Option<(&'a Built, &'a BuiltLevel, u16)>> {
    let mut bytes = [0; 0x25c];
    file.read_exact(&mut bytes)?;
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    if word(0x130) != 1 || matches!(word(0x144), 0 | 1 | u32::MAX) {
        return Ok(None);
    }
    let path = &bytes[0x14c..0x250];
    let path = path.split(|&v| v == 0).next().unwrap_or_default();
    let difficulty = u16::from_le_bytes(bytes[0x25a..0x25c].try_into().unwrap());
    if difficulty > 3 {
        return Ok(None);
    }
    let mut matches = built.iter().flat_map(|g| {
        g.levels
            .iter()
            .filter(move |l| l.scenario.as_bytes().eq_ignore_ascii_case(path))
            .map(move |l| (g, l, difficulty))
    });
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    if let Some(second) = matches.next() {
        // Manifests can explicitly reference the same retail scenario.
        // Only the saved campaign id can disambiguate those records.
        Ok([first, second]
            .into_iter()
            .chain(matches)
            .find(|(g, _, _)| g.campaign_id == word(0x144)))
    } else {
        Ok(Some(first))
    }
}

// Publish a separate inode. A subsequent retail save writes in place.
fn copy_once(source: &mut File, dest: &Path) -> io::Result<()> {
    if dest.try_exists()? {
        return Ok(());
    }
    const SIZE: u64 = 0x3fe000 + 20; // 0x2ad7c0, signed by the retail writer
    if source.metadata()?.len() < SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected checkpoint size",
        ));
    }
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let temp = dest.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut out = File::create_new(&temp)?;
    let result = (|| {
        source.rewind()?;
        if io::copy(&mut source.take(SIZE), &mut out)? != SIZE {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "checkpoint changed during copy",
            ));
        }
        out.sync_all()?;
        drop(out);
        match std::fs::hard_link(&temp, dest) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            result => result,
        }
    })();
    let _ = std::fs::remove_file(temp);
    result
}

/// Whether this profile's retail slot actually contains an imported campaign.
/// Its signed bytes remain intact. Repeated visits preserve newer saves.
pub fn recover(s: &State, m: &dyn GuestMemory) -> bool {
    let Some(root) = s
        .progress
        .path
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::parent)
    else {
        return false;
    };
    let Some((profile, folder)) = profile_directory(m) else {
        return false;
    };
    let dir = root.join("disk/Partition1/UDATA/4D530064").join(folder);
    let Ok(mut old) = File::open(dir.join("auxilary.bin")) else {
        return false;
    };
    let Ok(Some((game, _, _))) = identity(&mut old, &s.built) else {
        return false;
    };
    if s.progress
        .get(&profile, &game.key)
        .is_some_and(|p| p.last_map.is_some())
    {
        return true;
    }
    let dest = dir.join(filename(&game.key));
    let result = copy_once(&mut old, &dest).and_then(|()| {
        let mut saved = File::open(&dest)?;
        let (g, level, difficulty) = identity(&mut saved, &s.built)?
            .filter(|(g, _, _)| g.key == game.key)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "unrecognized scoped checkpoint")
            })?;
        s.progress.with(|f| {
            let p =
                f.0.entry(profile.clone())
                    .or_default()
                    .entry(g.key.clone())
                    .or_insert_with(CampaignProgress::fresh);
            if p.last_map.is_some() {
                return ((), false);
            }
            p.last = i64::from(level.map_id);
            p.last_map = Some(level.map.clone());
            p.difficulty = Some(difficulty);
            p.checkpoint = true;
            ((), true)
        });
        Ok(())
    });
    if let Err(e) = result {
        eprintln!(
            "campaigns: could not preserve {profile}'s old {} checkpoint: {e}",
            game.key
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::campaigns::{Progress, Selection};
    use content::campaign_list::{self as cl, CampaignManifest, HALO1};
    use guest_layout::testing::FakeGuest;
    use std::io::Write;

    #[test]
    fn the_old_shared_checkpoint_can_be_resumed_without_linking_future_writes() {
        let mut guest = FakeGuest::new();
        let (slot, cache) = (SLOTS + SLOT_STRIDE, 0x10000);
        guest
            .put_u32(slot, SLOT_SIGNED_IN)
            .put_u32(slot + 0x1f8, 0)
            .put_u32(0x0051_ea14, cache)
            .put_u32(cache + 0xbef8, 1)
            .put(cache + 0xbefc, b"U:\\0123456789AB\\\0");
        guest.put(slot + crate::skulls::SLOT_NAME, &[67, 0, 0, 0]);
        assert_eq!(
            profile_directory(&guest),
            Some(("C".into(), "0123456789AB".into()))
        );
        let serial = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("h2-picker-save-{}-{serial}", std::process::id()));
        let dir = root.join("disk/Partition1/UDATA/4D530064/0123456789AB");
        std::fs::create_dir_all(&dir).unwrap();
        let progress_path = root.join("campaigns/progress.toml");
        let state = State {
            built: cl::build(&[CampaignManifest::parse(HALO1).unwrap()], &[1], &[1]).unwrap(),
            progress: Progress::load(progress_path.clone()).0,
            selection: Selection::default(),
            cards: Default::default(),
        };
        let old = dir.join("auxilary.bin");
        let level = &state.built[0].levels[1];
        let mut header = [0u8; 0x25c];
        header[0x130] = 1;
        header[0x144] = 2;
        header[0x14c..0x14c + level.scenario.len()].copy_from_slice(level.scenario.as_bytes());
        header[0x25a] = 3;
        let mut f = File::create(&old).unwrap();
        f.write_all(&header).unwrap();
        f.set_len(0x3fe000 + 20).unwrap();
        drop(f);
        assert!(recover(&state, &guest));
        assert!(recover(&state, &guest));
        let p = Progress::load(progress_path).0.get("C", "halo1").unwrap();
        assert_eq!(p.last_map.as_deref(), Some("a30"));
        assert_eq!(p.difficulty, Some(3));
        assert!(p.checkpoint);
        // The next Halo 2 save overwrites its old file in place.
        header[0x144] = 1;
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .write_all(&header)
            .unwrap();
        assert!(!recover(&state, &guest));
        let mut saved = File::open(dir.join(filename("halo1"))).unwrap();
        assert_eq!(
            identity(&mut saved, &state.built).unwrap().unwrap().1.map,
            "a30"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
