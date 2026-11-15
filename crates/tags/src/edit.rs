//! Small reviewed tag edits layered on the lossless map writer.
use crate::{Error, Result, object};
use map::{TagId, writer::MapEditor};
use std::io::{Read, Seek};

/// Set an object's bounding-sphere radius, preserving every other byte.
/// The shared object prefix stores this f32 at +4; retail `0x000be075`
/// multiplies it by object scale (see `object::layouts` and docs/objects.md).
/// Returns the original radius. Requires a known object class, finite old/new
/// values and a nonnegative new radius.
pub fn set_object_bounding_radius<R: Read + Seek>(
    editor: &mut MapEditor<'_, R>,
    id: TagId,
    radius: f32,
) -> Result<f32> {
    if !radius.is_finite() || radius < 0.0 {
        return Err(Error::Invalid(
            "bounding radius must be finite and nonnegative",
        ));
    }
    let group = editor.source().tag(id)?.group;
    if !object::is_object_group(group) {
        return Err(Error::UnsupportedGroup(group));
    }
    let bytes = editor.read_tag_range(id, 0, 8)?;
    let old = f32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if !old.is_finite() {
        return Err(Error::Invalid("nonfinite source bounding radius"));
    }
    editor.patch_tag(id, 4, &radius.to_le_bytes())?;
    Ok(old)
}
