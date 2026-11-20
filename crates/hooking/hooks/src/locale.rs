//! The `locale` facade: our strings as sources, not hooks.
//!
//! A feature adds a [`source`](Locale::source): a function from a packed
//! string id to the guest address of its text, `None` for ids it does not
//! own. Every source is consulted by one place:
//!
//! - **Native**: the Rust `locale_string_id_find` (`engine-leaves`, on by
//!   default for the locale functions) asks the sources before the string
//!   table. No hook is added for the strings at all.
//! - **Bridge**: when nothing native serves them (`impl.engine.locale=off`,
//!   or the implementation is missing), the plan adds one `before` hook on
//!   `LOCALE_FIND`, owned by the `locale` bridge, that asks the sources and
//!   lets every other id through to the original.
//!
//! The choice is made in [`crate::Registry::build`], after every feature has
//! declared, so the order features are added in does not matter.

use std::sync::{Arc, OnceLock};

use crate::Builder;

/// A string source: the guest address of the text for `id`, if it is ours.
pub type SourceFn = dyn Fn(u32) -> Option<u32> + Send + Sync;

/// Every source in the plan, filled when the plan is built. Cheap to clone;
/// a lookup is a walk over the sources (a handful), no lock.
#[derive(Clone, Default)]
pub struct Sources(Arc<OnceLock<Vec<Arc<SourceFn>>>>);

impl Sources {
    /// The first source's answer for `id`.
    pub fn find(&self, id: u32) -> Option<u32> {
        self.0.get()?.iter().find_map(|s| s(id))
    }
    pub(crate) fn fill(&self, sources: Vec<Arc<SourceFn>>) {
        let _ = self.0.set(sources);
    }
}

/// One declared source.
#[derive(Clone)]
pub(crate) struct Source {
    pub feature: &'static str,
    pub label: &'static str,
    pub f: Arc<SourceFn>,
}

/// See [`Builder::locale`].
pub struct Locale<'b, 'r>(pub(crate) &'b mut Builder<'r>);

impl Locale<'_, '_> {
    /// Adds a string source.
    pub fn source(
        self,
        label: &'static str,
        f: impl Fn(u32) -> Option<u32> + Send + Sync + 'static,
    ) {
        let feature = self.0.feature;
        self.0.sources.push(Source {
            feature,
            label,
            f: Arc::new(f),
        });
    }

    /// For the native locale lookup: the sources it must consult. Calling
    /// this says this feature's implementation of `LOCALE_FIND` serves them;
    /// if that implementation is not in the plan, the bridge does.
    pub fn serve(self) -> Sources {
        self.0.serves_locale = true;
        self.0.locale.clone()
    }
}
