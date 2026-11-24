//! The differential: a view's Rust visibility against the guest's, as
//! sets (docs/graphics/native-visibility.md, "The differential").

use std::fmt;

/// A view's visible clusters and listed objects, each ascending and once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VisibilitySets {
    pub clusters: Vec<u16>,
    pub objects: Vec<u32>,
}

impl VisibilitySets {
    pub fn new(mut clusters: Vec<u16>, mut objects: Vec<u32>) -> Self {
        clusters.sort_unstable();
        clusters.dedup();
        objects.sort_unstable();
        objects.dedup();
        Self { clusters, objects }
    }
}

/// The first difference, clusters before objects, each ascending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mismatch {
    /// The guest sees the cluster; Rust does not.
    ClusterGuestOnly(u16),
    /// Rust sees the cluster; the guest does not.
    ClusterRustOnly(u16),
    ObjectGuestOnly(u32),
    ObjectRustOnly(u32),
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClusterGuestOnly(c) => write!(f, "cluster {c} guest only"),
            Self::ClusterRustOnly(c) => write!(f, "cluster {c} rust only"),
            Self::ObjectGuestOnly(h) => write!(f, "object {h:#010x} guest only"),
            Self::ObjectRustOnly(h) => write!(f, "object {h:#010x} rust only"),
        }
    }
}

/// How the two sets of one view compare.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Parity {
    pub clusters_guest_only: u32,
    pub clusters_rust_only: u32,
    pub objects_guest_only: u32,
    pub objects_rust_only: u32,
    pub first: Option<Mismatch>,
}

impl Parity {
    /// The sets are equal: the Rust ones may stand for the guest's.
    pub fn holds(&self) -> bool {
        self.first.is_none()
    }
}

/// Walks two ascending lists, calling `only(true, x)` for each element only
/// in `guest` and `only(false, x)` for each only in `rust`.
fn walk<T: Ord + Copy>(rust: &[T], guest: &[T], mut only: impl FnMut(bool, T)) {
    let (mut i, mut j) = (0, 0);
    while i < rust.len() || j < guest.len() {
        match (rust.get(i), guest.get(j)) {
            (Some(&r), Some(&g)) if r == g => {
                i += 1;
                j += 1;
            }
            (Some(&r), Some(&g)) if r < g => {
                only(false, r);
                i += 1;
            }
            (_, Some(&g)) => {
                only(true, g);
                j += 1;
            }
            (Some(&r), None) => {
                only(false, r);
                i += 1;
            }
            (None, None) => break,
        }
    }
}

pub fn compare(rust: &VisibilitySets, guest: &VisibilitySets) -> Parity {
    let mut p = Parity::default();
    walk(&rust.clusters, &guest.clusters, |guest_only, c| {
        let m = if guest_only {
            p.clusters_guest_only += 1;
            Mismatch::ClusterGuestOnly(c)
        } else {
            p.clusters_rust_only += 1;
            Mismatch::ClusterRustOnly(c)
        };
        p.first.get_or_insert(m);
    });
    walk(&rust.objects, &guest.objects, |guest_only, h| {
        let m = if guest_only {
            p.objects_guest_only += 1;
            Mismatch::ObjectGuestOnly(h)
        } else {
            p.objects_rust_only += 1;
            Mismatch::ObjectRustOnly(h)
        };
        p.first.get_or_insert(m);
    });
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_sets_hold_whatever_order_they_were_listed_in() {
        let rust = VisibilitySets::new(vec![3, 1, 2], vec![0x20001, 0x10002]);
        let guest = VisibilitySets::new(vec![2, 3, 1, 3], vec![0x10002, 0x20001]);
        let p = compare(&rust, &guest);
        assert!(p.holds());
        assert_eq!(p, Parity::default());
    }

    #[test]
    fn a_deliberate_mismatch_is_counted_and_the_first_named() {
        let rust = VisibilitySets::new(vec![1, 2, 5], vec![0x10002, 0x10004]);
        let guest = VisibilitySets::new(vec![1, 3, 5, 9], vec![0x10002, 0x10003]);
        let p = compare(&rust, &guest);
        assert!(!p.holds());
        assert_eq!((p.clusters_guest_only, p.clusters_rust_only), (2, 1));
        assert_eq!((p.objects_guest_only, p.objects_rust_only), (1, 1));
        assert_eq!(p.first, Some(Mismatch::ClusterRustOnly(2)));
        assert_eq!(p.first.unwrap().to_string(), "cluster 2 rust only");
        let objects_only = compare(
            &VisibilitySets::new(vec![1], vec![0x10004]),
            &VisibilitySets::new(vec![1], vec![0x10003, 0x10004]),
        );
        assert_eq!(
            objects_only.first.unwrap().to_string(),
            "object 0x00010003 guest only"
        );
    }
}
