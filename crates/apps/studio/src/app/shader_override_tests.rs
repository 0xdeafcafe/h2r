//! The override queue over invented WGSL (no game, no map).
use super::*;
use serde_json::json;

const GOOD: &str = "@vertex fn vs_main() -> @builtin(position) vec4f { return vec4f(0.0); }\n@fragment fn fs_main() -> @location(0) vec4f { return vec4f(1.0); }\n";
const ID: u64 = 0x00ab_cdef_0123_4567;

fn key() -> Key {
    Key { tag: TagId(0xe123_0045), pass: 0, implementation: 0 }
}

fn dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("h2e-shader-override-{tag}-{}", std::process::id()))
}

#[test]
fn a_program_is_named_in_sixteen_hex_digits() {
    assert_eq!(file_name(ID), "00abcdef01234567_studio.wgsl");
    assert_eq!(parse_program("0x00abcdef01234567"), Some(ID));
    assert_eq!(parse_program("abcdef"), None);
    assert_eq!(parse_program("zzzzzzzzzzzzzzzz"), None);
}

#[test]
fn a_push_needs_a_draft_that_checks() {
    let mut o = Overrides::default();
    let e = o.queue_push(key(), ID, &dir("a")).unwrap_err();
    assert!(e.contains("nothing to push"), "{e}");
    o.set_draft(key(), "fn vs_main() { nope }".into());
    let e = o.queue_push(key(), ID, &dir("a")).unwrap_err();
    assert!(e.starts_with("line 1"), "{e}");
    assert!(!o.waiting());
    assert!(o.pushed(ID).is_none());
}

#[test]
fn a_good_push_is_written_then_loaded_when_the_game_lists_it() {
    let d = dir("write");
    let _ = std::fs::remove_dir_all(&d);
    let mut o = Overrides::default();
    o.set_draft(key(), GOOD.into());
    let file = o.queue_push(key(), ID, &d).unwrap();
    assert_eq!(file, d.join("00abcdef01234567_studio.wgsl"));
    assert_eq!(o.pushed(ID).unwrap().state, State::Queued);
    let out = o.take_outgoing();
    assert_eq!(out.len(), 1);
    perform(&out[0]).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), GOOD);
    o.sent(&out[0]);
    assert_eq!(o.pushed(ID).unwrap().state, State::Sent);
    o.answered(ID, Ok(json!({"loaded": ["00abcdef01234567"], "problems": []})));
    assert_eq!(o.pushed(ID).unwrap().state, State::Loaded);
    assert_eq!(o.pushed_for(&key()).len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_file_the_game_names_a_problem_with_is_refused_with_its_words() {
    let mut o = Overrides::default();
    o.set_draft(key(), GOOD.into());
    o.queue_push(key(), ID, &dir("problem")).unwrap();
    o.answered(
        ID,
        Ok(json!({"loaded": [], "problems": [{"file": "00abcdef01234567_studio.wgsl", "error": "no entry point"}]})),
    );
    assert_eq!(o.pushed(ID).unwrap().state.why(), Some("no entry point"));
    // Not listed and no problem named: also refused, by name.
    o.answered(ID, Ok(json!({"loaded": [], "problems": []})));
    assert!(o.pushed(ID).unwrap().state.why().unwrap().contains("did not load"));
    o.answered(ID, Err("render.reload: gone".into()));
    assert_eq!(o.pushed(ID).unwrap().state.why(), Some("render.reload: gone"));
}

#[test]
fn a_revert_removes_the_file_and_forgets_the_push() {
    let d = dir("revert");
    let _ = std::fs::remove_dir_all(&d);
    let mut o = Overrides::default();
    assert!(o.queue_revert(ID).is_err());
    o.set_draft(key(), GOOD.into());
    let file = o.queue_push(key(), ID, &d).unwrap();
    perform(&o.take_outgoing()[0]).unwrap();
    o.answered(ID, Ok(json!({"loaded": ["00abcdef01234567"], "problems": []})));
    o.queue_revert(ID).unwrap();
    assert_eq!(o.pushed(ID).unwrap().state, State::Reverting);
    let out = o.take_outgoing();
    assert_eq!(out[0].job, Job::Remove);
    perform(&out[0]).unwrap();
    assert!(!file.exists());
    // A second removal is not an error.
    perform(&out[0]).unwrap();
    o.answered(ID, Ok(json!({"loaded": [], "problems": []})));
    assert!(o.pushed(ID).is_none());
    // The draft is still there to push again.
    assert_eq!(o.draft(&key()), Some(GOOD));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_revert_the_game_still_lists_is_refused() {
    let mut o = Overrides::default();
    o.set_draft(key(), GOOD.into());
    o.queue_push(key(), ID, &dir("stuck")).unwrap();
    o.queue_revert(ID).unwrap();
    o.answered(ID, Ok(json!({"loaded": ["00abcdef01234567"], "problems": []})));
    assert!(o.pushed(ID).unwrap().state.why().unwrap().contains("still lists"));
}

#[test]
fn a_program_carries_one_edit_at_a_time() {
    let mut o = Overrides::default();
    o.set_draft(key(), GOOD.into());
    let other = Key { pass: 1, ..key() };
    o.set_draft(other, GOOD.into());
    o.queue_push(key(), ID, &dir("two")).unwrap();
    let e = o.queue_push(other, ID, &dir("two")).unwrap_err();
    assert!(e.contains("already carries"), "{e}");
}

#[test]
fn a_push_the_link_cannot_send_is_refused_and_a_revert_leaves_it_loaded() {
    let mut o = Overrides::default();
    o.set_draft(key(), GOOD.into());
    o.queue_push(key(), ID, &dir("unsent")).unwrap();
    let out = o.take_outgoing();
    o.refused(&out[0], "pair with a running game first".into());
    assert_eq!(o.pushed(ID).unwrap().state.why(), Some("pair with a running game first"));
    o.queue_revert(ID).unwrap();
    let out = o.take_outgoing();
    o.refused(&out[0], "not paired".into());
    assert_eq!(o.pushed(ID).unwrap().state, State::Loaded);
}

#[test]
fn a_discarded_draft_leaves_the_push_alone() {
    let mut o = Overrides::default();
    o.set_draft(key(), GOOD.into());
    o.queue_push(key(), ID, &dir("discard")).unwrap();
    assert!(o.discard(&key()));
    assert!(!o.discard(&key()));
    assert!(o.pushed(ID).is_some());
}
