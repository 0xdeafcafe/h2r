//! Save, Save as and Load on the map variant store, with their questions
//! and notices (docs/game/forge-map-variants-ui.md, "Saving, loading and
//! renaming").
use std::time::Duration;

use super::*;
use crate::forge_screens::variants as flows;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "h2-forge-editing-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs the file job to its end.
fn settle(e: &mut Editor, tick: u32) {
    for _ in 0..5000 {
        if e.file_job.is_none() {
            return;
        }
        e.poll_file(tick);
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("the file job did not finish");
}

fn notice(e: &Editor) -> Option<&str> {
    e.notice.as_ref().map(|(text, _)| text.as_str())
}

/// Edits the session has not saved, as far as `dirty` can tell.
fn make_dirty(e: &mut Editor) {
    let mut other = e.session.document().clone();
    other.title = "Somewhere else".into();
    e.saved_head = LogHead::genesis(&other).unwrap();
    assert!(e.dirty());
}

fn names(e: &Editor) -> Vec<&str> {
    e.variants.iter().map(|l| l.name.as_str()).collect()
}

fn answer(e: &mut Editor, tick: u32, ask: Ask, yes: bool) {
    e.choose(tick, Choice::Answer { ask, yes }).unwrap();
}

#[test]
fn an_unnamed_save_is_refused_here_and_save_as_names_it() {
    let dir = Dir::new();
    let mut e = editor_in(&dir.0);
    // GAME MENU opens the keyboard before it sends Save; one that arrives
    // without a name is refused.
    assert_eq!(e.choose(1, Choice::Save).unwrap_err(), flows::UNNAMED);
    assert!(e.filename.is_none());
    e.choose(2, Choice::SaveAs("  Arena ".into())).unwrap();
    settle(&mut e, 3);
    assert_eq!(notice(&e), Some("Saved Arena."));
    assert_eq!(e.filename.as_deref(), Some("Arena"));
    assert_eq!(names(&e), ["Arena"]);
    assert!(!e.dirty());
    // Save now writes over its own name without asking.
    make_dirty(&mut e);
    e.choose(4, Choice::Save).unwrap();
    settle(&mut e, 5);
    assert_eq!(notice(&e), Some("Saved Arena."));
    assert_eq!(names(&e), ["Arena"]);
    assert!(!e.dirty());
    assert_eq!(screens::take_request(), None);
}

#[test]
fn a_name_the_store_refuses_saves_nothing() {
    let dir = Dir::new();
    let mut e = editor_in(&dir.0);
    let too_long = "x".repeat(forge_doc::MAX_VARIANT_NAME + 1);
    assert!(e.choose(1, Choice::SaveAs(too_long)).is_err());
    assert!(e.choose(1, Choice::SaveAs("   ".into())).is_err());
    assert!(e.file_job.is_none());
    assert!(!dir.0.join("map-variants").exists());
}

#[test]
fn a_taken_name_asks_and_no_is_the_default_answer() {
    let dir = Dir::new();
    let mut e = editor_in(&dir.0);
    for name in ["Arena", "Keep"] {
        e.choose(1, Choice::SaveAs(name.into())).unwrap();
        settle(&mut e, 2);
    }
    assert_eq!(e.filename.as_deref(), Some("Keep"));
    // The open variant is Keep: another variant's name asks first.
    e.choose(3, Choice::SaveAs("arena".into())).unwrap();
    settle(&mut e, 4);
    assert_eq!(screens::take_request(), Some(Kind::Question));
    let question = screens::screen(Kind::Question).sheet();
    assert_eq!(question.body, "Replace arena?");
    assert_eq!(question.rows[question.focus].label, "No");
    assert_eq!(e.overwrite_asked.as_deref(), Some("arena"));
    assert_eq!(
        e.filename.as_deref(),
        Some("Keep"),
        "nothing is written yet"
    );
    // No: nothing is written and the notice says so.
    answer(&mut e, 5, Ask::Overwrite, false);
    assert_eq!(notice(&e), Some(flows::NOT_SAVED));
    assert!(e.overwrite_asked.is_none() && e.file_job.is_none());
    assert_eq!(e.filename.as_deref(), Some("Keep"));
    // Yes: the file that has the name is replaced, not duplicated.
    e.choose(6, Choice::SaveAs("arena".into())).unwrap();
    settle(&mut e, 7);
    assert_eq!(screens::take_request(), Some(Kind::Question));
    answer(&mut e, 8, Ask::Overwrite, true);
    settle(&mut e, 9);
    assert_eq!(notice(&e), Some("Saved arena."));
    assert_eq!(e.filename.as_deref(), Some("arena"));
    assert_eq!(e.variants.len(), 2);
    assert!(names(&e).contains(&"arena") && !names(&e).contains(&"Arena"));
}

#[test]
fn load_with_edits_asks_first_and_ends_in_its_notice() {
    let dir = Dir::new();
    let mut e = editor_in(&dir.0);
    e.choose(1, Choice::SaveAs("Arena".into())).unwrap();
    settle(&mut e, 2);
    let file = e.variants[0].file.clone();
    // No edits: the load starts at once.
    e.choose(3, Choice::Load(file.clone())).unwrap();
    assert_eq!(screens::take_request(), None);
    settle(&mut e, 4);
    e.poll(5);
    assert_eq!(notice(&e), Some("Loaded Arena."));
    // Edits: the question comes first, and No leaves the session alone.
    make_dirty(&mut e);
    e.choose(6, Choice::Load(file.clone())).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Question));
    let question = screens::screen(Kind::Question).sheet();
    assert_eq!(question.body, flows::LOAD_QUESTION);
    assert!(e.file_job.is_none() && e.pending.is_none());
    answer(&mut e, 7, Ask::Load, false);
    assert_eq!(notice(&e), Some(flows::NOT_LOADED));
    assert!(e.dirty() && e.load_asked.is_none());
    // Yes loads it, and the session is clean again.
    e.choose(8, Choice::Load(file)).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Question));
    answer(&mut e, 9, Ask::Load, true);
    settle(&mut e, 10);
    e.poll(11);
    assert_eq!(notice(&e), Some("Loaded Arena."));
    assert!(!e.dirty());
}

#[test]
fn a_load_the_store_refuses_ends_in_its_sentence() {
    let dir = Dir::new();
    let mut e = editor_in(&dir.0);
    e.choose(1, Choice::Load("nothing".into())).unwrap();
    settle(&mut e, 2);
    assert_eq!(notice(&e), Some("nothing is not a map variant."));
    assert!(e.pending.is_none());
}
