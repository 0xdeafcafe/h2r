//! The game variant editor (`crates/variants`) as a shell [`Tool`].
//!
//! Built the first time it is drawn, and again when the opened disc
//! changes: the setting schema comes from the disc's `mainmenu` map (our
//! own fallback model without one), and save names from the XBE
//! (`H2_XBE`, else `default.xbe` beside the disc image), through
//! `editor_core::variant_sources`. Saves go to the
//! runtime's save storage (docs/editor/variants.md); there is no file path.

use std::path::{Path, PathBuf};

use editor_core::variant_sources;
use variants::save::XbeNames;
use variants::{Schema, VariantEditor};

use crate::source::MapSource;
use crate::tool::{Handoff, Tool, ToolContext, ToolError};

#[derive(Default)]
pub struct VariantTool {
    editor: Option<VariantEditor>,
    /// The source path the editor was built from (`None`: no source).
    built_for: Option<Option<PathBuf>>,
    /// The runtime data root (`--data`); `None` is the default one.
    data_root: Option<PathBuf>,
}

impl VariantTool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Save to and list from the storage under `root` (`h2-editor --data`),
    /// as a run with the same `--data` reads it.
    pub fn set_data_root(&mut self, root: PathBuf) {
        self.data_root = Some(root);
        self.built_for = None;
    }

    /// The save storage root: `U:` under the data root.
    pub fn save_root(data_root: Option<&Path>) -> PathBuf {
        variant_sources::save_root(data_root)
    }

    /// The schema from the source's main menu map, else the fallback.
    /// The bool says whether it came from the disc.
    pub fn schema(source: Option<&MapSource>) -> (Schema, bool) {
        variant_sources::schema(source)
    }

    /// The XBE to read save names from.
    pub fn xbe_path(source: Option<&MapSource>) -> Option<PathBuf> {
        variant_sources::xbe_path(source)
    }

    pub fn names(source: Option<&MapSource>) -> Option<XbeNames> {
        variant_sources::names(source)
    }

    fn build(&mut self, cx: &mut ToolContext<'_>) {
        let (editor, note) = variant_sources::editor(cx.source, self.data_root.as_deref());
        self.editor = Some(editor);
        self.built_for = Some(cx.source.map(|s| s.path().to_owned()));
        if let Some(note) = note {
            cx.info(note);
        }
    }
}

impl Tool for VariantTool {
    fn title(&self) -> &str {
        "Game variants"
    }

    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolContext<'_>) {
        let now = cx.source.map(|s| s.path().to_owned());
        let stale = self.built_for.as_ref() != Some(&now);
        if self.editor.is_none() || (stale && !self.is_dirty()) {
            self.build(cx);
        }
        if let Some(e) = self.editor.as_mut() {
            e.ui(ui);
        }
    }

    fn is_dirty(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.is_dirty())
    }

    fn path(&self) -> Option<&Path> {
        None
    }

    fn save(&mut self, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        let e = self
            .editor
            .as_mut()
            .ok_or(ToolError::Unsupported("saving before the editor is open"))?;
        e.save().map_err(|e| ToolError::Invalid(e.to_string()))?;
        cx.info("variant saved");
        Ok(())
    }

    fn save_as(&mut self, _path: &Path, _cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        Err(ToolError::Unsupported(
            "save as: variants save to the game's storage; use Export",
        ))
    }

    fn can_undo(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.can_undo())
    }
    fn can_redo(&self) -> bool {
        self.editor.as_ref().is_some_and(|e| e.can_redo())
    }
    fn undo(&mut self) -> bool {
        self.editor.as_mut().is_some_and(|e| e.undo())
    }
    fn redo(&mut self) -> bool {
        self.editor.as_mut().is_some_and(|e| e.redo())
    }

    /// Opens a variant from another tool (the hopper tool's "Edit in
    /// variant editor") as a new, unsaved variant, the way Import does.
    /// Refused while the open variant has unsaved changes.
    fn receive(&mut self, work: Handoff, cx: &mut ToolContext<'_>) -> Result<(), Handoff> {
        let Handoff::EditVariant(v) = &work;
        if self.editor.is_none() {
            self.build(cx);
        }
        let e = self.editor.as_mut().expect("built");
        if e.is_dirty() {
            cx.error("the variant editor has unsaved changes: save or revert them first");
            return Err(work);
        }
        match e.import(&variants::portable::export(v)) {
            Ok(()) => {
                cx.info(format!("editing a copy of {:?}", v.name()));
                Ok(())
            }
            Err(err) => {
                cx.error(format!("cannot open the variant: {err}"));
                Err(work)
            }
        }
    }
}

#[cfg(test)]
impl VariantTool {
    /// Uses `editor` as if it had been built for the source at `source`
    /// (the guide's screenshots, with a made-up schema).
    pub(crate) fn use_editor(&mut self, editor: VariantEditor, source: Option<&Path>) {
        self.editor = Some(editor);
        self.built_for = Some(source.map(Path::to_owned));
    }

    pub(crate) fn editor_mut(&mut self) -> Option<&mut VariantEditor> {
        self.editor.as_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_from_the_fallback_without_a_disc_and_draws() {
        let mut t = VariantTool::new();
        let mut status = None;
        let ctx = egui::Context::default();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut cx = ToolContext {
                source: None,
                selected_map: None,
                render_state: None,
                status: &mut status,
            };
            t.ui(ui, &mut cx);
        })
        .drop_without_applying_deltas();
        assert!(t.editor.is_some());
        assert!(!t.is_dirty());
        assert!(!t.can_undo());
        assert!(matches!(status, Some(crate::tool::Status::Info(_))));
    }

    #[test]
    fn receives_a_variant_as_a_new_copy_unless_dirty() {
        let mut t = VariantTool::new();
        let mut status = None;
        let mut cx = ToolContext {
            source: None,
            selected_map: None,
            render_state: None,
            status: &mut status,
        };
        let mut v = variants::GameVariant::blank(variants::GameType::Slayer);
        v.set_name("From hopper").unwrap();
        v.set_builtin_flag(true);
        t.receive(Handoff::EditVariant(Box::new(v.clone())), &mut cx)
            .unwrap();
        let e = t.editor.as_ref().unwrap();
        let got = e.current().unwrap();
        assert_eq!(got.name(), "From hopper");
        assert!(!got.builtin_flag(), "an imported copy is not a built-in");
        assert_eq!(e.origin(), Some(&variants::panel::Origin::New));
        // Now it has unsaved changes: a second handoff is refused, intact.
        assert!(t.is_dirty());
        let back = t.receive(Handoff::EditVariant(Box::new(v.clone())), &mut cx);
        assert_eq!(back, Err(Handoff::EditVariant(Box::new(v))));
        assert!(matches!(status, Some(crate::tool::Status::Error(_))));
    }

    #[test]
    fn save_as_is_not_offered() {
        let mut t = VariantTool::new();
        let mut status = None;
        let mut cx = ToolContext {
            source: None,
            selected_map: None,
            render_state: None,
            status: &mut status,
        };
        assert!(matches!(
            t.save_as(Path::new("x"), &mut cx),
            Err(ToolError::Unsupported(_))
        ));
    }
}
