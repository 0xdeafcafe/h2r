//! The game variant editor (`crates/variants`) as a shell [`Tool`].
//!
//! Built the first time it is drawn, and again when the opened disc
//! changes: the setting schema comes from the disc's `mainmenu` map (our
//! own fallback model without one), and save names from the XBE
//! (`H2_XBE`, else `default.xbe` beside the disc image). Saves go to the
//! runtime's save storage (docs/variants.md); there is no file path.

use std::path::{Path, PathBuf};

use variants::save::{SaveStore, XbeNames};
use variants::{Schema, VariantEditor};

use crate::source::MapSource;
use crate::tool::{Tool, ToolContext, ToolError};

const LANGUAGE: tags::Language = tags::Language::English;

#[derive(Default)]
pub struct VariantTool {
    editor: Option<VariantEditor>,
    /// The source path the editor was built from (`None`: no source).
    built_for: Option<Option<PathBuf>>,
}

impl VariantTool {
    pub fn new() -> Self {
        Self::default()
    }

    /// The schema from the source's main menu map, else the fallback.
    /// The bool says whether it came from the disc.
    fn schema(source: Option<&MapSource>) -> (Schema, bool) {
        let from_disc = source
            .and_then(|s| s.open_map("mainmenu").ok())
            .and_then(|mut m| Schema::from_map(&mut m, LANGUAGE).ok());
        match from_disc {
            Some(s) => (s, true),
            None => (Schema::fallback(), false),
        }
    }

    /// The XBE to read save names from.
    pub fn xbe_path(source: Option<&MapSource>) -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("H2_XBE") {
            return Some(PathBuf::from(p));
        }
        let beside = source?.path().parent()?.join("default.xbe");
        beside.is_file().then_some(beside)
    }

    fn names(source: Option<&MapSource>) -> Option<XbeNames> {
        let bytes = std::fs::read(Self::xbe_path(source)?).ok()?;
        let xbe = xbe::Xbe::parse(&bytes).ok()?;
        XbeNames::read(&xbe, LANGUAGE).ok()
    }

    fn build(&mut self, cx: &mut ToolContext<'_>) {
        let (schema, from_disc) = Self::schema(cx.source);
        let names = Self::names(cx.source);
        let have_names = names.is_some();
        let store = SaveStore::default_root().map(|root| SaveStore::new(root, names));
        self.editor = Some(VariantEditor::new(schema, store));
        self.built_for = Some(cx.source.map(|s| s.path().to_owned()));
        if !from_disc {
            cx.info("no disc: using the built-in setting model");
        } else if !have_names {
            cx.info("no XBE found (set H2_XBE): saved variants can be edited, but not created");
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
