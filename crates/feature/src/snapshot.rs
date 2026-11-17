//! What each feature says about machine snapshots
//! (docs/runtime/snapshots.md). A feature declares it with
//! [`crate::Builder::snapshot`]; one that says nothing is
//! [`SnapshotPolicy::Unaudited`], which forbids snapshots whenever the
//! feature is installed. So a feature nobody has checked fails closed: a
//! snapshot never silently misses its state.
use std::sync::Arc;

/// Answers, between slices, why the feature cannot be snapshotted right
/// now; `None` when it can.
pub type LiveFn = dyn Fn() -> Option<String> + Send + Sync;

#[derive(Clone)]
pub enum SnapshotPolicy {
    /// Nothing outside guest RAM that a snapshot would miss: observers,
    /// rewrites whose state is all in guest memory or fixed by the boot.
    Stateless,
    /// Holds host state a snapshot cannot carry while it is live; the
    /// function says when it is.
    ForbidWhileLive(Arc<LiveFn>),
    /// Not audited: treated as live all the time.
    Unaudited,
}

impl SnapshotPolicy {
    /// [`SnapshotPolicy::ForbidWhileLive`] from a closure.
    pub fn forbid_while(live: impl Fn() -> Option<String> + Send + Sync + 'static) -> Self {
        SnapshotPolicy::ForbidWhileLive(Arc::new(live))
    }
    pub fn name(&self) -> &'static str {
        match self {
            SnapshotPolicy::Stateless => "stateless",
            SnapshotPolicy::ForbidWhileLive(_) => "forbid while live",
            SnapshotPolicy::Unaudited => "unaudited",
        }
    }
    /// Why this feature blocks a snapshot now, if it does.
    pub fn blocker(&self) -> Option<String> {
        match self {
            SnapshotPolicy::Stateless => None,
            SnapshotPolicy::ForbidWhileLive(live) => live(),
            SnapshotPolicy::Unaudited => Some("not audited for snapshots".into()),
        }
    }
}

impl std::fmt::Debug for SnapshotPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// One built feature's policy, as the plan keeps it.
#[derive(Clone, Debug)]
pub struct FeatureSnapshot {
    pub id: &'static str,
    pub policy: SnapshotPolicy,
}

impl crate::Plan {
    /// Every built feature that blocks a snapshot now: `(feature id, why)`.
    /// Call between slices.
    pub fn snapshot_blockers(&self) -> Vec<(&'static str, String)> {
        self.snapshot
            .iter()
            .filter_map(|f| f.policy.blocker().map(|why| (f.id, why)))
            .collect()
    }

    /// Names the hooked point behind a continuation's call target: a
    /// chain's shadow entry, called by [`crate::install::AFTER_HOOKS`]
    /// continuations. `None` for any other address.
    pub fn describe_target(&self, target: u32) -> Option<String> {
        let c = self.chains.values().find(|c| c.shadow == Some(target))?;
        let hooks: Vec<String> = c
            .after
            .iter()
            .chain(&c.observe_after)
            .map(|h| format!("{}:{}", h.feature, h.label))
            .collect();
        Some(format!(
            "after hooks on {} ({:#010x}) [{}]",
            c.point.name,
            c.addr,
            hooks.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::{Base, BuildError, Builder, Feature, Manifest, Registry, SnapshotPolicy, Switches};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Quiet(&'static str, Option<SnapshotPolicy>);
    impl Feature for Quiet {
        fn manifest(&self) -> Manifest {
            Manifest::new(self.0, "test")
        }
        fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
            if let Some(p) = &self.1 {
                b.snapshot(p.clone());
            }
            Ok(())
        }
    }

    #[test]
    fn features_are_unaudited_unless_they_say_otherwise() {
        let live = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&live);
        let mut r = Registry::new(Switches::default());
        r.add(&Quiet("silent", None)).unwrap();
        r.add(&Quiet("observer", Some(SnapshotPolicy::Stateless)))
            .unwrap();
        r.add(&Quiet(
            "movie",
            Some(SnapshotPolicy::forbid_while(move || {
                flag.load(Ordering::Relaxed)
                    .then(|| "a movie is playing".to_string())
            })),
        ))
        .unwrap();
        let plan = r.build(&Base).unwrap();
        assert_eq!(
            plan.snapshot_blockers(),
            vec![("silent", "not audited for snapshots".to_string())]
        );
        live.store(true, Ordering::Relaxed);
        assert_eq!(
            plan.snapshot_blockers(),
            vec![
                ("silent", "not audited for snapshots".to_string()),
                ("movie", "a movie is playing".to_string())
            ]
        );
    }

    struct Ticker;
    impl Feature for Ticker {
        fn manifest(&self) -> Manifest {
            Manifest::new("ticker", "test")
        }
        fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
            b.frame().on_tick("count", |_| {});
            b.snapshot(SnapshotPolicy::Stateless);
            Ok(())
        }
    }

    #[test]
    fn an_after_hook_continuation_target_names_its_point() {
        let mut r = Registry::new(Switches::default());
        r.add(&Ticker).unwrap();
        let plan = r.build(&Base).unwrap();
        assert!(plan.snapshot_blockers().is_empty());
        let chain = plan.chains.values().next().unwrap();
        let shadow = chain.shadow.expect("an observer after needs a shadow");
        let text = plan.describe_target(shadow).unwrap();
        assert!(text.starts_with("after hooks on "), "{text}");
        assert!(text.contains("ticker:count"), "{text}");
        assert_eq!(plan.describe_target(chain.addr), None);
    }

    #[test]
    fn switched_off_features_do_not_block() {
        let mut r = Registry::new(Switches::parse("-silent").unwrap());
        assert!(!r.add(&Quiet("silent", None)).unwrap());
        let plan = r.build(&Base).unwrap();
        assert!(plan.snapshot_blockers().is_empty());
    }
}
