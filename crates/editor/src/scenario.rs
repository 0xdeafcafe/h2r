//! The scenario editor (`crates/scenario-edit`) as a shell [`Tool`].
//!
//! Picking a map in the sidebar opens it here, unless the open map has
//! unsaved edits: then the pick is held until they are saved or undone.
//! Saving always asks for a path (the source map is never overwritten);
//! that is the `.map` export. Map packages (docs/maps.md) are saved and
//! opened from the panel's own "Save as package..." and "Open package..."
//! windows, in `<data>/maps` (`--data`, default the runtime's).

use std::path::{Path, PathBuf};

use scenario_edit::{MapSource as EditSource, ScenarioEditor};

use crate::source::MapSource;
use crate::tool::{Tool, ToolContext, ToolError};

pub struct ScenarioTool {
    editor: ScenarioEditor,
    /// The sidebar map currently loaded, by name.
    loaded: Option<String>,
    /// The file last saved to.
    path: Option<PathBuf>,
}

impl Default for ScenarioTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ScenarioTool {
    pub fn new() -> Self {
        Self {
            editor: ScenarioEditor::new(),
            loaded: None,
            path: None,
        }
    }

    pub fn editor(&self) -> &ScenarioEditor {
        &self.editor
    }
    pub fn editor_mut(&mut self) -> &mut ScenarioEditor {
        &mut self.editor
    }

    /// Packages are saved to and opened from `<root>/maps`.
    pub fn set_data_root(&mut self, root: PathBuf) {
        self.editor.set_data_root(root);
    }

    /// The scenario crate's source for sidebar map `name`.
    pub fn edit_source(source: &MapSource, name: &str) -> anyhow::Result<EditSource> {
        let (path, region) = source.location(name)?;
        Ok(EditSource::region(path, region))
    }

    /// Follow the sidebar's pick. Returns an error message for the status
    /// line when the map cannot be opened.
    fn follow_selection(&mut self, cx: &mut ToolContext<'_>) {
        let (Some(source), Some(name)) = (cx.source, cx.selected_map) else {
            return;
        };
        if self.loaded.as_deref() == Some(name) || self.editor.is_dirty() {
            return;
        }
        let name = name.to_owned();
        let opened = Self::edit_source(source, &name).and_then(|s| {
            self.editor
                .open_named(s, &name)
                .map_err(anyhow::Error::from)
        });
        match opened {
            Ok(()) => {
                self.path = None;
                cx.info(format!("opened {name}"));
            }
            Err(e) => cx.error(format!("{name}: {e}")),
        }
        // Remember the pick either way, so a failing map is not retried
        // every frame.
        self.loaded = Some(name);
    }
}

impl Tool for ScenarioTool {
    fn title(&self) -> &str {
        "Scenario"
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolContext<'_>) {
        self.follow_selection(cx);
        if self.editor.is_dirty()
            && let (Some(pick), Some(open)) = (cx.selected_map, self.loaded.as_deref())
            && pick != open
        {
            ui.label(format!(
                "{open} has unsaved edits: save or undo them to open {pick}."
            ));
        }
        match cx.render_state {
            Some(render) => self.editor.ui(ui, render),
            None => {
                ui.label("The scenario editor needs the wgpu renderer.");
            }
        }
    }

    fn is_dirty(&self) -> bool {
        self.editor.is_dirty()
    }

    fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn extensions(&self) -> &[&str] {
        &["map"]
    }

    fn open(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        self.editor
            .open(EditSource::file(path))
            .map_err(|e| ToolError::Invalid(e.to_string()))?;
        self.loaded = cx.selected_map.map(str::to_owned);
        self.path = None;
        Ok(())
    }

    /// Always a fresh path: overwriting a map in place is refused.
    fn save(&mut self, _cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        Err(ToolError::NoPath)
    }

    fn save_as(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        let n = self
            .editor
            .save_as(path)
            .map_err(|e| ToolError::Invalid(e.to_string()))?;
        self.path = Some(path.to_owned());
        cx.info(format!("saved {} ({n} bytes)", path.display()));
        Ok(())
    }

    fn can_undo(&self) -> bool {
        self.editor.can_undo()
    }
    fn can_redo(&self) -> bool {
        self.editor.can_redo()
    }
    fn undo(&mut self) -> bool {
        self.editor.undo()
    }
    fn redo(&mut self) -> bool {
        self.editor.redo()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_source_maps_to_whole_file() {
        let dir = std::env::temp_dir().join(format!("h2-editor-scn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("demo.map");
        std::fs::write(&p, b"x").unwrap();
        let src = MapSource::Loose(p.clone());
        let s = ScenarioTool::edit_source(&src, "demo").unwrap();
        assert_eq!(s, EditSource::file(&p));
        assert!(ScenarioTool::edit_source(&src, "other").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn packages_and_variants_share_the_data_root() {
        let mut t = ScenarioTool::new();
        assert_eq!(
            t.editor().maps_dir(),
            content::store::default_data_root().join("maps")
        );
        t.set_data_root(PathBuf::from("/d"));
        assert_eq!(t.editor().maps_dir(), Path::new("/d/maps"));
        assert_eq!(
            crate::variants::VariantTool::save_root(Some(Path::new("/d"))),
            Path::new("/d/disk/Partition1/UDATA/4D530064")
        );
        assert_eq!(
            crate::variants::VariantTool::save_root(None),
            variants::save::SaveStore::default_root().unwrap()
        );
    }

    #[test]
    fn save_always_asks_for_a_path_and_nothing_is_dirty_at_start() {
        let mut t = ScenarioTool::new();
        let mut status = None;
        let mut cx = ToolContext {
            source: None,
            selected_map: None,
            render_state: None,
            status: &mut status,
        };
        assert!(matches!(t.save(&mut cx), Err(ToolError::NoPath)));
        assert!(!t.is_dirty());
        assert!(!t.can_undo());
    }
}
