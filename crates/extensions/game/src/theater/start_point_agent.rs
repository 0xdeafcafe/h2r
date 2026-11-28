//! `theater.start_point.capture` and `theater.start_point.load`
//! (docs/runtime/agent-api.md, "Theater transport"): the experiments of
//! slice 15 in docs/game/theater-design.md. Capture writes the running
//! game's arena and extension records as a tagged start point file; load
//! reads one, puts the arena back and reports whether the loaded state
//! hashes as the tag says.
//!
//! The calls are game methods, answered at a tick boundary with guest
//! memory at hand. The handlers are plain functions over guest memory and
//! [`ExtRecords`], so the tests run them over a fake game.
use super::start_point::{MAX_IDS, MAX_STORED_ARENA, MAX_STORED_EXT, StartPoint};
use super::start_point_state::{self as state, ExtRecords, Global};
use agent::Access;
use agent_proto::theater::{
    START_POINT_CAPTURE, START_POINT_LOAD, StartPointCapture, StartPointCaptured, StartPointLoad,
    StartPointLoaded,
};
use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use serde_json::Value;
use std::path::Path;

/// The most bytes of a start point file a load reads: the two stored frames
/// and a generous header.
const MAX_FILE: u64 = (MAX_STORED_ARENA + MAX_STORED_EXT + 1024) as u64;

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, message)
}

fn unavailable(message: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, message)
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(if params.is_null() {
        Value::Object(Default::default())
    } else {
        params
    })
    .map_err(|e| invalid(e.to_string()))
}

fn key(text: &str) -> Result<&[u8], RpcError> {
    if text.is_empty() {
        return Err(invalid("key: a tag key is required"));
    }
    Ok(text.as_bytes())
}

fn hex_bytes(text: &str) -> Result<Vec<u8>, RpcError> {
    if text.len() % 2 != 0 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("ids: an even number of hex digits"));
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16))
        .collect::<Result<_, _>>()
        .map_err(|_| invalid("ids: an even number of hex digits"))
}

fn hash_text(hash: u64) -> String {
    format!("{hash:016x}")
}

fn hash_value(text: &str) -> Result<u64, RpcError> {
    u64::from_str_radix(text.trim_start_matches("0x"), 16)
        .map_err(|_| invalid("film_hash: up to 16 hex digits"))
}

/// `theater.start_point.capture`: the state at the end of `tick`, written
/// to the file the params name.
pub fn capture_call(
    params: Value,
    mem: &dyn GuestMemory,
    ext: &dyn ExtRecords,
    tick: Option<u64>,
) -> Result<Value, RpcError> {
    let p: StartPointCapture = parse(params)?;
    let key = key(&p.key)?;
    let ids = p
        .ids
        .as_deref()
        .map(hex_bytes)
        .transpose()?
        .unwrap_or_default();
    if ids.len() > MAX_IDS {
        return Err(invalid(format!("ids: at most {MAX_IDS} bytes")));
    }
    let tick = tick
        .and_then(|t| u32::try_from(t).ok())
        .ok_or_else(|| unavailable("no game tick to take a start point at"))?;
    let point = state::capture(mem, ext, tick, ids).map_err(|e| unavailable(format!("{e:#}")))?;
    let bytes = point
        .encode(key)
        .map_err(|e| unavailable(format!("{e:#}")))?;
    std::fs::write(&p.path, &bytes).map_err(|e| unavailable(format!("writing {}: {e}", p.path)))?;
    Ok(serde_json::to_value(StartPointCaptured {
        path: p.path,
        tick,
        arena_hash: hash_text(point.arena_hash),
        arena_bytes: point.arena.len() as u64,
        ext_bytes: point.ext.len() as u64,
        stored_bytes: bytes.len() as u64,
    })
    .expect("serialises"))
}

/// `theater.start_point.load`: the file read and checked in full before
/// the game is touched; then the arena put back and hashed.
pub fn load_call(
    params: Value,
    mem: &mut dyn GuestMemory,
    ext: &mut dyn ExtRecords,
) -> Result<Value, RpcError> {
    let p: StartPointLoad = parse(params)?;
    let key = key(&p.key)?;
    let bytes = read_file(Path::new(&p.path))?;
    let film_hash = match &p.film_hash {
        Some(text) => hash_value(text)?,
        None => StartPoint::declared_arena_hash(&bytes).map_err(|e| invalid(format!("{e:#}")))?,
    };
    let point = StartPoint::decode(&bytes, key, state::arena_len(), film_hash)
        .map_err(|e| RpcError::new(code::INVALID_REQUEST, format!("{e:#}")))?;
    let after = state::load(mem, ext, &point).map_err(|e| unavailable(format!("{e:#}")))?;
    let check = point.check_loaded(after);
    Ok(serde_json::to_value(StartPointLoaded {
        tick: point.tick,
        arena_hash: hash_text(point.arena_hash),
        loaded_hash: hash_text(after),
        matches: check.is_ok(),
        error: check.err().map(|e| e.to_string()),
    })
    .expect("serialises"))
}

fn read_file(path: &Path) -> Result<Vec<u8>, RpcError> {
    let size = std::fs::metadata(path)
        .map_err(|e| invalid(format!("{}: {e}", path.display())))?
        .len();
    if size > MAX_FILE {
        return Err(invalid(format!(
            "{}: too large for a start point",
            path.display()
        )));
    }
    std::fs::read(path).map_err(|e| invalid(format!("{}: {e}", path.display())))
}

/// Adds both methods to the game agent. They write the running game's
/// arena (load) and a file (both), so they are act methods; use them on a
/// local game, never in a Live or co-op session.
pub fn install(agent: &agent::Agent) {
    agent.add_game_method(START_POINT_CAPTURE, Access::Act, |params, ctx| {
        let tick = ctx.at.tick;
        let mut out = None;
        let ran = ctx.reader.with_guest(&mut |mem| {
            out = Some(capture_call(params.clone(), mem, &Global, tick));
        });
        out.filter(|_| ran)
            .unwrap_or_else(|| Err(unavailable("no guest memory here (the game isn't running)")))
    });
    agent.add_game_method(START_POINT_LOAD, Access::Act, |params, ctx| {
        let mut out = None;
        let ran = ctx.reader.with_guest(&mut |mem| {
            out = Some(load_call(params.clone(), mem, &mut Global));
        });
        out.filter(|_| ran)
            .unwrap_or_else(|| Err(unavailable("no guest memory here (the game isn't running)")))
    });
}

#[cfg(test)]
mod tests {
    use super::super::start_point_state::fakes::{Ext, guest};
    use super::*;
    use serde_json::json;

    struct Dir(std::path::PathBuf);
    impl Dir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("start-point-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn file(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_captured_file_loads_into_another_state_and_matches() {
        let dir = Dir::new("round");
        let path = dir.file("a.sp");
        let ext = Ext {
            bytes: vec![5, 6],
            refuse: false,
        };
        let captured = capture_call(
            json!({"path": path, "key": "synthetic", "ids": "0a0b"}),
            &guest(3),
            &ext,
            Some(300),
        )
        .unwrap();
        assert_eq!(captured["tick"], 300);
        assert_eq!(captured["arena_bytes"], state::arena_len() as u64);
        assert_eq!(captured["ext_bytes"], 2);
        assert!(captured["stored_bytes"].as_u64().unwrap() > 0);

        let mut target = guest(9);
        let mut target_ext = Ext::default();
        let loaded = load_call(
            json!({"path": path, "key": "synthetic"}),
            &mut target,
            &mut target_ext,
        )
        .unwrap();
        assert_eq!(loaded["matches"], true);
        assert_eq!(loaded["tick"], 300);
        assert_eq!(loaded["arena_hash"], captured["arena_hash"]);
        assert_eq!(loaded["loaded_hash"], captured["arena_hash"]);
        assert_eq!(target_ext.bytes, [5, 6]);
    }

    #[test]
    fn a_film_hash_that_differs_or_a_wrong_key_is_refused_before_the_game_is_touched() {
        let dir = Dir::new("refused");
        let path = dir.file("a.sp");
        capture_call(
            json!({"path": path, "key": "k"}),
            &guest(3),
            &Ext::default(),
            Some(1),
        )
        .unwrap();
        let mut target = guest(9);
        let before = state::current_hash(&target, &Ext::default()).unwrap();
        let e = load_call(
            json!({"path": path, "key": "k", "film_hash": "0000000000000001"}),
            &mut target,
            &mut Ext::default(),
        )
        .unwrap_err();
        assert!(e.message.contains("not the film's"), "{}", e.message);
        assert!(
            load_call(
                json!({"path": path, "key": "other"}),
                &mut target,
                &mut Ext::default()
            )
            .is_err()
        );
        assert_eq!(
            state::current_hash(&target, &Ext::default()).unwrap(),
            before
        );
    }

    #[test]
    fn one_changed_byte_in_the_file_is_refused() {
        let dir = Dir::new("flip");
        let path = dir.file("a.sp");
        capture_call(
            json!({"path": path, "key": "k"}),
            &guest(3),
            &Ext::default(),
            Some(1),
        )
        .unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let at = bytes.len() / 2;
        bytes[at] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        let mut target = guest(9);
        assert!(
            load_call(
                json!({"path": path, "key": "k"}),
                &mut target,
                &mut Ext::default()
            )
            .is_err()
        );
    }

    #[test]
    fn bad_params_are_named() {
        let dir = Dir::new("params");
        let path = dir.file("a.sp");
        let g = guest(1);
        let e = Ext::default();
        for params in [
            json!({"path": path}),
            json!({"path": path, "key": ""}),
            json!({"path": path, "key": "k", "ids": "0"}),
            json!({"path": path, "key": "k", "ids": "zz"}),
            json!({"path": path, "key": "k", "ids": "00".repeat(MAX_IDS + 1)}),
            json!({"path": path, "key": "k", "extra": 1}),
        ] {
            let err = capture_call(params, &g, &e, Some(1)).unwrap_err();
            assert_eq!(err.code, code::INVALID_PARAMS);
        }
        let err = capture_call(json!({"path": path, "key": "k"}), &g, &e, None).unwrap_err();
        assert_eq!(err.code, code::UNAVAILABLE);
        let mut t = guest(1);
        let err = load_call(
            json!({"path": dir.file("missing"), "key": "k"}),
            &mut t,
            &mut Ext::default(),
        )
        .unwrap_err();
        assert_eq!(err.code, code::INVALID_PARAMS);
        let err = load_call(
            json!({"path": path, "key": "k", "film_hash": "xyz"}),
            &mut t,
            &mut Ext::default(),
        )
        .unwrap_err();
        assert_eq!(err.code, code::INVALID_PARAMS);
    }

    #[test]
    fn a_game_with_no_arena_answers_unavailable() {
        let dir = Dir::new("noarena");
        let mut m = guest(1);
        m.write(
            super::super::start_point_state::fakes::POINTER,
            &0u32.to_le_bytes(),
        )
        .unwrap();
        let err = capture_call(
            json!({"path": dir.file("a.sp"), "key": "k"}),
            &m,
            &Ext::default(),
            Some(1),
        )
        .unwrap_err();
        assert_eq!(err.code, code::UNAVAILABLE);
    }
}
