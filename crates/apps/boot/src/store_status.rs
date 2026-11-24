//! `store.status` on the agent: the tag store at `<data>/tags`, the maps it
//! can serve and whether this boot serves them (`store-maps`;
//! docs/engine/asset-store.md, docs/runtime/agent-api.md "The tag store").
use std::path::{Path, PathBuf};

use agent::{Access, Agent};
use agent_proto::method;
use serde_json::{Value, json};

/// The tag store of a data root: Studio's default `tags_root`.
pub fn root(data_root: &Path) -> PathBuf {
    data_root.join("tags")
}

/// Register `store.status` (read, no params).
pub fn install(agent: &Agent, data_root: &Path, served: bool) {
    let root = root(data_root);
    let images = workspace::level::images_dir(data_root);
    agent.add_method(method::STORE_STATUS, Access::Read, move |_| {
        Ok(report(&root, &images, served))
    });
}

/// The reply: the store's root, whether it is a tag repository, whether
/// this boot serves the disc's maps from it, every map it can serve, the
/// recorded maps it cannot (no skeleton), the maps whose files do not
/// read, and the level images under `images` (`<data>/store/images`).
pub fn report(root: &Path, images: &Path, served: bool) -> Value {
    let st = vfs::store_map::status(root);
    let imgs = workspace::level::images(images);
    json!({
        "root": root.display().to_string(),
        "exists": st.exists,
        "served": served,
        "maps": st.maps.iter().map(|m| json!({
            "map": m.map,
            "length": m.len,
            "sha256": m.file,
            "skeleton_bytes": m.skeleton_file,
            "extents": m.extents.0,
            "extent_bytes": m.extents.1,
        })).collect::<Vec<_>>(),
        "stale": st.stale,
        "broken": st.broken.iter().map(|(m, why)| json!({"map": m, "why": why})).collect::<Vec<_>>(),
        "images": imgs.iter().map(|i| json!({
            "level_id": i.id.to_string(),
            "length": i.len,
            "sha256": i.file,
            "disk_bytes": i.disk,
            "extents": i.extents.0,
            "extent_bytes": i.extents.1,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_root_with_no_store_reports_none() {
        let d = std::env::temp_dir().join(format!("boot-store-status-{}", std::process::id()));
        let v = report(&root(&d), &workspace::level::images_dir(&d), false);
        assert_eq!(v["exists"], false);
        assert_eq!(v["served"], false);
        assert_eq!(v["maps"], json!([]));
        assert_eq!(v["images"], json!([]));
    }
}
