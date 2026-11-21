//! The retail Live browser's filters and the host's discovery metadata.

use live_proto::session::{BrowserFilters, BrowserMetadata, MapId};
use runtime::Guest;
use std::sync::Arc;

/// A host's current lobby settings, read with guest memory held. None means
/// that the console has no multiplayer lobby to describe.
pub type BrowserProbe = Arc<dyn Fn(&Guest, Option<MapId>) -> Option<BrowserMetadata> + Send + Sync>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowserStatus {
    /// Variant names from the reader's visible custom browser candidates.
    pub variants: Vec<String>,
    /// Number of rows delivered for the active filters, after local map gating.
    /// None while the search is pending or when its result is unavailable.
    pub matching: Option<usize>,
    /// None before a server answers, false when an older server ignores filters.
    pub filtering_supported: Option<bool>,
}

#[derive(Default)]
pub(crate) struct Browser {
    filters: BrowserFilters,
    generation: u64,
    status: BrowserStatus,
}

impl Browser {
    pub fn set_filters(&mut self, filters: BrowserFilters) {
        if self.filters != filters {
            self.filters = filters;
            self.generation = self.generation.wrapping_add(1);
            self.status.matching = None;
        }
    }

    pub fn request(&self) -> (u64, BrowserFilters) {
        (self.generation, self.filters.clone())
    }

    pub fn current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    pub fn complete(&mut self, generation: u64, count: usize, variants: Option<Vec<String>>) {
        if self.current(generation) {
            let supported = variants.is_some();
            self.status.filtering_supported = Some(supported);
            self.status.matching =
                (supported || self.filters == BrowserFilters::default()).then_some(count);
            self.status.variants = variants.unwrap_or_default();
        }
    }

    pub fn result(&self) -> (Option<usize>, Option<bool>) {
        (self.status.matching, self.status.filtering_supported)
    }

    pub fn status(&self) -> BrowserStatus {
        self.status.clone()
    }

    pub fn signed_out(&mut self) {
        self.status = BrowserStatus::default();
        self.generation = self.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_filter_revisions_discard_old_results() {
        let mut browser = Browser::default();
        let (initial, _) = browser.request();
        browser.complete(initial, 2, Some(vec!["Short Team Night".into()]));
        assert_eq!(browser.status().matching, Some(2));

        let filters = BrowserFilters {
            has_space: true,
            ..Default::default()
        };
        browser.set_filters(filters.clone());
        let (current, active) = browser.request();
        assert_eq!(active, filters);
        assert!(!browser.current(initial));
        assert_eq!(browser.status().matching, None);
        assert_eq!(browser.status().variants, ["Short Team Night"]);
        browser.complete(initial, 99, Some(vec!["Stale".into()]));
        assert_eq!(browser.status().matching, None);

        browser.complete(current, 0, Some(Vec::new()));
        browser.set_filters(filters.clone());
        assert_eq!(browser.request(), (current, filters.clone()));
        assert_eq!(browser.status().matching, Some(0));

        browser.signed_out();
        assert_eq!(browser.status(), BrowserStatus::default());
        assert_eq!(browser.request().1, filters);
        assert!(!browser.current(current));
    }

    #[test]
    fn older_servers_do_not_report_an_unfiltered_count_as_a_filtered_result() {
        let mut browser = Browser::default();
        browser.complete(browser.request().0, 3, None);
        assert_eq!(browser.status().matching, Some(3));
        browser.set_filters(BrowserFilters {
            has_space: true,
            ..Default::default()
        });
        browser.complete(browser.request().0, 3, None);
        assert_eq!(browser.status().matching, None);
        assert_eq!(browser.status().filtering_supported, Some(false));
    }
}
