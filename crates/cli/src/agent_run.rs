//! `h2 agent run <script>`: a scripted scenario on one connection, with
//! assertions, answered as one JSON verdict (docs/runtime/agent-api.md,
//! phase 5).
//!
//! A script is the `h2 agent -` script (one call per line, `sleep <ms>`,
//! `#` comments) plus:
//!
//! - `expect <pointer> <op> [JSON]`: checks the last call's answer at a
//!   JSON pointer (`/at/phase`, `/total`, `/spikes/0/causes`). Ops: `==`,
//!   `!=`, `<`, `<=`, `>`, `>=` (numbers), `contains` (a string's
//!   substring, an array's element, an object's key), `exists`, `missing`.
//! - `keep`: put the last call's answer in the verdict.
//!
//! A failed `expect` is recorded and the script goes on; a call that fails
//! ends it (what follows would act on an unknown state). The verdict:
//! `{"pass", "calls", "expects", "failed", "ms", "kept", "stopped"}`; the
//! command exits 1 unless it passed.

use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Call(Vec<String>),
    Sleep(u64),
    Expect {
        pointer: String,
        op: Op,
        want: Option<Value>,
    },
    Keep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    Exists,
    Missing,
}

impl Op {
    fn parse(s: &str) -> Option<Op> {
        Some(match s {
            "==" => Op::Eq,
            "!=" => Op::Ne,
            "<" => Op::Lt,
            "<=" => Op::Le,
            ">" => Op::Gt,
            ">=" => Op::Ge,
            "contains" => Op::Contains,
            "exists" => Op::Exists,
            "missing" => Op::Missing,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Contains => "contains",
            Op::Exists => "exists",
            Op::Missing => "missing",
        }
    }
}

/// The script's lines, numbered from 1, with `split` doing the words.
pub fn parse(
    text: &str,
    split: impl Fn(&str) -> Result<Vec<String>>,
) -> Result<Vec<(usize, Line)>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words = split(line).with_context(|| format!("line {n}"))?;
        let parsed = match words[0].as_str() {
            "sleep" => Line::Sleep(
                words
                    .get(1)
                    .and_then(|w| w.parse().ok())
                    .with_context(|| format!("line {n}: sleep needs milliseconds"))?,
            ),
            "keep" if words.len() == 1 => Line::Keep,
            "expect" => {
                let (Some(pointer), Some(op)) = (words.get(1), words.get(2)) else {
                    bail!("line {n}: expect <pointer> <op> [JSON]");
                };
                if !(pointer.is_empty() || pointer.starts_with('/')) {
                    bail!("line {n}: a JSON pointer starts with / ({pointer})");
                }
                let op = Op::parse(op).with_context(|| {
                    format!("line {n}: {op}: one of == != < <= > >= contains exists missing")
                })?;
                let want = match words.get(3) {
                    None => None,
                    Some(w) => Some(
                        serde_json::from_str(w)
                            .with_context(|| format!("line {n}: not JSON: {w}"))?,
                    ),
                };
                let needs = !matches!(op, Op::Exists | Op::Missing);
                if needs != want.is_some() || words.len() > 4 {
                    bail!(
                        "line {n}: {} {}",
                        op.name(),
                        if needs {
                            "needs one JSON value"
                        } else {
                            "takes no value"
                        }
                    );
                }
                Line::Expect {
                    pointer: pointer.clone(),
                    op,
                    want,
                }
            }
            _ => Line::Call(words),
        };
        out.push((n, parsed));
    }
    Ok(out)
}

/// Whether `got` (absent: `None`) passes `op want`.
pub fn check(got: Option<&Value>, op: Op, want: Option<&Value>) -> bool {
    let num = |v: Option<&Value>| v.and_then(Value::as_f64);
    let cmp = |f: fn(f64, f64) -> bool| match (num(got), num(want)) {
        (Some(a), Some(b)) => f(a, b),
        _ => false,
    };
    match op {
        Op::Exists => got.is_some(),
        Op::Missing => got.is_none(),
        Op::Eq => got.is_some() && got == want,
        Op::Ne => got != want,
        Op::Lt => cmp(|a, b| a < b),
        Op::Le => cmp(|a, b| a <= b),
        Op::Gt => cmp(|a, b| a > b),
        Op::Ge => cmp(|a, b| a >= b),
        Op::Contains => match (got, want) {
            (Some(Value::String(s)), Some(Value::String(w))) => s.contains(w.as_str()),
            (Some(Value::Array(a)), Some(w)) => a.contains(w),
            (Some(Value::Object(o)), Some(Value::String(k))) => o.contains_key(k),
            _ => false,
        },
    }
}

/// Runs the lines: `call` makes one call from its words, `sleep` waits.
pub fn execute(
    lines: &[(usize, Line)],
    mut call: impl FnMut(&[String]) -> Result<Value, String>,
    mut sleep: impl FnMut(u64),
) -> Value {
    let started = Instant::now();
    let (mut calls, mut expects, mut kept) = (Vec::new(), Vec::new(), Vec::new());
    let mut last: Option<Value> = None;
    let mut stopped = None;
    for (n, line) in lines {
        match line {
            Line::Sleep(ms) => sleep(*ms),
            Line::Keep => kept.push(json!({ "line": n, "result": last })),
            Line::Call(words) => {
                let t = Instant::now();
                let r = call(words);
                let ms = (t.elapsed().as_secs_f64() * 1e5).round() / 100.0;
                match r {
                    Ok(v) => {
                        calls.push(
                            json!({ "line": n, "call": words.join(" "), "ms": ms, "ok": true }),
                        );
                        last = Some(v);
                    }
                    Err(e) => {
                        calls.push(json!({ "line": n, "call": words.join(" "), "ms": ms, "ok": false, "error": e }));
                        stopped = Some(*n);
                        break;
                    }
                }
            }
            Line::Expect { pointer, op, want } => {
                let got = last.as_ref().and_then(|v| v.pointer(pointer));
                let pass = check(got, *op, want.as_ref());
                let mut e = json!({ "line": n, "expect": format!("{pointer} {}", op.name()), "pass": pass });
                if let Some(w) = want {
                    e["want"] = w.clone();
                }
                if !pass {
                    e["got"] = got.cloned().unwrap_or(Value::Null);
                    if last.is_none() {
                        e["why"] = json!("no call answered before it");
                    }
                }
                expects.push(e);
            }
        }
    }
    let failed = expects.iter().filter(|e| e["pass"] == false).count();
    let mut v = json!({
        "pass": failed == 0 && stopped.is_none(),
        "failed": failed,
        "ms": (started.elapsed().as_secs_f64() * 1e3).round(),
        "calls": calls,
        "expects": expects,
    });
    if let Some(n) = stopped {
        v["stopped"] = json!(n);
    }
    if !kept.is_empty() {
        v["kept"] = json!(kept);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> Result<Vec<String>> {
        crate::agent_cmd::split_words(s)
    }

    #[test]
    fn a_script_with_expectations_gives_one_verdict() {
        let text = r#"
            # a grenade in an empty room
            status
            expect /at/phase == "gameplay"
            expect /at/map_ticks >= 30
            count
            expect /total < 5
            expect /by/actor missing
            keep
            sleep 10
            capture 30
            expect /spikes/0/causes contains "shader_compile"
            expect /csv contains "captures"
        "#;
        let lines = parse(text, split).unwrap();
        assert_eq!(lines.len(), 11);
        let mut slept = 0;
        let v = execute(
            &lines,
            |w| {
                Ok(match w[0].as_str() {
                    "status" => json!({"at": {"phase": "gameplay", "map_ticks": 12}}),
                    "count" => json!({"total": 3, "by": {"weapon": 3}}),
                    "capture" => {
                        json!({"spikes": [{"causes": ["shader_compile"]}], "csv": "/c/captures/a.csv"})
                    }
                    other => panic!("{other}"),
                })
            },
            |ms| slept += ms,
        );
        assert_eq!(slept, 10);
        assert_eq!(v["pass"], false);
        assert_eq!(v["failed"], 1);
        let bad: Vec<&Value> = v["expects"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["pass"] == false)
            .collect();
        assert_eq!(bad[0]["line"], 5);
        assert_eq!(bad[0]["got"], 12);
        assert_eq!(v["kept"][0]["result"]["total"], 3);
        assert_eq!(v["calls"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn a_failed_call_stops_the_script() {
        let lines = parse("claim\npause\nexpect /x exists\nstep 3", split).unwrap();
        let v = execute(
            &lines,
            |w| match w[0].as_str() {
                "claim" => Ok(json!({"controller": true})),
                _ => Err("NOT_CONTROLLER".into()),
            },
            |_| {},
        );
        assert_eq!(
            (v["pass"].clone(), v["stopped"].clone()),
            (json!(false), json!(2))
        );
        assert_eq!(v["calls"].as_array().unwrap().len(), 2);
        assert!(v["expects"].as_array().unwrap().is_empty());
    }

    #[test]
    fn checks() {
        let v = json!({"a": 2, "s": "abc", "l": [1, 2], "o": {"k": 1}});
        let at = |p: &str| v.pointer(p);
        assert!(check(at("/a"), Op::Eq, Some(&json!(2))));
        assert!(check(at("/a"), Op::Le, Some(&json!(2.0))));
        assert!(!check(at("/s"), Op::Gt, Some(&json!(1))), "not a number");
        assert!(check(at("/s"), Op::Contains, Some(&json!("bc"))));
        assert!(check(at("/l"), Op::Contains, Some(&json!(2))));
        assert!(check(at("/o"), Op::Contains, Some(&json!("k"))));
        assert!(check(at("/nope"), Op::Missing, None));
        assert!(check(at("/nope"), Op::Ne, Some(&json!(1))));
        assert!(!check(at("/nope"), Op::Eq, Some(&Value::Null)));
    }

    #[test]
    fn bad_lines_are_refused() {
        for bad in [
            "expect /a",
            "expect a == 1",
            "expect /a ~ 1",
            "expect /a == ",
            "expect /a exists 1",
            "expect /a == {oops",
            "sleep soon",
        ] {
            assert!(parse(bad, split).is_err(), "{bad}");
        }
    }
}
