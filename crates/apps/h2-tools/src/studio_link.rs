//! The language servers' line to Studio (`h2-editor`) over its socket
//! (docs/editor/agent-api.md): the commands VS Code and Zed run with
//! `workspace/executeCommand` (docs/editor/rules-editors.md, "Commands
//! that talk to Studio"). Each command is a few socket calls; [`Studio`]
//! is the seam the tests fill with a fake.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The commands, by the name the server declares
/// (`executeCommandProvider.commands`).
pub mod command {
    /// Reload a `.rules` file into the game paired with Studio.
    pub const RELOAD: &str = "h2.studio.reload";
    /// Run a `.rules` text over a scenario in Studio.
    pub const RUN_SCENARIO: &str = "h2.studio.runScenario";
    /// The map's definitions that name a script, global, object or volume.
    pub const USES: &str = "h2.studio.uses";
    /// Show a script or global in Studio's Scripts tab.
    pub const SHOW: &str = "h2.studio.show";
}

/// Something that answers Studio's socket methods.
pub(crate) trait Studio {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
    /// Waits a moment between two reads of a state the game fills in.
    fn pause(&mut self) {
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Studio at its socket: `$H2_STUDIO`, else `studio.sock` in the cache
/// directory (`studio_proto::default_socket`). One connection a call.
pub(crate) struct Socket;

impl Studio for Socket {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let path = studio_proto::default_socket();
        let mut c = studio_proto::cli::connect(&path).map_err(|e| format!("{e:#}"))?;
        c.set_timeout(Some(Duration::from_secs(20)))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        c.call(method, params).map_err(|e| match e {
            agent_proto::CallError::Rpc(e) => e.message,
            other => format!("Studio: {other}"),
        })
    }
}

/// How long [`reload`] waits for the game's answer.
const RELOAD_WAIT: Duration = Duration::from_secs(3);

/// A `file://` URI's path, its `%XX` escapes decoded.
pub(crate) fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://host/path` names a host; only the local one is a path here.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = rest.get(i + 1..i + 3)
            && let Ok(v) = u8::from_str_radix(h, 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(out).ok()?;
    // Windows: `file:///C:/x` is `C:/x`.
    let text = match text.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => text[1..].to_owned(),
        _ => text,
    };
    Some(PathBuf::from(text))
}

/// The one argument of a command: an object.
pub(crate) fn argument(params: &Value) -> Result<&Value, String> {
    params
        .get("arguments")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .filter(|a| a.is_object())
        .ok_or_else(|| "the command takes one object argument".to_owned())
}

/// The file an argument's `uri` names.
pub(crate) fn file_of(arg: &Value) -> Result<PathBuf, String> {
    let uri = arg.get("uri").and_then(Value::as_str).ok_or("no uri")?;
    path_of(uri).ok_or_else(|| format!("{uri} is not a file on this machine"))
}

/// The sentence for a reload state (`rules.get`'s `reload`).
pub(crate) fn reload_message(name: &str, r: &Value) -> Option<String> {
    match r.get("state").and_then(Value::as_str)? {
        "done" => {
            let tick = r.get("tick").and_then(Value::as_u64).unwrap_or(0);
            Some(match r.get("mode").and_then(Value::as_str) {
                Some("restarted") => {
                    let reasons: Vec<&str> = r
                        .get("reasons")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    if reasons.is_empty() {
                        format!("Reloaded {name}: the game restarted the round")
                    } else {
                        format!("Reloaded {name}: the game restarted the round ({})", reasons.join("; "))
                    }
                }
                _ => format!("Reloaded {name} in place at tick {tick}"),
            })
        }
        "refused" => Some(format!(
            "{name} was not reloaded: {}",
            r.get("why").and_then(Value::as_str).unwrap_or("refused")
        )),
        _ => None,
    }
}

/// `h2.studio.reload`: Studio takes the file (opening it, or taking its
/// saved text), checks it and sends it to the paired game, then the game's
/// answer is read back for up to [`RELOAD_WAIT`].
pub(crate) fn reload(studio: &mut dyn Studio, arg: &Value) -> Result<Value, String> {
    let path = file_of(arg)?;
    let name = path
        .file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let mut params = json!({"path": path.display().to_string()});
    if let Some(mode) = arg.get("mode").and_then(Value::as_str) {
        params["mode"] = json!(mode);
    }
    let queued = studio.call("rules.reload", params)?;
    let hash = queued.get("queued").and_then(Value::as_str).unwrap_or("").to_owned();
    let started = Instant::now();
    loop {
        let state = studio.call("rules.get", json!({}))?;
        let r = state.get("reload").cloned().unwrap_or(Value::Null);
        // Only the answer to this reload.
        let ours = r.get("hash").and_then(Value::as_str).is_none_or(|h| h == hash);
        if ours && let Some(message) = reload_message(&name, &r) {
            return Ok(json!({"message": message, "reload": r}));
        }
        if started.elapsed() >= RELOAD_WAIT {
            let short = hash.get(..12).unwrap_or(&hash);
            return Ok(json!({
                "message": format!("Sent {name} ({short}) to the game; it has not answered yet: Studio's script window shows its answer"),
                "reload": r,
            }));
        }
        studio.pause();
    }
}

/// `h2.studio.runScenario`: Studio runs the document's text over the
/// scenario file and answers with the printout.
pub(crate) fn run_scenario(studio: &mut dyn Studio, arg: &Value) -> Result<Value, String> {
    let path = file_of(arg)?;
    let scenario = arg.get("scenario").and_then(Value::as_str).ok_or("no scenario file")?;
    let mut params = json!({"scenario": scenario, "label": path.display().to_string()});
    match arg.get("source").and_then(Value::as_str) {
        Some(src) => params["source"] = json!(src),
        None => params["path"] = json!(path.display().to_string()),
    }
    if arg.get("trace").and_then(Value::as_bool) == Some(true) {
        params["trace"] = json!(true);
    }
    let run = studio.call("rules.run", params)?;
    let ok = run.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let result = run.get("result").and_then(Value::as_str).unwrap_or("");
    let file = std::path::Path::new(scenario)
        .file_name()
        .map_or_else(|| scenario.to_owned(), |n| n.to_string_lossy().into_owned());
    let message = if ok {
        format!("{file}: {result}")
    } else {
        format!("{file}: failed: {result}")
    };
    Ok(json!({
        "message": message,
        "ok": ok,
        "text": run.get("text").and_then(Value::as_str).unwrap_or(""),
    }))
}

/// `h2.studio.uses`: the map's definitions that name `kind` `name`
/// (Studio's `script.uses`), each with where it is in `defs` when the
/// working copy has it (`(name, range)`).
pub(crate) fn uses(
    studio: &mut dyn Studio,
    kind: &str,
    name: &str,
    defs: &[(String, Value)],
) -> Result<Value, String> {
    let r = studio.call("script.uses", json!({"kind": kind, "name": name}))?;
    let list: Vec<Value> = r
        .get("uses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|u| {
            let user = u.get("name").and_then(Value::as_str).unwrap_or("");
            let range = defs.iter().find(|(n, _)| n == user).map_or(Value::Null, |(_, r)| r.clone());
            json!({
                "name": user,
                "item": u.get("item").cloned().unwrap_or(Value::Null),
                "count": u.get("count").cloned().unwrap_or(json!(0)),
                "range": range,
            })
        })
        .collect();
    let message = match list.len() {
        0 => format!("No script or global in the map names {kind} {name}"),
        1 => format!("1 definition in the map names {kind} {name}"),
        n => format!("{n} definitions in the map name {kind} {name}"),
    };
    Ok(json!({"message": message, "kind": kind, "name": name, "uses": list}))
}

/// `h2.studio.show`: Studio's Scripts tab shows the script or global.
pub(crate) fn show(studio: &mut dyn Studio, name: &str) -> Result<Value, String> {
    let r = studio.call("script.source", json!({"name": name}))?;
    let item = r.get("item").and_then(Value::as_str).unwrap_or("script");
    Ok(json!({"message": format!("Studio shows {item} {name}")}))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Answers each method from a list, in order, and records the calls
    /// (shared, so a test still reads them once a server owns the fake).
    #[derive(Default)]
    pub(crate) struct Fake {
        pub answers: Vec<(&'static str, Result<Value, String>)>,
        pub calls: std::rc::Rc<std::cell::RefCell<Vec<(String, Value)>>>,
    }

    impl Studio for Fake {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
            self.calls.borrow_mut().push((method.to_owned(), params));
            let i = self
                .answers
                .iter()
                .position(|(m, _)| *m == method)
                .unwrap_or_else(|| panic!("no answer for {method}"));
            // The last answer for a method stays for later calls.
            if self.answers.iter().filter(|(m, _)| *m == method).count() > 1 {
                self.answers.remove(i).1
            } else {
                self.answers[i].1.clone()
            }
        }
        fn pause(&mut self) {}
    }

    #[test]
    fn file_uris_decode_to_paths() {
        assert_eq!(path_of("file:///tmp/a%20b/x.rules"), Some(PathBuf::from("/tmp/a b/x.rules")));
        assert_eq!(path_of("file://localhost/tmp/x.hsc"), Some(PathBuf::from("/tmp/x.hsc")));
        assert_eq!(path_of("file:///C:/maps/x.hsc"), Some(PathBuf::from("C:/maps/x.hsc")));
        assert_eq!(path_of("untitled:Untitled-1"), None);
        assert_eq!(path_of("file:///tmp/%zz"), Some(PathBuf::from("/tmp/%zz")), "a bad escape stays");
    }

    #[test]
    fn a_reload_waits_for_the_games_answer_to_this_reload() {
        let mut s = Fake::default();
        s.answers = vec![
            ("rules.reload", Ok(json!({"queued": "abc"}))),
            ("rules.get", Ok(json!({"reload": {"state": "done", "hash": "old", "mode": "in_place", "tick": 1}}))),
            ("rules.get", Ok(json!({"reload": {"state": "sending", "hash": "abc"}}))),
            ("rules.get", Ok(json!({"reload": {"state": "done", "hash": "abc", "mode": "in_place", "tick": 40}}))),
        ];
        let arg = json!({"uri": "file:///w/mine.rules", "mode": "in_place"});
        let v = reload(&mut s, &arg).unwrap();
        assert_eq!(v["message"], "Reloaded mine.rules in place at tick 40");
        assert_eq!(s.calls.borrow()[0], ("rules.reload".to_owned(), json!({"path": "/w/mine.rules", "mode": "in_place"})));
        assert_eq!(s.calls.borrow().len(), 4);

        let mut s = Fake::default();
        s.answers = vec![
            ("rules.reload", Ok(json!({"queued": "abc"}))),
            ("rules.get", Ok(json!({"reload": {"state": "refused", "why": "pair with a running game first"}}))),
        ];
        let v = reload(&mut s, &json!({"uri": "file:///w/mine.rules"})).unwrap();
        assert_eq!(v["message"], "mine.rules was not reloaded: pair with a running game first");

        // Studio's refusal (unsaved edits there, a script that does not
        // compile) is the command's error.
        let mut s = Fake::default();
        s.answers = vec![("rules.reload", Err("the open script has unsaved edits".into()))];
        assert!(reload(&mut s, &json!({"uri": "file:///w/mine.rules"})).unwrap_err().contains("unsaved"));

        let restarted = json!({"state": "done", "mode": "restarted", "reasons": ["a new player variable"]});
        assert_eq!(
            reload_message("x.rules", &restarted).unwrap(),
            "Reloaded x.rules: the game restarted the round (a new player variable)"
        );
        assert_eq!(reload_message("x", &json!({"state": "sending"})), None);
    }

    #[test]
    fn a_scenario_runs_the_documents_text_in_studio() {
        let mut s = Fake::default();
        s.answers = vec![(
            "rules.run",
            Ok(json!({"ok": true, "result": "the script ended the round at tick 3", "text": "tick 0: 0 calls\n"})),
        )];
        let arg = json!({"uri": "file:///w/mine.rules", "source": "rules \"T\" 1\n", "scenario": "/w/case.json"});
        let v = run_scenario(&mut s, &arg).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["message"], "case.json: the script ended the round at tick 3");
        assert_eq!(v["text"], "tick 0: 0 calls\n");
        assert_eq!(
            s.calls.borrow()[0].1,
            json!({"scenario": "/w/case.json", "label": "/w/mine.rules", "source": "rules \"T\" 1\n"})
        );
        assert!(run_scenario(&mut s, &json!({"uri": "file:///w/mine.rules"})).unwrap_err().contains("scenario"));
    }

    #[test]
    fn uses_point_at_the_definitions_in_the_working_copy() {
        let mut s = Fake::default();
        s.answers = vec![(
            "script.uses",
            Ok(json!({"uses": [{"name": "main", "item": "script", "count": 2}, {"name": "gone", "item": "script", "count": 1}]})),
        )];
        let range = json!({"start": {"line": 6, "character": 16}, "end": {"line": 6, "character": 20}});
        let v = uses(&mut s, "script", "helper", &[("main".to_owned(), range.clone())]).unwrap();
        assert_eq!(v["message"], "2 definitions in the map name script helper");
        assert_eq!(v["uses"][0]["range"], range);
        assert_eq!(v["uses"][1]["range"], Value::Null);
        assert_eq!(s.calls.borrow()[0].1, json!({"kind": "script", "name": "helper"}));

        s.answers = vec![("script.source", Ok(json!({"item": "global"})))];
        assert_eq!(show(&mut s, "count").unwrap()["message"], "Studio shows global count");
    }
}
