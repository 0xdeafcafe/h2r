//! What each feature says about machine snapshots
//! (docs/runtime/snapshots.md). A feature declares it with
//! [`crate::Builder::snapshot`]; one that says nothing is
//! [`SnapshotPolicy::Unaudited`], which forbids snapshots whenever the
//! feature is installed. So a feature nobody has checked fails closed: a
//! snapshot never silently misses its state.
//!
//! A feature whose host state must survive a load says
//! [`SnapshotPolicy::Section`]: it saves bytes of its own and reads them
//! back, or re-derives its state from the loaded guest memory.
//! [`crate::Plan::save_sections`] and [`crate::Plan::load_sections`]
//! collect them; the boot puts them in the snapshot file.
use runtime::Runtime;
use std::sync::Arc;

/// Answers, between slices, why the feature cannot be snapshotted right
/// now; `None` when it can.
pub type LiveFn = dyn Fn() -> Option<String> + Send + Sync;
/// A feature's host state as bytes, between slices with nothing live.
pub type SaveFn = dyn Fn() -> Vec<u8> + Send + Sync;
/// Reads [`SaveFn`]'s bytes back after guest RAM and the kernel are
/// loaded; the runtime is there for state the feature re-derives from
/// guest memory.
pub type LoadFn = dyn Fn(&[u8], &Runtime) -> Result<(), String> + Send + Sync;

/// What [`SnapshotPolicy::Section`] holds.
pub struct Section {
    /// When the feature cannot be snapshotted at all, as
    /// [`SnapshotPolicy::ForbidWhileLive`].
    pub live: Option<Arc<LiveFn>>,
    pub save: Arc<SaveFn>,
    pub load: Arc<LoadFn>,
}

#[derive(Clone)]
pub enum SnapshotPolicy {
    /// Nothing outside guest RAM that a snapshot would miss: observers,
    /// rewrites whose state is all in guest memory or fixed by the boot.
    Stateless,
    /// Holds host state a snapshot cannot carry while it is live; the
    /// function says when it is.
    ForbidWhileLive(Arc<LiveFn>),
    /// Holds host state a snapshot saves and a load puts back.
    Section(Arc<Section>),
    /// Not audited: treated as live all the time.
    Unaudited,
}

impl SnapshotPolicy {
    /// [`SnapshotPolicy::ForbidWhileLive`] from a closure.
    pub fn forbid_while(live: impl Fn() -> Option<String> + Send + Sync + 'static) -> Self {
        SnapshotPolicy::ForbidWhileLive(Arc::new(live))
    }
    /// [`SnapshotPolicy::Section`], never live.
    pub fn section(
        save: impl Fn() -> Vec<u8> + Send + Sync + 'static,
        load: impl Fn(&[u8], &Runtime) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        SnapshotPolicy::Section(Arc::new(Section {
            live: None,
            save: Arc::new(save),
            load: Arc::new(load),
        }))
    }
    /// [`SnapshotPolicy::Section`] that also forbids snapshots while
    /// `live` says why.
    pub fn section_forbid_while(
        live: impl Fn() -> Option<String> + Send + Sync + 'static,
        save: impl Fn() -> Vec<u8> + Send + Sync + 'static,
        load: impl Fn(&[u8], &Runtime) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        let live: Arc<LiveFn> = Arc::new(live);
        SnapshotPolicy::Section(Arc::new(Section {
            live: Some(live),
            save: Arc::new(save),
            load: Arc::new(load),
        }))
    }
    pub fn name(&self) -> &'static str {
        match self {
            SnapshotPolicy::Stateless => "stateless",
            SnapshotPolicy::ForbidWhileLive(_) => "forbid while live",
            SnapshotPolicy::Section(_) => "section",
            SnapshotPolicy::Unaudited => "unaudited",
        }
    }
    /// Why this feature blocks a snapshot now, if it does.
    pub fn blocker(&self) -> Option<String> {
        match self {
            SnapshotPolicy::Stateless => None,
            SnapshotPolicy::ForbidWhileLive(live) => live(),
            SnapshotPolicy::Section(s) => s.live.as_ref().and_then(|live| live()),
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

    /// Every [`SnapshotPolicy::Section`] feature's bytes, in build order:
    /// `(feature id, bytes)`. Call between slices, after
    /// [`Self::snapshot_blockers`] came back empty.
    pub fn save_sections(&self) -> Vec<(&'static str, Vec<u8>)> {
        self.snapshot
            .iter()
            .filter_map(|f| match &f.policy {
                SnapshotPolicy::Section(s) => Some((f.id, (s.save)())),
                _ => None,
            })
            .collect()
    }

    /// Give each section feature its saved bytes back, after guest RAM
    /// and the kernel are loaded. The saved features must be exactly this
    /// plan's section features (the snapshot's build key covers the
    /// switches, so a difference means a damaged or foreign file); that is
    /// checked before any is loaded. A feature that fails stops the load
    /// there, with earlier features already loaded.
    pub fn load_sections(&self, saved: &[(String, Vec<u8>)], rt: &Runtime) -> Result<(), String> {
        let ours: Vec<(&'static str, &Section)> = self
            .snapshot
            .iter()
            .filter_map(|f| match &f.policy {
                SnapshotPolicy::Section(s) => Some((f.id, &**s)),
                _ => None,
            })
            .collect();
        let same = ours.len() == saved.len()
            && ours.iter().zip(saved).all(|((id, _), (sid, _))| *id == sid.as_str());
        if !same {
            let names = |ids: Vec<&str>| ids.join(", ");
            return Err(format!(
                "the snapshot has sections for [{}], this plan [{}]",
                names(saved.iter().map(|(id, _)| id.as_str()).collect()),
                names(ours.iter().map(|(id, _)| *id).collect())
            ));
        }
        for ((id, section), (_, bytes)) in ours.iter().zip(saved) {
            (section.load)(bytes.as_slice(), rt).map_err(|e| format!("feature {id}: {e}"))?;
        }
        Ok(())
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

    #[test]
    fn section_features_save_their_bytes_and_load_them_back() {
        use std::sync::atomic::AtomicU32;
        let count = Arc::new(AtomicU32::new(7));
        let busy = Arc::new(AtomicBool::new(false));
        let (c, b) = (Arc::clone(&count), Arc::clone(&busy));
        let saver = Arc::clone(&count);
        let loader = Arc::clone(&count);
        let mut r = Registry::new(Switches::default());
        r.add(&Quiet("observer", Some(SnapshotPolicy::Stateless)))
            .unwrap();
        r.add(&Quiet(
            "counter",
            Some(SnapshotPolicy::section_forbid_while(
                move || b.load(Ordering::Relaxed).then(|| "counting".to_string()),
                move || saver.load(Ordering::Relaxed).to_le_bytes().to_vec(),
                move |bytes, _| {
                    let v = <[u8; 4]>::try_from(bytes).map_err(|_| "not 4 bytes")?;
                    loader.store(u32::from_le_bytes(v), Ordering::Relaxed);
                    Ok(())
                },
            )),
        ))
        .unwrap();
        let plan = r.build(&Base).unwrap();
        assert!(plan.snapshot_blockers().is_empty());
        busy.store(true, Ordering::Relaxed);
        assert_eq!(
            plan.snapshot_blockers(),
            vec![("counter", "counting".to_string())]
        );
        let saved: Vec<(String, Vec<u8>)> = plan
            .save_sections()
            .into_iter()
            .map(|(id, bytes)| (id.to_string(), bytes))
            .collect();
        assert_eq!(saved, vec![("counter".to_string(), vec![7, 0, 0, 0])]);
        c.store(99, Ordering::Relaxed);

        let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
        plan.load_sections(&saved, &rt).unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 7);
        // A file whose features differ loads nothing.
        c.store(99, Ordering::Relaxed);
        let other = vec![
            ("counter".to_string(), vec![1, 0, 0, 0]),
            ("extra".to_string(), Vec::new()),
        ];
        assert!(plan.load_sections(&other, &rt).is_err());
        assert_eq!(count.load(Ordering::Relaxed), 99);
        let bad = vec![("counter".to_string(), vec![1])];
        let err = plan.load_sections(&bad, &rt).unwrap_err();
        assert!(err.contains("feature counter: not 4 bytes"), "{err}");
    }
}
