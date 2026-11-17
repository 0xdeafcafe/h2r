//! Halo 1 models and animations as Halo 2 tags.
//!
//! - [`h1`] reads Halo 1 Xbox `mode` (render model) and `antr` (animation
//!   graph) tags out of a cache file, through [`h1map::H1Map`].
//! - [`mode::convert_model`] turns a model into a Halo 2 `mode` tag.
//! - [`jmad::convert_graph`] turns an animation graph into a Halo 2 `jmad`.
//!
//! Both return a [`Converted`]: a `map::build` block tree plus its raw
//! resources, which [`Converted::install`] adds to a [`map::build::TagSet`].
//! [`model_from_map`] and [`graph_from_map`] are the one-call forms.
//!
//! Layouts, evidence and the name table: `docs/formats/h1-models.md`. No
//! game data is in this crate or its tests; real maps are read only by the
//! `#[ignore]` tests, from `$H1_MAPS`.
#![forbid(unsafe_code)]
// Strides here are format constants; indexed loops mirror the layouts.
#![allow(
    clippy::chunks_exact_to_as_chunks,
    clippy::needless_range_loop,
    clippy::type_complexity
)]

pub mod compressed;
pub mod h1;
pub mod jmad;
pub mod mode;
pub mod names;
mod out;

pub use out::{Converted, SELF_ID};

/// Conversion failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("halo 1 map: {0}")]
    H1(#[from] h1map::Error),
    #[error("map builder: {0}")]
    Build(#[from] map::Error),
    #[error("truncated {0}")]
    Truncated(&'static str),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("tag is a {0}, want {1}")]
    WrongGroup(String, &'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

fn tag_address(map: &h1map::H1Map, tag: &h1map::Tag, group: &'static [u8; 4]) -> Result<u32> {
    if tag.group != h1map::Group::new(group) {
        return Err(Error::WrongGroup(
            tag.group.to_string(),
            std::str::from_utf8(group).unwrap_or("?"),
        ));
    }
    map.tag_data(tag)?;
    Ok(tag.data_address)
}

/// The last path component of a tag name: `characters\cyborg\cyborg` ->
/// `cyborg`.
pub fn short_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Reads and converts the Halo 1 `mode` tag `tag` of `map`.
pub fn model_from_map(
    map: &h1map::H1Map,
    tag: &h1map::Tag,
    opts: &mode::ModelOptions,
) -> Result<(h1::Model, Converted)> {
    let model = h1::Model::read(map, tag_address(map, tag, b"mode")?)?;
    let out = mode::convert_model(&model, short_name(&tag.name), opts)?;
    Ok((model, out))
}

/// Reads and converts the Halo 1 `antr` tag `tag` of `map`. `model` is the
/// `mode` tag of the object it animates; graphs without a node block of
/// their own (vehicles, scenery) need it for their skeleton.
pub fn graph_from_map(
    map: &h1map::H1Map,
    tag: &h1map::Tag,
    model: Option<&h1map::Tag>,
) -> Result<(h1::Graph, jmad::GraphReport, Converted)> {
    let graph = h1::Graph::read(map, tag_address(map, tag, b"antr")?)?;
    let skeleton = match model {
        Some(t) if graph.nodes.is_empty() => Some(h1::AnimNode::from_model(&h1::Model::read(
            map,
            tag_address(map, t, b"mode")?,
        )?)),
        _ => None,
    };
    let (out, report) = jmad::convert_graph(&graph, skeleton.as_deref())?;
    Ok((graph, report, out))
}

#[cfg(test)]
mod tests;
