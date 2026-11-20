//! From the VFS's change reports (`vfs::Vfs::set_change_observer`) to the
//! save containers to push, debounced: XCreateSaveGame makes a directory
//! and `SaveMeta.xbx`, then the game writes its payload, sometimes in
//! several opens, so a container is pushed once it has been quiet for a
//! moment (or has been changing for too long).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::Key;

/// The device U: is on.
pub const DATA_DEVICE: &str = "\\Device\\Harddisk0\\Partition1";

/// A container is pushed once nothing in it changed for this long...
pub const QUIET: Duration = Duration::from_secs(2);
/// ...or once it has been changing for this long.
pub const MAX_DELAY: Duration = Duration::from_secs(15);

/// U:'s path on [`DATA_DEVICE`]: `UDATA\4D530064\`.
fn udata_prefix() -> String {
    format!("UDATA\\{:08X}\\", live_proto::HALO_2_TITLE_ID.0)
}

/// The save item a change is about (`0123456789AB/SaveMeta.xbx`), or
/// `None` for anything outside U: (the cache partitions, T:).
pub fn save_name(c: &vfs::Changed) -> Option<String> {
    if !c.device.eq_ignore_ascii_case(DATA_DEVICE) {
        return None;
    }
    let prefix = udata_prefix();
    let head = c.path.get(..prefix.len())?;
    if !head.eq_ignore_ascii_case(&prefix) {
        return None;
    }
    let rest = &c.path[prefix.len()..];
    (!rest.is_empty()).then(|| rest.replace('\\', "/"))
}

/// The container a change is about, as the engine pushes them: the first
/// segment of its name.
pub fn container(c: &vfs::Changed) -> Option<Key> {
    let name = save_name(c)?;
    let top = name.split('/').next().unwrap_or(&name);
    Some(Key::save(top))
}

/// Containers waiting to be quiet.
#[derive(Debug)]
pub struct Debouncer {
    quiet: Duration,
    max: Duration,
    /// First and last change of each.
    pending: BTreeMap<Key, (Instant, Instant)>,
}

impl Default for Debouncer {
    fn default() -> Self {
        Debouncer::new(QUIET, MAX_DELAY)
    }
}

impl Debouncer {
    pub fn new(quiet: Duration, max: Duration) -> Self {
        Debouncer {
            quiet,
            max,
            pending: BTreeMap::new(),
        }
    }

    pub fn touch(&mut self, key: Key, now: Instant) {
        self.pending
            .entry(key)
            .and_modify(|(_, last)| *last = now)
            .or_insert((now, now));
    }

    /// The containers due at `now`, taken out.
    pub fn due(&mut self, now: Instant) -> Vec<Key> {
        let due: Vec<Key> = self
            .pending
            .iter()
            .filter(|(_, (first, last))| {
                now.duration_since(*last) >= self.quiet || now.duration_since(*first) >= self.max
            })
            .map(|(k, _)| k.clone())
            .collect();
        for k in &due {
            self.pending.remove(k);
        }
        due
    }

    /// Everything waiting, due or not (at exit).
    pub fn drain(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.pending).into_keys().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs::{ChangeKind, Changed};

    fn changed(device: &str, path: &str) -> Changed {
        Changed {
            device: device.into(),
            path: path.into(),
            kind: ChangeKind::Written,
        }
    }

    #[test]
    fn only_u_is_watched() {
        let c = changed(DATA_DEVICE, "UDATA\\4D530064\\0123456789AB\\SaveMeta.xbx");
        assert_eq!(save_name(&c).as_deref(), Some("0123456789AB/SaveMeta.xbx"));
        assert_eq!(container(&c), Some(Key::save("0123456789AB")));
        // Whatever case the guest used.
        let c = changed("\\device\\harddisk0\\partition1", "udata\\4d530064\\TitleMeta.xbx");
        assert_eq!(container(&c), Some(Key::save("TitleMeta.xbx")));
        for (device, path) in [
            ("\\Device\\Harddisk0\\Partition5", "UDATA\\4D530064\\x"),
            (DATA_DEVICE, "TDATA\\4D530064\\x"),
            (DATA_DEVICE, "UDATA\\4D530065\\x"),
            (DATA_DEVICE, "UDATA\\4D530064\\"),
            (DATA_DEVICE, "UDATA"),
        ] {
            assert_eq!(save_name(&changed(device, path)), None, "{device} {path}");
        }
    }

    #[test]
    fn a_container_waits_until_quiet_but_not_forever() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_millis(n);
        let mut d = Debouncer::new(Duration::from_millis(100), Duration::from_millis(1000));
        let a = Key::save("A");
        let b = Key::save("B");
        d.touch(a.clone(), s(0));
        d.touch(a.clone(), s(50));
        assert!(d.due(s(120)).is_empty(), "70 ms since the last write");
        assert_eq!(d.due(s(150)), std::slice::from_ref(&a));
        assert!(d.is_empty());
        // Written every 50 ms: pushed at the max delay anyway.
        for n in (0..=1000).step_by(50) {
            d.touch(b.clone(), s(2000 + n));
            let due = d.due(s(2000 + n));
            if n < 1000 {
                assert!(due.is_empty(), "{n}");
            } else {
                assert_eq!(due, std::slice::from_ref(&b));
            }
        }
        d.touch(a.clone(), s(5000));
        assert_eq!(d.drain(), [a]);
        assert!(d.due(s(9000)).is_empty());
    }
}
