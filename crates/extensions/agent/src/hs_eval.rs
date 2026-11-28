//! `hs.eval`: one HaloScript expression, compiled against the loaded map's
//! scripts and evaluated at a tick boundary (docs/runtime/agent-api.md,
//! "HaloScript state"; docs/editor/rich-editors.md, "Scripts").
//!
//! The expression is wrapped in a static script of a copy of the loaded
//! scenario's tables (`hs::compile`, with the builtin catalog read from the
//! running image and the names the map's own calls give them), then run
//! on `hs::vm`, the native evaluator that mirrors the retail one
//! (docs/engine/hs_vm.md), over a snapshot: the scenario's tables and the
//! live cells of its globals, read at this boundary. Nothing is written
//! back to the game. A builtin that reaches the game's objects, AI or sound
//! needs the guest-backed engine (docs/engine/hs_vm.md, "Not verified") and
//! is refused by name; control flow, arithmetic, comparison, `set` of a
//! global (in the snapshot) and reads of scenario globals run.
use agent_proto::{RpcError, code};
use guest_layout::{GuestMemory, MemFault, datum::{DatumArray, layout}, points};
use hs::vm::{Cell, Engine, EngineCall, EngineError, HsType, Value, Vm, VmImage};
use hs::{Catalog, Message, Naming, Options, Scenario};
use serde_json::{Value as Json, json};

use crate::{Access, Agent};

use engine::hs_pool::{EXTERNALS, GLOBALS as CELLS};

/// Longest expression, in bytes.
const MAX_SOURCE: usize = 4096;
/// Addresses below this hold the program's code (a sanity check on the
/// catalog's evaluator pointers).
const CODE_END: u32 = 0x0040_0000;

pub fn install(agent: &Agent) {
    agent.add_game_method("hs.eval", Access::Read, |params, ctx| {
        let request = Request::parse(&params)?;
        ctx.with_guest(|m| eval(m, &request))
    });
}

/// `hs.eval`'s parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub expression: String,
    /// The result's type name; absent, the expression's own where it is
    /// fixed (a call of a builtin or script that returns one, or a global).
    pub ty: Option<String>,
}

impl Request {
    pub fn parse(params: &Json) -> Result<Self, RpcError> {
        let bad = |what: &str| RpcError::new(code::INVALID_PARAMS, what.to_owned());
        let expression = params
            .get("expression")
            .and_then(Json::as_str)
            .ok_or_else(|| bad("expression: a string, one HaloScript expression"))?
            .to_owned();
        if expression.trim().is_empty() {
            return Err(bad("expression is empty"));
        }
        if expression.len() > MAX_SOURCE {
            return Err(bad(&format!("expression is over {MAX_SOURCE} bytes")));
        }
        let ty = match params.get("type") {
            None | Some(Json::Null) => None,
            Some(Json::String(t)) => Some(t.clone()),
            Some(_) => return Err(bad("type: a type name, such as real or short")),
        };
        Ok(Self { expression, ty })
    }
}

/// The map's scripts and global cells as the game holds them at this
/// boundary.
pub struct Snapshot<'a> {
    pub scenario: &'a Scenario,
    /// Builtins, with the names the scenario's calls give them.
    pub catalog: &'a Catalog,
    /// The value bits of each scenario global, in declaration order.
    pub cells: &'a [u32],
    /// The game's tick.
    pub tick: i32,
}

fn unavailable(message: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, message)
}

fn fault(e: MemFault) -> RpcError {
    unavailable(format!("HS state unreadable at {:#010x}", e.addr))
}

/// The game's memory, read as the script crate's tables want it.
fn reader(m: &dyn GuestMemory) -> impl Fn(u32, usize) -> hs::Result<Vec<u8>> + '_ {
    move |at, len| {
        let mut bytes = vec![0; len];
        m.read(at, &mut bytes)
            .map_err(|e| hs::Error::Invalid(format!("unreadable at {:#010x}", e.addr)))?;
        Ok(bytes)
    }
}

/// Read the snapshot's parts from the game and evaluate.
fn eval(m: &dyn GuestMemory, request: &Request) -> Result<Json, RpcError> {
    let base = m.u32(points::SCENARIO.base).map_err(fault)?;
    if base == 0 {
        return Err(unavailable("HS state has not been initialized"));
    }
    let read = reader(m);
    let scenario = Scenario::read_memory(base, &read)
        .map_err(|e| unavailable(format!("the loaded map's scripts do not read: {e}")))?;
    let mut catalog = Catalog::from_memory(&read, &|at| (0x1000..CODE_END).contains(&at))
        .map_err(|e| unavailable(format!("the builtin table does not read: {e}")))?;
    catalog
        .observe(&scenario)
        .map_err(|e| unavailable(format!("the loaded map's scripts do not check: {e}")))?;
    let cells = global_cells(m, scenario.globals().len())?;
    let tick = (|| {
        let time = m.u32(live_telemetry::layout::GAME_TIME_PTR).ok()?;
        (time != 0)
            .then(|| m.u32(time + live_telemetry::layout::TIME_TICK).ok())
            .flatten()
    })()
    .unwrap_or(0) as i32;
    evaluate(
        &Snapshot {
            scenario: &scenario,
            catalog: &catalog,
            cells: &cells,
            tick,
        },
        request,
    )
}

/// The value bits of each scenario global's live cell, as `hs.globals`
/// finds them.
fn global_cells(m: &dyn GuestMemory, count: usize) -> Result<Vec<u32>, RpcError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let array = m.u32(CELLS).map_err(fault)?;
    if array == 0 {
        return Err(unavailable("HS state has not been initialized"));
    }
    let array = DatumArray::at(array);
    if array
        .header_field(m, layout::MAXIMUM_COUNT)
        .map_err(fault)?
        < (count + EXTERNALS as usize) as i32
    {
        return Err(unavailable("invalid HS global count"));
    }
    (0..count)
        .map(|i| {
            let cell = array
                .get_absolute(m, (EXTERNALS as usize + i) as i32)
                .map_err(fault)?
                .ok_or_else(|| unavailable("scenario global has no live cell"))?;
            m.u32(cell + 4).map_err(fault)
        })
        .collect()
}

/// Compile the expression into a copy of the snapshot's tables and run it.
pub fn evaluate(snapshot: &Snapshot<'_>, request: &Request) -> Result<Json, RpcError> {
    let Snapshot {
        scenario, catalog, ..
    } = *snapshot;
    let naming = Naming::new(catalog, None);
    let options = Options {
        naming: Some(&naming),
        ..Options::default()
    };
    let kind = catalog
        .script_kinds()
        .get(3)
        .ok_or_else(|| unavailable("the builtin table names no static script kind"))?;
    let mut name = "hs_eval_expression".to_owned();
    while scenario.script(&name).is_ok()
        || scenario
            .globals()
            .iter()
            .any(|g| g.name().ok() == Some(name.as_str()))
    {
        name.push('_');
    }
    // The body follows the name: a script takes no arguments, and the
    // compiler reads a `()` there as a call that names no function.
    let wrap = |ty: &str| format!("(script {kind} {ty} {name}\n{}\n)", request.expression);
    let compile = |ty: &str| {
        hs::compile(scenario, catalog, &wrap(ty), &options)
            .map_err(|messages| source_problems(&messages, &request.expression))
    };
    let type_index = |ty: &str| {
        catalog
            .types()
            .iter()
            .position(|t| t == ty)
            .map(|i| i as u16)
            .ok_or_else(|| {
                RpcError::new(
                    code::INVALID_PARAMS,
                    format!("type: `{ty}` is not a type of this game"),
                )
            })
    };
    let (compiled, ty) = match &request.ty {
        Some(ty) => (compile(ty.as_str())?, type_index(ty.as_str())?),
        None => {
            let probe = compile("void")?;
            let ty = natural_type(&probe.scenario, catalog, &name, &request.expression).ok_or_else(|| {
                RpcError::new(
                    code::INVALID_PARAMS,
                    "type: the expression's type is not fixed (a begin, an if, a literal); name the result type",
                )
            })?;
            let ty_name = catalog
                .type_name(ty)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
            (compile(ty_name)?, ty)
        }
    };
    let root = compiled
        .scenario
        .script(&name)
        .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
        .root();
    let image = VmImage::without_stubs();
    let mut vm = Vm::new(&compiled.scenario, catalog, &image);
    vm.set_enabled(true);
    for (i, bits) in snapshot.cells.iter().enumerate() {
        vm.set_global(i, Cell(*bits));
    }
    let mut engine = Frozen {
        tick: snapshot.tick,
        seed: 0,
    };
    let cell = match vm.evaluate(&mut engine, root, false) {
        Ok(Ok(cell)) => cell,
        Ok(Err(slot)) => {
            vm.stop_thread(slot);
            return Err(unavailable(
                "the expression sleeps: hs.eval answers at one tick boundary and cannot wait",
            ));
        }
        Err(e) => return Err(unavailable(format!("the evaluator stopped: {e}"))),
    };
    let ty_name = catalog.type_name(ty).unwrap_or("?");
    Ok(json!({
        "type": ty_name,
        "type_index": ty,
        "value": json_value(&vm, Value::decode(HsType(ty), cell)),
        "bits": cell.0,
    }))
}

/// The expression's own type, where its root fixes one: a call of a
/// builtin or script that returns a plain value, or a bare global's name.
fn natural_type(compiled: &Scenario, catalog: &Catalog, name: &str, source: &str) -> Option<u16> {
    let node = compiled.node(compiled.script(name).ok()?.root()).ok()?;
    let ty = if node.script_call() {
        compiled
            .scripts()
            .get(usize::from(node.opcode()))?
            .return_type()
    } else if node.atom() {
        let word = source.trim();
        compiled
            .globals()
            .iter()
            .find(|g| g.name().ok() == Some(word))?
            .value_type()
    } else {
        catalog.function(node.opcode()).ok()?.return_type()
    };
    // Unparsed, special form, function name and passthrough fix nothing.
    (ty > 3).then_some(ty)
}

/// The compiler's problems with the places counted in the expression, not
/// in the script that wraps it. A `(` the expression never closes swallows
/// the wrapper's `)`, so the compiler blames the wrapper's own `(`: the
/// message points at the expression's instead.
fn source_problems(messages: &[Message], expression: &str) -> RpcError {
    let rows: Vec<Json> = messages
        .iter()
        .map(|m| {
            let unclosed = (m.code == "syntax.open" && m.line == 1)
                .then(|| first_unclosed(expression))
                .flatten();
            let (line, column) = unclosed.unwrap_or(if m.line >= 2 {
                (m.line - 1, m.column)
            } else {
                (m.line, m.column)
            });
            json!({"line": line, "column": column, "code": m.code, "text": m.text})
        })
        .collect();
    let first = rows.first().map_or(String::new(), |r| {
        format!(
            "{}:{}: {}",
            r["line"],
            r["column"],
            r["text"].as_str().unwrap_or_default()
        )
    });
    let more = rows.len().saturating_sub(1);
    let message = if more == 0 {
        first
    } else {
        format!("{first} (and {more} more)")
    };
    RpcError {
        code: code::INVALID_PARAMS,
        message,
        data: Some(json!({"problems": rows})),
    }
}

/// The 1-based line and column of the outermost `(` the text never
/// closes, skipping strings and `;` comments.
fn first_unclosed(text: &str) -> Option<(u32, u32)> {
    let mut open: Vec<(u32, u32)> = Vec::new();
    let (mut line, mut column) = (1u32, 0u32);
    let (mut string, mut comment) = (false, false);
    for c in text.chars() {
        column += c.len_utf16() as u32;
        if c == '\n' {
            (line, column, comment) = (line + 1, 0, false);
            continue;
        }
        match c {
            _ if comment => {}
            '"' => string = !string,
            _ if string => {}
            ';' => comment = true,
            '(' => open.push((line, column)),
            ')' => {
                open.pop();
            }
            _ => {}
        }
    }
    open.first().copied()
}

fn json_value(vm: &Vm<'_>, value: Value) -> Json {
    match value {
        Value::Void => Json::Null,
        Value::Boolean(b) => json!(b),
        Value::Real(r) => json!(r),
        Value::Short(s) => json!(s),
        Value::Long(l) => json!(l),
        Value::String(s) => {
            let text = vm.string(s.cell()).unwrap_or_default();
            json!(String::from_utf8_lossy(text))
        }
        Value::StringId(bits) => json!(bits),
        Value::Index { index, .. } => json!(index),
        Value::Handle { bits, .. } | Value::Raw { bits, .. } => json!(bits),
    }
}

/// The evaluator's engine over a snapshot: it knows the tick, answers no
/// world queries and refuses every builtin that would reach the game.
struct Frozen {
    tick: i32,
    seed: u32,
}

impl Engine for Frozen {
    fn game_time(&self) -> i32 {
        self.tick
    }
    fn random_seed(&mut self) -> &mut u32 {
        &mut self.seed
    }
    fn call(&mut self, call: EngineCall<'_>) -> Result<Cell, EngineError> {
        let who = call
            .name
            .map_or_else(|| format!("builtin {}", call.index), str::to_owned);
        Err(EngineError::Failed(format!(
            "`{who}` needs the running game; hs.eval runs over a snapshot of the map's scripts and globals"
        )))
    }
    fn object_type(&mut self, _object: u32) -> Option<u8> {
        None
    }
    fn object_from_name(&mut self, _name: i16) -> u32 {
        u32::MAX
    }
    fn object_list_new(&mut self, _objects: &[u32]) -> u32 {
        u32::MAX
    }
    fn object_list_from_ai(&mut self, _ai: u32) -> u32 {
        u32::MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use hs::fixture::{Builtin, Exe, TYPES, Tables};

    /// Invented types and builtins at the indices the compiler reads.
    fn exe() -> Exe {
        let mut types: Vec<String> = (0..TYPES).map(|i| format!("t{i}")).collect();
        for (i, n) in [
            (2, "function_name"),
            (3, "passthrough"),
            (4, "void"),
            (5, "boolean"),
            (6, "real"),
            (7, "short"),
            (8, "long"),
            (9, "string"),
            (10, "script"),
        ] {
            types[i] = n.to_owned();
        }
        let b = |returns, flags, parameters: &[u16]| Builtin {
            returns,
            flags,
            parameters: parameters.to_vec(),
            usage: None,
        };
        Exe {
            types,
            builtins: vec![
                b(4, 0, &[]),     // 0: unused
                b(6, 0, &[6, 6]), // 1: invented_add
                b(3, 2, &[]),     // 2: begin (reads its own arguments)
            ],
            external_type: 6,
            ..Exe::default()
        }
    }

    /// A short global `count`, a real global `ratio`, and a script that
    /// uses `invented_add` so the map names that builtin.
    fn tables() -> Tables {
        let mut t = Tables::new();
        let four = t.value(7, 4);
        t.global_definition("count", 7, four);
        let half = t.value(6, 0.5f32.to_bits());
        t.global_definition("ratio", 6, half);
        let (one, two) = (t.value(6, 1.0f32.to_bits()), t.value(6, 2.0f32.to_bits()));
        let add = t.call(1, "invented_add", 6, &[one, two]);
        let begin = t.call(2, "begin", 4, &[add]);
        t.script("main", 0, 4, begin);
        t
    }

    fn run(cells: &[u32], expression: &str, ty: Option<&str>) -> Result<Json, RpcError> {
        let scenario = tables().scenario();
        let mut catalog = exe().catalog();
        catalog.observe(&scenario).unwrap();
        let request = Request {
            expression: expression.to_owned(),
            ty: ty.map(str::to_owned),
        };
        evaluate(
            &Snapshot {
                scenario: &scenario,
                catalog: &catalog,
                cells,
                tick: 7,
            },
            &request,
        )
    }

    #[test]
    fn a_global_reads_with_its_own_type_and_the_live_cell() {
        let v = run(&[0xfffe, 1.5f32.to_bits()], "count", None).unwrap();
        assert_eq!(
            (v["type"].as_str(), v["value"].clone()),
            (Some("short"), json!(-2))
        );
        let v = run(&[0, 1.5f32.to_bits()], "ratio", None).unwrap();
        assert_eq!(
            (v["type"].as_str(), v["value"].clone()),
            (Some("real"), json!(1.5))
        );
        assert_eq!(v["bits"], 1.5f32.to_bits());
    }

    #[test]
    fn a_global_converts_to_the_named_type() {
        let v = run(&[4, 0], "count", Some("real")).unwrap();
        assert_eq!(
            (v["type"].as_str(), v["value"].clone()),
            (Some("real"), json!(4.0))
        );
    }

    #[test]
    fn a_builtin_that_needs_the_game_is_refused_by_name() {
        let e = run(&[0, 0], "(invented_add 1.0 2.0)", None).unwrap_err();
        assert_eq!(e.code, code::UNAVAILABLE);
        assert!(
            e.message.contains("invented_add") && e.message.contains("snapshot"),
            "{}",
            e.message
        );
    }

    #[test]
    fn a_parse_error_returns_the_compilers_text_at_the_expressions_place() {
        let e = run(&[0, 0], "(invented_add 1.0", None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
        let rows = e.data.unwrap()["problems"].clone();
        assert_eq!(rows[0]["line"], 1);
        assert_eq!(rows[0]["code"], "syntax.open");
        assert_eq!(
            (rows[0]["column"].clone(), e.message.starts_with("1:1: ")),
            (json!(1), true),
            "{}",
            e.message
        );
    }

    #[test]
    fn an_unknown_name_is_a_problem_and_an_unfixed_type_asks_for_one() {
        let e = run(&[0, 0], "(no_such_call 1)", None).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
        assert!(e.data.is_some());
        let e = run(&[0, 0], "(begin count)", None).unwrap_err();
        assert!(e.message.contains("name the result type"), "{}", e.message);
        let e = run(&[0, 0], "count", Some("nonesuch")).unwrap_err();
        assert!(e.message.contains("nonesuch"));
    }

    #[test]
    fn the_request_names_what_is_wrong() {
        assert!(Request::parse(&json!({})).is_err());
        assert!(Request::parse(&json!({"expression": "  "})).is_err());
        assert!(Request::parse(&json!({"expression": "x", "type": 3})).is_err());
        let r = Request::parse(&json!({"expression": "count", "type": "real"})).unwrap();
        assert_eq!(
            (r.expression.as_str(), r.ty.as_deref()),
            ("count", Some("real"))
        );
    }

    /// The same, read from the guest: the scenario's tables at its tag's
    /// address, the builtin table at the executable's, the cells in the
    /// global array.
    #[test]
    fn the_guest_supplies_the_tables_the_catalog_and_the_cells() {
        const BASE: u32 = 0x0900_0000;
        let mut m = FakeGuest::new();
        let xbe = exe().xbe();
        m.put(0x0020_0000, &xbe[0x400..]);
        let mut payload = vec![0u8; 0x2000];
        tables().write(&mut payload, BASE, 0x240);
        m.put(BASE, &payload);
        let cells = 0x0a00_0000;
        let data = 0x0a01_0000;
        let index = EXTERNALS;
        m.put_u32(points::SCENARIO.base, BASE)
            .put_u32(CELLS, cells)
            .put_u32(cells + layout::MAXIMUM_COUNT, 3072)
            .put_u32(cells + layout::ELEMENT_SIZE, 8)
            .put_u32(cells + layout::HIGH_WATER, index + 2)
            .put_u32(cells + layout::DATA, data)
            .put_u16(data + 8 * index, 1)
            .put_u32(data + 4 + 8 * index, 9)
            .put_u16(data + 8 * (index + 1), 1)
            .put_u32(data + 4 + 8 * (index + 1), 2.5f32.to_bits());
        let v = eval(
            &m,
            &Request {
                expression: "ratio".into(),
                ty: None,
            },
        )
        .unwrap();
        assert_eq!(
            (v["type"].as_str(), v["value"].clone()),
            (Some("real"), json!(2.5))
        );
        let v = eval(
            &m,
            &Request {
                expression: "count".into(),
                ty: None,
            },
        )
        .unwrap();
        assert_eq!(v["value"], 9);
        m.put_u32(points::SCENARIO.base, 0);
        assert_eq!(
            eval(
                &m,
                &Request {
                    expression: "count".into(),
                    ty: None
                }
            )
            .unwrap_err()
            .code,
            code::UNAVAILABLE
        );
    }
}
