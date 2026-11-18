//! Hot iteration on the agent API (docs/graphics/hot-iteration.md):
//! `edit.*` (live tag edits through the runtime's memory) and `render.*`
//! (the host renderer's WGSL overrides, parameters and toggles, on
//! `hot_render::global()`, which `d3d::nv2a` reads once a frame).
//!
//! Overrides come from `$H2_RENDER_OVERRIDES`, else the cache directory's
//! `render-overrides`, watched for changes. Tag names come from the loaded
//! map's file, found through the VFS as telemetry finds it.

use std::sync::Arc;

use crate::telemetry::MapFile;

/// Finds a map's file by its short name.
pub type MapLocator = Box<dyn Fn(&str) -> Option<MapFile> + Send + Sync>;

/// Adds the hot methods to `agent` and starts watching the override
/// directory.
pub fn install(agent: &agent::Agent, rt: &Arc<runtime::Runtime>, maps: Option<MapLocator>) {
    let hub = hot_render::global();
    agent::hot::add_render_methods(agent, hub);
    let r = agent::hot::watch_overrides(hub, agent_proto::hot::default_override_dir());
    eprintln!(
        "render overrides: {} ({} loaded{}; docs/graphics/hot-iteration.md)",
        r.dir.as_deref().unwrap_or("-"),
        r.loaded.len(),
        if r.problems.is_empty() {
            String::new()
        } else {
            format!(
                ", {} not taken: render.overrides says why",
                r.problems.len()
            )
        }
    );
    let mut editor = hot_tags::Editor::new();
    if let Some(locate) = maps {
        editor = editor.with_names(Box::new(move |g| names(g, &locate)));
    }
    let rt = Arc::clone(rt);
    agent::hot::add_edit_methods(
        agent,
        editor,
        Arc::new(move |f: &mut dyn FnMut(&mut runtime::Guest)| f(&mut rt.memory())),
    );
}

/// The names of the map loaded in `g`, from its file.
fn names(g: &runtime::Guest, locate: &MapLocator) -> Option<hot_tags::Names> {
    let name = crate::telemetry::loaded_map_name(g).ok()?;
    if name.is_empty() {
        return None;
    }
    let file = locate(&name)?;
    let reader = std::fs::File::open(&file.path).ok()?;
    let map = map::Map::from_region(std::io::BufReader::new(reader), file.offset, file.len).ok()?;
    hot_tags::Names::from_map(&map)
}
