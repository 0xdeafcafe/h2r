//! Codec symmetry and independently tokenized source node-count checks.
use crate::{Catalog, Result, Scenario, Validation, invalid, source_counts};
use std::io::{Read, Seek};

#[derive(Debug)]
pub struct Check {
    pub validation: Validation,
    pub scripts: usize,
    pub globals: usize,
    pub references: usize,
    pub emitted_nodes: usize,
}

pub fn check_map<R: Read + Seek>(map: &mut map::Map<R>, catalog: &mut Catalog) -> Result<Check> {
    let s = Scenario::read(map)?;
    let tag = map.scenario().ok_or_else(|| invalid("no scenario"))?;
    let (id, address) = (tag.id, tag.location.ok_or(map::Error::NoTagData)?.address);
    s.verify_codec_symmetry(&map.read_tag_bytes(id)?, address)?;
    let validation = s.validate(catalog)?;
    let source = s.decompile(catalog, None)?;
    let parsed = source_counts(&source.source)?;
    if parsed.nodes != source.emitted_nodes
        || parsed.scripts != s.scripts.len()
        || parsed.globals != s.globals.len()
        || source.unique_nodes != validation.rooted_nodes
    {
        return Err(invalid("source node-count check failed"));
    }
    catalog.observe(&s)?;
    Ok(Check {
        validation,
        scripts: s.scripts.len(),
        globals: s.globals.len(),
        references: s.references.len(),
        emitted_nodes: source.emitted_nodes,
    })
}
