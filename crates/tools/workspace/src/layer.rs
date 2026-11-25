//! `Layered`: tag sources in order, first hit wins
//! (docs/editor/tag-workspace.md, "Layers").
use map::TagGroup;
use tags::repo::{LooseTag, RawRes, Result, TagKey, TagSource};

/// A [`TagSource`] over other sources. The first layer that holds a tag
/// gives it; a key held by several is listed once, at its first layer.
#[derive(Default)]
pub struct Layered {
    layers: Vec<Box<dyn TagSource>>,
}

impl Layered {
    pub fn new() -> Self {
        Self::default()
    }

    /// `layer` goes below every layer already here.
    pub fn with(mut self, layer: impl TagSource + 'static) -> Self {
        self.layers.push(Box::new(layer));
        self
    }

    /// [`Layered::with`] for a boxed source.
    pub fn push(&mut self, layer: Box<dyn TagSource>) {
        self.layers.push(layer);
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// The layer (0 is the top) that gives `key`.
    pub fn layer_of(&self, key: &TagKey) -> Option<usize> {
        self.layers.iter().position(|l| l.keys().contains(key))
    }
}

impl TagSource for Layered {
    fn keys(&self) -> Vec<TagKey> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for layer in &self.layers {
            for key in layer.keys() {
                if seen.insert(key.clone()) {
                    out.push(key);
                }
            }
        }
        out
    }

    fn read(&mut self, key: &TagKey) -> Result<Option<LooseTag>> {
        for layer in &mut self.layers {
            if let Some(tag) = layer.read(key)? {
                return Ok(Some(tag));
            }
        }
        Ok(None)
    }

    /// The first layer whose tags name the resource and can give it.
    fn raw(&mut self, raw: &RawRes) -> Result<Vec<u8>> {
        let mut last = None;
        for layer in &mut self.layers {
            match layer.raw(raw) {
                Ok(bytes) => return Ok(bytes),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| {
            tags::repo::Error::Invalid("no layer holds this raw resource".into())
        }))
    }

    fn groups(&self) -> Vec<TagGroup> {
        let mut out: Vec<TagGroup> = Vec::new();
        for layer in &self.layers {
            for g in layer.groups() {
                if !out.iter().any(|o| o.class == g.class) {
                    out.push(g);
                }
            }
        }
        out
    }
}
