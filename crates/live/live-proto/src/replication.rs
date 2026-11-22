//! The entity schema agreed before a distributed session admits a peer.

/// Explicit version for the stock entity definitions.
pub const STOCK: u16 = 1;
/// Slot 17 carries the position-only creature probe.
pub const CREATURE_PROBE: u16 = 2;
/// Reserved for the full creature definition.
pub const CREATURE_FULL: u16 = 3;

/// The schema this process installed in its entity registry.
pub const fn local(creature_probe_enabled: bool) -> u16 {
    if creature_probe_enabled {
        CREATURE_PROBE
    } else {
        STOCK
    }
}

/// A peer either omitted the version or announced a different entity schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("replication_version_mismatch (local {local}, peer {peer:?})")]
pub struct IncompatibleVersion {
    pub local: u16,
    pub peer: Option<u16>,
}

/// Require an explicit matching schema on either side of admission.
pub fn require(local: u16, peer: Option<u16>) -> Result<(), IncompatibleVersion> {
    if local != 0 && peer == Some(local) {
        Ok(())
    } else {
        Err(IncompatibleVersion { local, peer })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replication_versions_are_explicit_and_exact() {
        assert_eq!(local(false), STOCK);
        assert_eq!(local(true), CREATURE_PROBE);
        for version in [STOCK, CREATURE_PROBE, CREATURE_FULL] {
            assert_eq!(require(version, Some(version)), Ok(()));
            for peer in [None, Some(0), Some(STOCK), Some(CREATURE_PROBE), Some(CREATURE_FULL)] {
                if peer != Some(version) {
                    assert_eq!(
                        require(version, peer),
                        Err(IncompatibleVersion { local: version, peer }),
                    );
                }
            }
        }
        assert!(require(0, Some(0)).is_err());
        assert!(require(STOCK, None).unwrap_err().to_string().starts_with("replication_version_mismatch"));
    }
}
