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

/// The maps the game opens before any level: the shared caches and the
/// main menu (the cache slots 0 to 2, docs/runtime/cache-slots.md).
pub const FIRST_MAPS: [&str; 3] = ["shared", "single_player_shared", "mainmenu"];

/// Whether the disc at `disc` has map files of its own. An image is taken
/// to (its entries are read when it is mounted); a directory has them when
/// `maps/` holds a `.map` file. The copy of the disc's other files that an
/// extraction makes, `<data>/disc/` (docs/engine/asset-store.md, "The rest
/// of the disc"), has none.
pub fn disc_has_maps(disc: &Path) -> bool {
    if !disc.is_dir() {
        return true;
    }
    std::fs::read_dir(disc.join("maps"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.path().is_file()
                && e.path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("map"))
        })
}

/// Whether this boot serves `maps\*.map` from the store: when `store-maps`
/// is on, or when the disc has no maps and the store holds every one of
/// [`FIRST_MAPS`] (`stored`: the maps the store can serve). A disc with no
/// maps and a store without them is an error that names what is missing:
/// booted, the game finds no `shared.map`, fails the copy into its cache
/// slots and shows its disc error screen.
pub fn serve_from_store(
    feature: bool,
    disc_has_maps: bool,
    stored: &[&str],
    disc: &Path,
    root: &Path,
) -> anyhow::Result<bool> {
    if feature || disc_has_maps {
        return Ok(feature);
    }
    let missing: Vec<&str> = FIRST_MAPS
        .iter()
        .copied()
        .filter(|m| !stored.contains(m))
        .collect();
    anyhow::ensure!(
        missing.is_empty(),
        "the disc at {} has no maps\\*.map and the store at {} lacks {}: boot from the disc image",
        disc.display(),
        root.display(),
        missing.join(", ")
    );
    Ok(true)
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

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("boot-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("maps/fonts")).unwrap();
        d
    }

    #[test]
    fn a_disc_directory_has_maps_only_with_map_files() {
        let d = scratch("disc-maps");
        std::fs::write(d.join("default.xbe"), b"x").unwrap();
        std::fs::write(d.join("maps/fonts/conduit-12"), b"x").unwrap();
        assert!(
            !disc_has_maps(&d),
            "the store's copy of the disc's other files"
        );
        std::fs::write(d.join("maps/shared.MAP"), b"x").unwrap();
        assert!(disc_has_maps(&d));
        let iso = d.join("halo2.iso");
        std::fs::write(&iso, b"x").unwrap();
        assert!(disc_has_maps(&iso));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_disc_with_no_maps_boots_from_the_store_or_is_refused() {
        let (disc, root) = (Path::new("/d/disc"), Path::new("/d/tags"));
        let all = ["mainmenu", "shared", "single_player_shared", "zanzibar"];
        // The switch and a disc with maps decide as before.
        assert!(serve_from_store(true, true, &[], disc, root).unwrap());
        assert!(!serve_from_store(false, true, &all, disc, root).unwrap());
        // No maps on the disc: the store serves them when it has the first ones.
        assert!(serve_from_store(false, false, &all, disc, root).unwrap());
        let e = serve_from_store(false, false, &["shared", "zanzibar"], disc, root)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("single_player_shared, mainmenu") && e.contains("/d/disc"),
            "{e}"
        );
    }

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
