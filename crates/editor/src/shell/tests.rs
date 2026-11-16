use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::*;

/// What the fake tool was asked to do.
#[derive(Default)]
struct Log {
    opened: Vec<PathBuf>,
    saved: Vec<PathBuf>,
    undos: usize,
}

struct Fake {
    title: String,
    dirty: bool,
    path: Option<PathBuf>,
    fail_save: bool,
    log: Rc<RefCell<Log>>,
}

impl Fake {
    fn new(title: &str, log: &Rc<RefCell<Log>>) -> Self {
        Fake {
            title: title.into(),
            dirty: false,
            path: None,
            fail_save: false,
            log: log.clone(),
        }
    }
}

impl Tool for Fake {
    fn title(&self) -> &str {
        &self.title
    }
    fn ui(&mut self, ui: &mut egui::Ui, cx: &mut ToolContext<'_>) {
        ui.label(format!("map {:?}", cx.selected_map));
    }
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    fn extensions(&self) -> &[&str] {
        &["fake"]
    }
    fn open(&mut self, path: &Path, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        self.log.borrow_mut().opened.push(path.into());
        self.path = Some(path.into());
        self.dirty = false;
        cx.info("opened");
        Ok(())
    }
    fn save(&mut self, cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        let p = self.path.clone().ok_or(ToolError::NoPath)?;
        self.save_as(&p, cx)
    }
    fn save_as(&mut self, path: &Path, _cx: &mut ToolContext<'_>) -> Result<(), ToolError> {
        if self.fail_save {
            return Err(ToolError::Invalid("bad edit".into()));
        }
        self.log.borrow_mut().saved.push(path.into());
        self.path = Some(path.into());
        self.dirty = false;
        Ok(())
    }
    fn undo(&mut self) -> bool {
        self.log.borrow_mut().undos += 1;
        true
    }
}

fn shell_with(fake: Fake) -> Shell {
    let mut s = Shell::new();
    s.add_tool(Box::new(fake));
    s
}

#[test]
fn opening_over_a_clean_tool_needs_no_prompt() {
    let log = Rc::default();
    let mut s = shell_with(Fake::new("a", &log));
    s.request_open("x.fake".into());
    assert!(s.prompt().is_none());
    assert_eq!(log.borrow().opened, [PathBuf::from("x.fake")]);
    assert_eq!(s.status(), Some(&Status::Info("opened".into())));
}

#[test]
fn unsaved_changes_prompt_before_open() {
    let log = Rc::default();
    let mut f = Fake::new("a", &log);
    f.dirty = true;
    f.path = Some("old.fake".into());
    let mut s = shell_with(f);

    s.request_open("new.fake".into());
    assert_eq!(
        s.prompt(),
        Some(&Prompt::Unsaved {
            then: Pending::Open("new.fake".into())
        })
    );
    s.answer(Answer::Cancel);
    assert!(s.prompt().is_none());
    assert!(log.borrow().opened.is_empty());

    s.request_open("new.fake".into());
    s.answer(Answer::Save);
    assert_eq!(log.borrow().saved, [PathBuf::from("old.fake")]);
    assert_eq!(log.borrow().opened, [PathBuf::from("new.fake")]);
}

#[test]
fn discard_opens_without_saving() {
    let log = Rc::default();
    let mut f = Fake::new("a", &log);
    f.dirty = true;
    let mut s = shell_with(f);
    s.request_open("new.fake".into());
    s.answer(Answer::Discard);
    assert!(log.borrow().saved.is_empty());
    assert_eq!(log.borrow().opened.len(), 1);
}

#[test]
fn save_without_a_path_asks_for_one_then_continues() {
    let log = Rc::default();
    let mut f = Fake::new("a", &log);
    f.dirty = true;
    let mut s = shell_with(f);
    s.request_open("next.fake".into());
    s.answer(Answer::Save);
    let Some(Prompt::Path {
        purpose: PathPurpose::SaveAs,
        text,
        then,
    }) = s.prompt().cloned()
    else {
        panic!("wanted a Save As prompt, got {:?}", s.prompt());
    };
    assert_eq!(text, "untitled.fake");
    assert_eq!(then, Some(Pending::Open("next.fake".into())));
    if let Some(Prompt::Path { text, .. }) = &mut s.prompt {
        *text = " chosen.fake ".into();
    }
    s.submit_path();
    assert_eq!(log.borrow().saved, [PathBuf::from("chosen.fake")]);
    assert_eq!(log.borrow().opened, [PathBuf::from("next.fake")]);
}

#[test]
fn a_failed_save_keeps_the_file_and_reports() {
    let log = Rc::default();
    let mut f = Fake::new("a", &log);
    f.dirty = true;
    f.path = Some("old.fake".into());
    f.fail_save = true;
    let mut s = shell_with(f);
    s.request_open("new.fake".into());
    s.answer(Answer::Save);
    assert!(log.borrow().opened.is_empty());
    assert!(matches!(s.status(), Some(Status::Error(m)) if m.contains("bad edit")));
}

#[test]
fn quitting_asks_when_any_tool_is_dirty() {
    let log = Rc::default();
    let mut s = shell_with(Fake::new("clean", &log));
    assert!(s.request_quit(), "nothing dirty: close now");

    let mut s = Shell::new();
    s.add_tool(Box::new(Fake::new("clean", &log)));
    let mut d = Fake::new("dirty", &log);
    d.dirty = true;
    d.path = Some("d.fake".into());
    s.add_tool(Box::new(d));
    assert!(!s.request_quit());
    assert!(!s.wants_quit());
    s.answer(Answer::Cancel);
    assert!(!s.wants_quit());
    assert!(!s.request_quit());
    s.answer(Answer::Save);
    assert_eq!(log.borrow().saved, [PathBuf::from("d.fake")]);
    assert!(s.wants_quit());
    assert!(s.request_quit(), "already agreed");
}

#[test]
fn quitting_stays_open_when_a_save_fails() {
    let log = Rc::default();
    let mut d = Fake::new("dirty", &log);
    d.dirty = true;
    let mut s = shell_with(d);
    assert!(!s.request_quit());
    s.answer(Answer::Save);
    assert!(!s.wants_quit(), "no path: cannot save, so not closed");
    assert!(matches!(s.status(), Some(Status::Error(m)) if m.contains("not closed")));
    assert!(!s.request_quit());
    s.answer(Answer::Discard);
    assert!(s.wants_quit());
}

#[test]
fn undo_goes_to_the_active_tool_and_tools_switch() {
    let log_a: Rc<RefCell<Log>> = Rc::default();
    let log_b: Rc<RefCell<Log>> = Rc::default();
    let mut s = Shell::new();
    s.add_tool(Box::new(Fake::new("a", &log_a)));
    s.add_tool(Box::new(Fake::new("b", &log_b)));
    s.select_tool(1);
    s.undo();
    s.select_tool(7);
    assert_eq!(s.active(), 1);
    s.redo();
    assert!(matches!(s.status(), Some(Status::Info(m)) if m == "nothing to redo"));
    assert_eq!((log_a.borrow().undos, log_b.borrow().undos), (0, 1));
}

#[test]
fn a_bad_source_path_is_reported_and_keeps_none() {
    let mut s = Shell::new();
    s.open_source("/nonexistent/disc.iso".into());
    assert!(s.source().is_none());
    assert!(matches!(s.status(), Some(Status::Error(_))));
    s.ask_path(PathPurpose::Source);
    assert!(matches!(
        s.prompt(),
        Some(Prompt::Path {
            purpose: PathPurpose::Source,
            ..
        })
    ));
}

/// Draws whole frames headless (no window, no GPU): the shell with the
/// hopper tool on a synthetic file, then with the path and unsaved prompts.
#[test]
fn frames_draw_headless() {
    let mut s = Shell::new();
    let mut hopper = crate::hopper::HopperTool::new();
    let bytes = hopper_edit::fixture::sample().build();
    hopper.open_bytes(&bytes, None).unwrap();
    s.add_tool(Box::new(hopper));
    let log = Rc::default();
    let mut f = Fake::new("fake", &log);
    f.dirty = true;
    s.add_tool(Box::new(f));
    let ctx = egui::Context::default();
    let frame = |s: &mut Shell| {
        ctx.run_ui(egui::RawInput::default(), |ui| s.ui(ui))
            .drop_without_applying_deltas();
    };
    frame(&mut s);
    frame(&mut s);
    s.select_tool(1);
    s.ask_path(PathPurpose::Open);
    frame(&mut s);
    s.cancel_prompt();
    s.request_open("x.fake".into());
    frame(&mut s);
    assert!(matches!(s.prompt(), Some(Prompt::Unsaved { .. })));
    assert!(!s.wants_quit());
}

// ---------- the file dialog ----------

use crate::dialog::{Picked, ScriptedDialog};

fn with_dialog(s: &mut Shell, answers: Vec<Picked>) -> Rc<RefCell<Vec<crate::dialog::Request>>> {
    let d = ScriptedDialog::new(answers);
    let asked = d.asked.clone();
    s.set_file_dialog(Box::new(d));
    asked
}

#[test]
fn open_uses_the_dialog_with_the_tools_extensions() {
    let log = Rc::default();
    let mut f = Fake::new("a", &log);
    f.path = Some("/somewhere/old.fake".into());
    let mut s = shell_with(f);
    let asked = with_dialog(&mut s, vec![Picked::Path("picked.fake".into())]);
    s.ask_path(PathPurpose::Open);
    assert!(s.prompt().is_none(), "no typed prompt");
    assert_eq!(log.borrow().opened, [PathBuf::from("picked.fake")]);
    let asked = asked.borrow();
    assert!(!asked[0].save);
    assert_eq!(asked[0].filters[0].extensions, ["fake"]);
    assert_eq!(asked[0].start, Some("/somewhere/old.fake".into()));
}

#[test]
fn a_cancelled_dialog_does_nothing() {
    let log = Rc::default();
    let mut s = shell_with(Fake::new("a", &log));
    with_dialog(&mut s, vec![Picked::Cancelled, Picked::Cancelled]);
    s.ask_path(PathPurpose::Open);
    s.ask_save_as(None);
    assert!(s.prompt().is_none());
    assert!(log.borrow().opened.is_empty() && log.borrow().saved.is_empty());
    assert!(s.status().is_none());
}

#[test]
fn an_unavailable_dialog_falls_back_to_typing() {
    let log = Rc::default();
    let mut s = shell_with(Fake::new("a", &log));
    with_dialog(&mut s, vec![]);
    s.ask_save_as(None);
    assert_eq!(
        s.prompt(),
        Some(&Prompt::Path {
            purpose: PathPurpose::SaveAs,
            text: "untitled.fake".into(),
            then: None
        })
    );
    s.cancel_prompt();
    s.ask_path(PathPurpose::Open);
    assert!(matches!(
        s.prompt(),
        Some(Prompt::Path {
            purpose: PathPurpose::Open,
            ..
        })
    ));
}

#[test]
fn save_without_a_path_asks_the_dialog_then_continues() {
    let log = Rc::default();
    let mut f = Fake::new("a", &log);
    f.dirty = true;
    let mut s = shell_with(f);
    let asked = with_dialog(&mut s, vec![Picked::Path("/out/saved.fake".into())]);
    s.request_open("next.fake".into());
    s.answer(Answer::Save);
    assert!(s.prompt().is_none());
    assert_eq!(log.borrow().saved, [PathBuf::from("/out/saved.fake")]);
    assert_eq!(log.borrow().opened, [PathBuf::from("next.fake")]);
    let asked = asked.borrow();
    assert!(asked[0].save);
    assert_eq!(asked[0].start, Some("untitled.fake".into()));
    assert_eq!(asked[0].filters[0].extensions, ["fake"]);
}

#[test]
fn the_source_dialog_offers_discs_and_maps() {
    let mut s = Shell::new();
    let asked = with_dialog(&mut s, vec![Picked::Path("/nonexistent/disc.iso".into())]);
    s.ask_path(PathPurpose::Source);
    assert!(s.prompt().is_none());
    assert!(s.source().is_none());
    assert!(
        matches!(s.status(), Some(Status::Error(_))),
        "tried to open it"
    );
    let asked = asked.borrow();
    assert_eq!(asked[0].filters[0].extensions, ["iso", "xiso", "map"]);
    assert_eq!(asked[0].start, None);
}

#[test]
fn a_tool_without_extensions_gets_no_filter() {
    let mut s = Shell::new();
    s.add_tool(Box::new(crate::variants::VariantTool::new()));
    let asked = with_dialog(&mut s, vec![Picked::Cancelled]);
    s.ask_path(PathPurpose::Open);
    assert!(asked.borrow()[0].filters.is_empty());
}
