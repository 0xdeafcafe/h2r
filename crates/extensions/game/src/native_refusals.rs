//! Bounded, per-present-window diagnostics for native draw refusals.
use std::collections::BTreeMap;
use std::sync::Mutex;

type Reasons = BTreeMap<(&'static str, &'static str), u64>;

pub(crate) struct Counts(Mutex<Reasons>);

impl Counts {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(BTreeMap::new()))
    }

    pub(crate) fn count(&self, producer: &'static str, reason: &'static str) {
        let mut reasons = self.0.lock().unwrap_or_else(|e| e.into_inner());
        *reasons.entry((producer, reason)).or_default() += 1;
    }

    fn take_summary(&self) -> Option<String> {
        let reasons = std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()));
        if reasons.is_empty() {
            return None;
        }
        let total: u64 = reasons.values().sum();
        let counts = reasons
            .into_iter()
            .map(|((producer, reason), count)| format!("{producer}/{reason}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!("refused {total} ({counts})"))
    }

    pub(crate) fn report(&self, label: &str) {
        if let Some(summary) = self.take_summary() {
            eprintln!("{label}: {summary}; D3D retained");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_window_has_one_summary_with_counts_by_producer_and_reason() {
        let counts = Counts::new();
        assert_eq!(counts.take_summary(), None);
        for _ in 0..32 {
            counts.count("Particle", "Geometry");
        }
        counts.count("Particle", "MissingDepthTarget");
        counts.count("Display", "Geometry");
        assert_eq!(
            counts.take_summary().as_deref(),
            Some("refused 34 (Display/Geometry=1, Particle/Geometry=32, Particle/MissingDepthTarget=1)")
        );
        assert_eq!(counts.take_summary(), None);
        counts.count("production", "target 13 is unresolved");
        assert_eq!(
            counts.take_summary().as_deref(),
            Some("refused 1 (production/target 13 is unresolved=1)")
        );
    }
}
