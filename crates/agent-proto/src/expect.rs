//! Expectations: a JSON pointer into an answer, an op and a wanted value,
//! judged into a verdict entry. One format for `h2 agent run`'s `expect`
//! lines and for `sim.expect`, which judges the same checks in the game
//! over `sim.view`'s view (docs/runtime/agent-api.md).
//!
//! Ops: `==`, `!=` (JSON equality), `<`, `<=`, `>`, `>=` (numbers),
//! `contains` (a string's substring, an array's element, an object's key),
//! `exists`, `missing`, and `near` (`{"to": number or array, "within": d}`:
//! every element within `d`, for positions and vitality read from `f32`s).

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

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
    Near,
}

impl Op {
    pub const NAMES: &str = "== != < <= > >= contains exists missing near";

    pub fn parse(s: &str) -> Option<Op> {
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
            "near" => Op::Near,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
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
            Op::Near => "near",
        }
    }

    /// Whether it compares with a value (all but `exists` and `missing`).
    pub fn needs_value(self) -> bool {
        !matches!(self, Op::Exists | Op::Missing)
    }
}

impl Serialize for Op {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for Op {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Op, D::Error> {
        let s = String::deserialize(d)?;
        Op::parse(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("{s}: one of {}", Op::NAMES)))
    }
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
        Op::Near => {
            let (Some(got), Some(w)) = (got, want) else {
                return false;
            };
            let (Some(to), Some(d)) = (w.get("to"), w.get("within").and_then(Value::as_f64))
            else {
                return false;
            };
            near(got, to, d)
        }
    }
}

fn near(got: &Value, to: &Value, d: f64) -> bool {
    match (got, to) {
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| near(x, y, d))
        }
        _ => match (got.as_f64(), to.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() <= d,
            _ => false,
        },
    }
}

/// One expectation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Expect {
    /// A JSON pointer (`/at/phase`, `/player/1/team`); empty is the whole
    /// answer.
    pub pointer: String,
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub want: Option<Value>,
}

impl Expect {
    /// Checks the pointer and that the op has a value exactly when it
    /// needs one (and `near`'s is `{"to", "within"}`).
    pub fn new(pointer: &str, op: Op, want: Option<Value>) -> Result<Expect, String> {
        if !(pointer.is_empty() || pointer.starts_with('/')) {
            return Err(format!("a JSON pointer starts with / ({pointer})"));
        }
        let needs = op.needs_value();
        if needs != want.is_some() {
            return Err(format!(
                "{} {}",
                op.name(),
                if needs {
                    "needs one JSON value"
                } else {
                    "takes no value"
                }
            ));
        }
        if op == Op::Near
            && !want
                .as_ref()
                .is_some_and(|w| w.get("to").is_some() && w["within"].is_number())
        {
            return Err("near needs {\"to\": number or array, \"within\": number}".into());
        }
        Ok(Expect {
            pointer: pointer.into(),
            op,
            want,
        })
    }

    /// From `<pointer> <op> [JSON]`: the JSON is the rest of the line,
    /// spaces and all.
    pub fn parse(line: &str) -> Result<Expect, String> {
        let line = line.trim();
        let (pointer, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim_start();
        let (op, want) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let op = Op::parse(op).ok_or_else(|| format!("{op:?}: one of {}", Op::NAMES))?;
        let want = want.trim();
        let want = match want {
            "" => None,
            w => Some(serde_json::from_str(w).map_err(|e| format!("not JSON: {w} ({e})"))?),
        };
        Expect::new(pointer, op, want)
    }

    /// `<pointer> <op>`, as verdicts name an expectation.
    pub fn name(&self) -> String {
        format!("{} {}", self.pointer, self.op.name())
    }

    /// The verdict entry for this expectation against `answer` (`None`:
    /// nothing answered before it): `{"expect", "pass", "want"}`, and
    /// `got` when it failed.
    pub fn judge(&self, answer: Option<&Value>) -> Value {
        let got = answer.and_then(|v| v.pointer(&self.pointer));
        let pass = check(got, self.op, self.want.as_ref());
        let mut e = json!({ "expect": self.name(), "pass": pass });
        if let Some(w) = &self.want {
            e["want"] = w.clone();
        }
        if !pass {
            e["got"] = got.cloned().unwrap_or(Value::Null);
            if answer.is_none() {
                e["why"] = json!("no call answered before it");
            }
        }
        e
    }
}

/// An expectation is `{"pointer", "op", "want"}` or the line
/// `"<pointer> <op> [JSON]"`.
impl<'de> Deserialize<'de> for Expect {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Expect, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Parts {
            pointer: String,
            op: Op,
            #[serde(default)]
            want: Option<Value>,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            Line(String),
            Parts(Parts),
        }
        match Form::deserialize(d)? {
            Form::Line(l) => Expect::parse(&l),
            Form::Parts(p) => Expect::new(&p.pointer, p.op, p.want),
        }
        .map_err(serde::de::Error::custom)
    }
}

/// Every expectation judged against one answer: `{"pass", "failed",
/// "expects"}`, the fields `h2 agent run`'s verdict has.
pub fn verdict(answer: &Value, expects: &[Expect]) -> Value {
    let entries: Vec<Value> = expects.iter().map(|e| e.judge(Some(answer))).collect();
    let failed = entries.iter().filter(|e| e["pass"] == false).count();
    json!({ "pass": failed == 0, "failed": failed, "expects": entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks() {
        let v = json!({"a": 2, "s": "abc", "l": [1, 2], "o": {"k": 1}, "p": [1.0, 2.0, 2.999]});
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
        let near = |to: Value, d: f64| json!({"to": to, "within": d});
        assert!(check(at("/p"), Op::Near, Some(&near(json!([1, 2, 3]), 0.01))));
        assert!(!check(at("/p"), Op::Near, Some(&near(json!([1, 2, 4]), 0.5))));
        assert!(!check(at("/p"), Op::Near, Some(&near(json!([1, 2]), 9.0))));
        assert!(check(at("/a"), Op::Near, Some(&near(json!(2.4), 0.5))));
    }

    #[test]
    fn lines_and_objects_parse_to_the_same_expectation() {
        let a = Expect::parse(r#"/player/0/pos near {"to": [1, 2, 3], "within": 0.5}"#).unwrap();
        let b: Expect = serde_json::from_value(json!({
            "pointer": "/player/0/pos", "op": "near", "want": {"to": [1, 2, 3], "within": 0.5}
        }))
        .unwrap();
        assert_eq!(a, b);
        let c: Expect = serde_json::from_value(json!("/at/phase == \"gameplay\"")).unwrap();
        assert_eq!(
            (c.op, c.want),
            (Op::Eq, Some(json!("gameplay")))
        );
        let d = Expect::parse("/sim/1/player exists").unwrap();
        assert_eq!((d.op, d.want), (Op::Exists, None));
    }

    #[test]
    fn bad_expectations_are_refused() {
        for bad in [
            "/a",
            "a == 1",
            "/a ~ 1",
            "/a ==",
            "/a exists 1",
            "/a == {oops",
            "/a near [1, 2]",
        ] {
            assert!(Expect::parse(bad).is_err(), "{bad}");
        }
        assert!(
            serde_json::from_value::<Expect>(json!({"pointer": "/a", "op": "==", "want": 1, "x": 0}))
                .is_err()
        );
    }

    #[test]
    fn a_verdict_says_what_it_got_when_it_failed() {
        let answer = json!({"at": {"phase": "menu"}, "count": {"player": 2}});
        let expects = [
            Expect::parse("/count/player >= 2").unwrap(),
            Expect::parse(r#"/at/phase == "gameplay""#).unwrap(),
        ];
        let v = verdict(&answer, &expects);
        assert_eq!((v["pass"].clone(), v["failed"].clone()), (json!(false), json!(1)));
        assert_eq!(v["expects"][0], json!({"expect": "/count/player >=", "pass": true, "want": 2}));
        assert_eq!(v["expects"][1]["got"], "menu");
        assert_eq!(v["expects"][1]["want"], "gameplay");
        let none = expects[0].judge(None);
        assert_eq!(none["why"], "no call answered before it");
    }
}
