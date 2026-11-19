use super::*;
use crate::bots_verdict::Plan;
use crate::coop_verdict::format_line;

fn log_of(lines: &[&str]) -> String {
    let plan = Plan {
        count: 5,
        tag: "Tester".into(),
        map: None,
    };
    let mut s = "# host Tester\n".to_string();
    s.push_str(&format_line(0.0, "Tester", &plan.line()));
    s.push('\n');
    for (i, l) in lines.iter().enumerate() {
        s.push_str(&format_line(i as f64 + 1.0, "Tester", l));
        s.push('\n');
    }
    s
}

fn json(log: &str, finished: bool) -> Value {
    serde_json::from_str(&render(log, finished).expect("a plan")).unwrap()
}

#[test]
fn no_plan_line_no_file() {
    assert_eq!(render("# host Tester\n", false), None);
}

#[test]
fn a_fresh_run_has_twelve_pending_checks() {
    let v = json(&log_of(&[]), false);
    let checks = v["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 12);
    assert!(checks.iter().all(|c| c["state"] == "pending"));
    assert_eq!(v["finished"], false);
    assert_eq!(v["phase"], "-");
    assert_eq!(v["next"]["step"], 1);
}

#[test]
fn a_finished_run_fails_what_is_not_passed() {
    let v = json(&log_of(&[]), true);
    assert!(
        v["checks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["state"] == "fail")
    );
}

#[test]
fn it_carries_the_last_status_ui_and_bots_lines() {
    let v = json(
        &log_of(&[
            "bots-test: status phase menu mode live map -",
            "ui: screen none (slot 4: one)",
            "bots: first",
            "ui: screen none (slot 4: two)",
            "clankers: second",
            "bots-test: status phase gameplay mode live map lockout",
        ]),
        false,
    );
    assert_eq!(v["phase"], "gameplay");
    assert_eq!(v["mode"], "live");
    assert_eq!(v["map"], "lockout");
    assert_eq!(v["last_ui"], "ui: screen none (slot 4: two)");
    assert_eq!(v["last_bots"], "clankers: second");
}

#[test]
fn write_replaces_the_file_whole() {
    let dir = std::env::temp_dir().join(format!("h2-progress-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    write(&dir, "{\"a\":1}").unwrap();
    write(&dir, "{\"a\":2}").unwrap();
    assert_eq!(fs::read_to_string(dir.join(FILE)).unwrap(), "{\"a\":2}");
    assert!(!dir.join("progress.json.tmp").exists());
    fs::remove_dir_all(&dir).unwrap();
}
