//! Curve functions as the agent API ([`super::Provider`], registered in
//! [`super::seams`]): `function.read`, `function.set` and
//! `function.sample`, over a `view=function` data field's blob, written
//! through the Tags tool's own command (`Action::Tag` with
//! `App::perform`), so the window's curve editor and the API are one
//! path: the same undo step and the same saved patch. A write that
//! keeps the type is a same-length byte write; a type change rebuilds
//! the blob at the new type's length with its defaults
//! (`Function::with_kind`) and goes through `TagDoc::resize_data`
//! (rich-editors.md's F2). docs/editor/rich-editors.md,
//! docs/editor/agent-api.md.

use map::TagId;
use serde_json::{Map, Value, json};
use studio_proto::{code, method as m};
use tags::effects::function::{Function, FunctionType, Set};
use tags::registry::{FieldView, Kind};

use super::tag_fields;
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::tag_api;
use crate::app::{TagAction, TagWrite};
use crate::fields::curve;

/// The provider.
#[derive(Default)]
pub struct Curves;

/// The shape of `function.read`'s result, which `function.set` also gives.
const READ_RESULT: &str = r#"{"type":"string","ranged":"boolean","sub_types":"[integer, integer]","range":"[number, number]","length":"integer","sets":"[{shape?, from, to}|{wave?, frequency, phase, minimum, maximum}|{slope, intercept}|{base, keys:[{value, position, inverse_span}]}|{points, coefficients}|{minimum, maximum, power}|{words}|{}]|null","editable":"boolean","read_only":"string|null"}"#;
const READ: MethodDef = def(
    m::FUNCTION_READ,
    "The function blob a `view=function` data field points at: the type word, whether it is ranged, the two sub-type bytes (set 0's and set 1's wave or shape), the output range, the blob's length, its two sets' typed values (per type, the words the engine's evaluator reads, named as the editor's handles; a constant's, type 6's and type 8's words are given as `words`; null for a type with no layout), whether it may be edited, and why not when it may not (a type the engine does not evaluate, or the tag's own reason)",
    r#"{"tag":"string|integer","path":"string (a tag.set path)"}"#,
    READ_RESULT,
    r#"{"tag":"weap","path":"curve"}"#,
);
const SET: MethodDef = def(
    m::FUNCTION_SET,
    "Edit a function curve: `type` changes the type (the blob is rebuilt at the new type's length with its defaults, a transition from 0 to 1, a periodic cosine, a line of slope 1 and so on, keeping the flags, the output range and the header's other bytes; one of the editable types, not 6, 8 or an unknown one), `ranged` turns the second set on or off, `range` sets the output range, and `sets` holds one or two set objects whose keys present are written (the shape or wave byte rides in the set, as the dialog's picker rows write it). One undo step. Refused as the Curves section says: a change to a type the game evaluates as 0, a type the game evaluates as 0, a read-only tag, a data reference that is not a function blob",
    r#"{"tag":"string|integer","path":"string (a tag.set path)","type?":"string (identity, constant, transition, periodic, linear, linear_key, spline, spline_2 or exponent)","ranged?":"boolean","range?":"[number, number]","sets?":"[set objects]"}"#,
    READ_RESULT,
    r#"{"tag":"weap","path":"curve","sets":[{"from":0.2,"to":0.8,"shape":2}],"ranged":true,"range":[0,1]}"#,
);
const SAMPLE: MethodDef = def(
    m::FUNCTION_SAMPLE,
    "Sample the curve: `Function::evaluate` itself (the engine's evaluator, ported) at each input, with `range_input` blending the two sets of a ranged function (default 0, set 0). Types the engine does not evaluate give 0",
    r#"{"tag":"string|integer","path":"string (a tag.set path)","inputs":"[number]","range_input?":"number"}"#,
    r#"{"outputs":"[number]"}"#,
    r#"{"tag":"weap","path":"curve","inputs":[0,0.5,1]}"#,
);

/// A finite number of a JSON value.
fn number(v: &Value) -> Result<f32, agent_proto::RpcError> {
    v.as_f64()
        .filter(|x| x.is_finite())
        .map(|x| x as f32)
        .ok_or_else(|| err(code::INVALID_PARAMS, "a number"))
}

/// An error from [`number`], with where the number was.
fn at(prefix: &str, e: agent_proto::RpcError) -> agent_proto::RpcError {
    err(e.code, format!("{prefix}: {}", e.message))
}

/// The curve a call works on.
struct Curve {
    tag: TagId,
    /// The field's name, for the undo step's label.
    name: String,
    /// The field's struct (its address and layout) and offset, for a
    /// resize.
    element: u32,
    layout: std::sync::Arc<tags::registry::Layout>,
    field: u32,
    /// The blob's length and start.
    length: u32,
    start: u32,
    function: Function,
}

/// The field `path` names, as a function curve.
fn curve_field(app: &mut App, p: &P) -> Result<Curve, agent_proto::RpcError> {
    let tag = tag_fields::resolve(app, p)?;
    let path = p.str("path")?;
    let doc = app.shown_tags();
    let (element, layout, f) = tag_api::resolve(doc, tag, path)
        .map_err(|e| err(code::INVALID_PARAMS, format!("{path}: {e}")))?;
    if f.kind != Kind::Data || f.view != Some(FieldView::Function) {
        return Err(err(
            code::REFUSED,
            format!("{path} is not a function curve field"),
        ));
    }
    let (length, start, function) = curve::function(doc, tag, element + f.offset)
        .map_err(|e| err(code::REFUSED, format!("{path}: {e}")))?;
    Ok(Curve {
        tag,
        name: f.name,
        element,
        layout,
        field: f.offset,
        length,
        start,
        function,
    })
}

/// Why the function may not be written, if it may not: the tag's own
/// reason, or a type the engine does not evaluate.
fn read_only(app: &App, tag: TagId, f: &Function) -> Option<String> {
    app.shown_tags()
        .refusal(tag)
        .map(str::to_owned)
        .or((!f.kind.is_evaluated()).then(|| curve::EVALUATED_AS_ZERO.to_owned()))
}

/// One set as `function.read` gives it: the typed values, with the set's
/// wave or shape where the type uses one.
fn set_json(f: &Function, i: usize) -> Value {
    let sub = f.sub_types[i];
    match f.set_values(i) {
        Some(Set::Identity) => json!({}),
        Some(Set::Words(words)) => json!({ "words": words }),
        Some(Set::Transition { from, to }) => json!({ "shape": sub, "from": from, "to": to }),
        Some(Set::Periodic {
            frequency,
            phase,
            minimum,
            maximum,
        }) => json!({
            "wave": sub,
            "frequency": frequency,
            "phase": phase,
            "minimum": minimum,
            "maximum": maximum,
        }),
        Some(Set::Linear { slope, intercept }) => json!({ "slope": slope, "intercept": intercept }),
        Some(Set::LinearKey { base, keys }) => json!({
            "base": base,
            "keys": keys.iter().map(|k| json!({
                "value": k.value,
                "position": k.position,
                "inverse_span": k.inverse_span,
            })).collect::<Vec<_>>(),
        }),
        Some(Set::Spline {
            points,
            coefficients,
        }) => json!({ "points": points, "coefficients": coefficients }),
        Some(Set::Exponent {
            minimum,
            maximum,
            power,
        }) => json!({
            "minimum": minimum,
            "maximum": maximum,
            "power": power,
        }),
        None => Value::Null,
    }
}

/// The read of `f` (the result of `function.read` and of `function.set`).
fn read_json(app: &App, tag: TagId, n: u32, f: &Function) -> Value {
    let sets = f
        .set_values(0)
        .map(|_| (0..2).map(|i| set_json(f, i)).collect::<Vec<_>>());
    let read_only = read_only(app, tag, f);
    json!({
        "type": f.kind.word(),
        "ranged": f.is_ranged(),
        "sub_types": f.sub_types,
        "range": f.range,
        "length": n,
        "sets": sets,
        "editable": read_only.is_none(),
        "read_only": read_only,
    })
}

fn read(app: &mut App, p: &P) -> Reply {
    let c = curve_field(app, p)?;
    Ok(read_json(app, c.tag, c.length, &c.function))
}

fn sample(app: &mut App, p: &P) -> Reply {
    let c = curve_field(app, p)?;
    let inputs = p
        .get("inputs")
        .and_then(Value::as_array)
        .ok_or_else(|| err(code::INVALID_PARAMS, "inputs: a list of numbers"))?;
    let r = p.opt_f32("range_input")?.unwrap_or(0.0);
    let mut outputs = Vec::with_capacity(inputs.len());
    for x in inputs {
        let x = number(x).map_err(|e| at("inputs", e))?;
        outputs.push(c.function.evaluate(x, r));
    }
    Ok(json!({ "outputs": outputs }))
}

fn set(app: &mut App, p: &P) -> Reply {
    let c = curve_field(app, p)?;
    if let Some(why) = read_only(app, c.tag, &c.function) {
        return Err(err(code::REFUSED, why));
    }
    let mut f = c.function.clone();
    if let Some(want) = p.opt_str("type")? {
        let kind = FunctionType::from_word(want).ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                format!("type: {want:?} is not a curve type"),
            )
        })?;
        if kind != f.kind {
            f = f
                .with_kind(kind)
                .map_err(|_| err(code::REFUSED, curve::change_to_unevaluated(kind)))?;
        }
    }
    if let Some(v) = p.get("ranged") {
        let ranged = v
            .as_bool()
            .ok_or_else(|| err(code::INVALID_PARAMS, "ranged: true or false"))?;
        f.flags = (f.flags & !1) | u8::from(ranged);
    }
    if let Some(v) = p.get("range") {
        let list = v
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or_else(|| err(code::INVALID_PARAMS, "range: two numbers, low and high"))?;
        f.range = [
            number(&list[0]).map_err(|e| at("range", e))?,
            number(&list[1]).map_err(|e| at("range", e))?,
        ];
    }
    if let Some(v) = p.get("sets") {
        let list = v
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 2)
            .ok_or_else(|| err(code::INVALID_PARAMS, "sets: one or two set objects"))?;
        for (i, s) in list.iter().enumerate() {
            let (sub, values) = merge_set(&f, i, s)?;
            if let Some(sub) = sub {
                f.sub_types[i] = sub;
            }
            f.write_set(i, &values)
                .map_err(|e| err(code::REFUSED, format!("sets[{i}]: {e}")))?;
        }
    }
    let bytes = f.to_bytes();
    let action = if bytes.len() == c.length as usize {
        TagAction::Write(TagWrite {
            tag: c.tag,
            address: c.start,
            bytes,
            label: c.name,
            merge: None,
        })
    } else {
        TagAction::ResizeData {
            tag: c.tag,
            element: c.element,
            layout: c.layout.clone(),
            field: c.field,
            bytes,
            label: c.name,
        }
    };
    tag_fields::perform(app, action)?;
    let c = curve_field(app, p)?;
    Ok(read_json(app, c.tag, c.length, &c.function))
}

/// One set object of `function.set`'s `sets`: the keys present, applied
/// to set `i`'s current values (missing keys keep them), with the wave
/// or shape byte when the object carries one.
fn merge_set(
    f: &Function,
    i: usize,
    v: &Value,
) -> Result<(Option<u8>, Set), agent_proto::RpcError> {
    let bad = |why: String| err(code::INVALID_PARAMS, format!("sets[{i}]: {why}"));
    let object: &Map<String, Value> = v.as_object().ok_or_else(|| bad("a set object".into()))?;
    let get = |k: &str| object.get(k);
    let whole = |k: &str, max: u8| -> Result<u8, agent_proto::RpcError> {
        let n = get(k)
            .and_then(Value::as_u64)
            .ok_or_else(|| bad(format!("{k}: a whole number")))?;
        u8::try_from(n)
            .ok()
            .filter(|s| *s <= max)
            .ok_or_else(|| bad(format!("{k}: 0 to {max}")))
    };
    // A key this type does not take is refused by name.
    let allowed = |keys: &[&str]| -> Result<(), agent_proto::RpcError> {
        for k in object.keys() {
            if !keys.contains(&k.as_str()) {
                return Err(bad(format!(
                    "the {} type has no handle {k:?}",
                    f.kind.word()
                )));
            }
        }
        Ok(())
    };
    let mut sub = None;
    // A number of key `k`, when present.
    let mut one = |k: &str, v: &mut f32| -> Result<(), agent_proto::RpcError> {
        if let Some(x) = get(k) {
            *v = number(x).map_err(|e| bad(e.message))?;
        }
        Ok(())
    };
    let values = match f.set_values(i) {
        Some(Set::Identity) => {
            allowed(&[])?;
            Set::Identity
        }
        Some(Set::Words(current)) => {
            allowed(&["words"])?;
            let words = get("words")
                .and_then(Value::as_array)
                .ok_or_else(|| bad("words: a list of numbers".into()))?;
            let mut out = current;
            if words.len() != out.len() {
                return Err(bad(format!("{} words wanted", out.len())));
            }
            for (k, w) in out.iter_mut().zip(words) {
                *k = number(w).map_err(|e| bad(e.message))?;
            }
            Set::Words(out)
        }
        Some(Set::Transition { mut from, mut to }) => {
            allowed(&["shape", "from", "to"])?;
            if object.contains_key("shape") {
                sub = Some(whole("shape", 5)?);
            }
            one("from", &mut from)?;
            one("to", &mut to)?;
            Set::Transition { from, to }
        }
        Some(Set::Periodic {
            mut frequency,
            mut phase,
            mut minimum,
            mut maximum,
        }) => {
            allowed(&["wave", "frequency", "phase", "minimum", "maximum"])?;
            if object.contains_key("wave") {
                sub = Some(whole("wave", 11)?);
            }
            one("frequency", &mut frequency)?;
            one("phase", &mut phase)?;
            one("minimum", &mut minimum)?;
            one("maximum", &mut maximum)?;
            Set::Periodic {
                frequency,
                phase,
                minimum,
                maximum,
            }
        }
        Some(Set::Linear {
            mut slope,
            mut intercept,
        }) => {
            allowed(&["slope", "intercept"])?;
            one("slope", &mut slope)?;
            one("intercept", &mut intercept)?;
            Set::Linear { slope, intercept }
        }
        Some(Set::LinearKey { mut base, mut keys }) => {
            allowed(&["base", "keys"])?;
            one("base", &mut base)?;
            if let Some(list) = get("keys").and_then(Value::as_array) {
                if list.len() != 3 {
                    return Err(bad("keys: three key objects".into()));
                }
                for (k, key) in list.iter().zip(keys.iter_mut()) {
                    let key_object: &Map<String, Value> = k
                        .as_object()
                        .ok_or_else(|| bad("keys: three key objects".into()))?;
                    for name in key_object.keys() {
                        if !["value", "position", "inverse_span"].contains(&name.as_str()) {
                            return Err(bad(format!("a key has no handle {name:?}")));
                        }
                    }
                    for (name, word) in [
                        ("value", &mut key.value),
                        ("position", &mut key.position),
                        ("inverse_span", &mut key.inverse_span),
                    ] {
                        if let Some(v) = key_object.get(name) {
                            *word = number(v).map_err(|e| bad(e.message))?;
                        }
                    }
                }
            }
            Set::LinearKey { base, keys }
        }
        Some(Set::Spline {
            mut points,
            mut coefficients,
        }) => {
            allowed(&["points", "coefficients"])?;
            if let Some(list) = get("coefficients").and_then(Value::as_array) {
                if list.len() != 4 {
                    return Err(bad("coefficients: four numbers".into()));
                }
                for (k, c) in list.iter().zip(coefficients.iter_mut()) {
                    *c = number(k).map_err(|e| bad(e.message))?;
                }
            }
            if let Some(list) = get("points").and_then(Value::as_array) {
                if list.len() != 4 {
                    return Err(bad("points: four [x, y] pairs".into()));
                }
                for (k, p) in list.iter().zip(points.iter_mut()) {
                    let pair = k
                        .as_array()
                        .filter(|a| a.len() == 2)
                        .ok_or_else(|| bad("points: four [x, y] pairs".into()))?;
                    p[0] = number(&pair[0]).map_err(|e| bad(e.message))?;
                    p[1] = number(&pair[1]).map_err(|e| bad(e.message))?;
                }
            }
            Set::Spline {
                points,
                coefficients,
            }
        }
        Some(Set::Exponent {
            mut minimum,
            mut maximum,
            mut power,
        }) => {
            allowed(&["minimum", "maximum", "power"])?;
            one("minimum", &mut minimum)?;
            one("maximum", &mut maximum)?;
            one("power", &mut power)?;
            Set::Exponent {
                minimum,
                maximum,
                power,
            }
        }
        None => {
            return Err(bad("the type has no layout, so no sets".into()));
        }
    };
    Ok((sub, values))
}

impl Provider for Curves {
    fn methods(&self) -> Vec<MethodDef> {
        vec![READ, SET, SAMPLE]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::FUNCTION_READ => read(app, &p),
            m::FUNCTION_SET => set(app, &p),
            m::FUNCTION_SAMPLE => sample(app, &p),
            _ => return None,
        })
    }
}

#[cfg(test)]
pub(super) mod tests;
