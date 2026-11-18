//! The HUD tool's commands without a window, over `hud_edit`'s invented
//! map (no game data).
use std::path::PathBuf;

use hud_edit::{Mode, Value, WidgetKey, WidgetKind};

use crate::app::{Action, Answer, App, ToolKind};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(tag: &str) -> (Temp, PathBuf, App) {
    let dir = std::env::temp_dir().join(format!("h2-editor-hud-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hudtest.map");
    std::fs::write(&path, hud_edit::fixture::build().0).unwrap();
    let mut app = App::new();
    app.set_data_root(dir.join("data"));
    app.dispatch(Action::OpenSource(path.clone()));
    app.dispatch(Action::SetTool(ToolKind::Hud));
    app.dispatch(Action::HudOpenMap("hudtest".into()));
    (Temp(dir), path, app)
}

const STRAY: WidgetKey = WidgetKey {
    kind: WidgetKind::Bitmap,
    index: 2,
};

#[test]
fn a_map_opens_with_its_first_hud_shown() {
    let (_t, _, mut app) = open("open");
    let s = app.hud.session.as_ref().unwrap();
    assert_eq!(s.huds.len(), 2);
    assert_eq!(s.hud().unwrap().name, "invented\\hud\\player");
    assert_eq!(app.title(), "hudtest / invented\\hud\\player - h2-editor");
    // Clicking the panel's pixels finds it; the warning names the stray.
    assert_eq!(
        app.hud.widget_at([60.0, 410.0]),
        Some(WidgetKey {
            kind: WidgetKind::Bitmap,
            index: 0
        })
    );
    assert_eq!(app.hud.widget_at([300.0, 300.0]), None);
    let shot = app.hud.shot().unwrap();
    assert!(shot.findings.iter().any(|f| f.widgets.contains(&(0, STRAY))));
}

#[test]
fn a_drag_is_one_undo_step_and_marks_the_tool_dirty() {
    let (_t, _, mut app) = open("drag");
    app.dispatch(Action::HudSelect(Some(STRAY)));
    for (i, x) in [41, 45, 60].into_iter().enumerate() {
        app.dispatch(Action::HudMove(STRAY, (x, -48), i > 0));
    }
    assert!(app.tool_dirty(ToolKind::Hud));
    assert!(app.title().ends_with(" * - h2-editor"));
    assert_eq!(app.hud.offset(STRAY), Some((60, -48)));
    assert!(app.can_undo());
    app.dispatch(Action::Undo);
    assert_eq!(app.hud.offset(STRAY), Some((40, -48)));
    assert!(!app.tool_dirty(ToolKind::Hud));
    app.dispatch(Action::Redo);
    assert_eq!(app.hud.offset(STRAY), Some((60, -48)));
    // A field edit, then the state: half screen shows the half offsets.
    app.dispatch(Action::HudSet(STRAY, "anchor".into(), Value::Int(3), false));
    assert_eq!(app.hud.session.as_ref().unwrap().history().count(), 2);
    let mut st = app.hud.state.clone();
    st.mode = Mode::Half;
    app.dispatch(Action::HudState(Box::new(st)));
    assert_eq!(app.hud.offset(STRAY), Some((20, -24)));
}

#[test]
fn opening_another_map_asks_about_unsaved_edits_and_save_as_writes() {
    let (t, _, mut app) = open("save");
    app.dispatch(Action::HudMove(STRAY, (40, -60), false));
    app.dispatch(Action::HudOpenMap("hudtest".into()));
    assert_eq!(app.pending_tool(), Some(ToolKind::Hud));
    let out = t.0.join("out.map");
    app.resolve(Answer::SaveAs(out.clone()));
    assert!(out.exists());
    assert!(!app.tool_dirty(ToolKind::Hud));
    assert!(app.pending().is_none(), "the open went ahead");
    // The saved map opens with the edit.
    app.dispatch(Action::OpenSource(out));
    app.dispatch(Action::HudOpenMap("out".into()));
    assert_eq!(app.hud.offset(STRAY), Some((40, -60)));
}
