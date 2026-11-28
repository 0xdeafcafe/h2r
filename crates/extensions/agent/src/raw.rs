//! `world.raw` (docs/runtime/agent-api.md, "world.raw"): an object's body,
//! physics component, Havok entity, motion and shape as raw guest bytes,
//! as `live_telemetry::raw` reads them. `world.poke` writes within one of
//! those parts at a tick boundary. `h2 agent raw-diff` compares two reads.

use agent_proto::raw::{
    PARTS, POKE_MAX, Poke, PokeParams, Raw, RawParams, RawPart, from_hex, method, to_hex,
};
use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use live_telemetry::raw::{self as rd, Part};
use serde_json::Value;

use crate::exec::GameCtx;
use crate::sim::SimHandle;
use crate::{Access, Agent};

/// Registers `world.raw` and `world.poke`, using sim's host-write checks.
pub fn install(agent: &Agent, sim: &SimHandle) {
    agent.add_game_method(method::RAW, Access::Read, raw_method);
    let sim = sim.clone();
    agent.add_game_method(method::POKE, Access::Claimed, move |v, ctx| {
        poke_method(v, ctx, &sim)
    });
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn parse_part(name: &str) -> Result<Part, RpcError> {
    Part::parse(name)
        .ok_or_else(|| invalid(format!("unknown part {name} (one of {})", PARTS.join(", "))))
}

/// The parts asked for; all of them without `parts`.
fn parts(p: &RawParams) -> Result<Vec<Part>, RpcError> {
    let Some(names) = &p.parts else {
        return Ok(Part::ALL.to_vec());
    };
    if names.is_empty() {
        return Err(invalid("parts is empty; leave it out for all of them"));
    }
    names.iter().map(|name| parse_part(name)).collect()
}

fn datum(ctx: &mut GameCtx<'_>, id: &str) -> Result<u32, RpcError> {
    let objects = ctx
        .reader
        .objects()
        .map_err(|e| RpcError::new(code::UNAVAILABLE, format!("reading the world: {e}")))?;
    let (datum, _) = crate::havok::resolve(&objects, id)?;
    Ok(datum)
}

fn read_regions(
    m: &dyn GuestMemory,
    id: &str,
    datum: u32,
    parts: &[Part],
    len: Option<u32>,
) -> Result<Vec<rd::Region>, RpcError> {
    rd::object_parts(m, datum, parts, len)
        .map_err(|e| {
            RpcError::new(
                code::UNAVAILABLE,
                format!("reading the objects at {:08x}: {e:?}", e.addr),
            )
        })?
        .ok_or_else(|| RpcError::new(code::GONE, format!("{id} ({datum:08x}) has no object body")))
}

fn raw_method(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    let p: RawParams = serde_json::from_value(v).map_err(|e| invalid(e.to_string()))?;
    let parts = parts(&p)?;
    if p.len == Some(0) {
        return Err(invalid("len must be at least 1"));
    }
    let datum = datum(ctx, &p.id)?;
    let regions = ctx.with_guest(|m| read_regions(m, &p.id, datum, &parts, p.len))?;
    Ok(serde_json::to_value(answer(&p.id, datum, &regions)).expect("raw serialises"))
}

fn poke_method(v: Value, ctx: &mut GameCtx<'_>, sim: &SimHandle) -> Result<Value, RpcError> {
    let p: PokeParams = serde_json::from_value(v).map_err(|e| invalid(e.to_string()))?;
    let part = parse_part(&p.part)?;
    if p.hex.is_empty() || p.hex.len() > 2 * POKE_MAX {
        return Err(invalid(format!("hex must encode 1 to {POKE_MAX} bytes")));
    }
    let bytes = from_hex(&p.hex).ok_or_else(|| invalid("hex must have two digits per byte"))?;
    sim.writable_session(ctx)?;
    let datum = datum(ctx, &p.id)?;

    let result = ctx.with_guest(|m| {
        let region = read_regions(m, &p.id, datum, &[part], None)?
            .into_iter()
            .next()
            .expect("requested one part");
        let (base, previous) = region.read.map_err(|why| {
            RpcError::new(code::UNAVAILABLE, format!("{} {}: {why}", p.id, p.part))
        })?;

        let end = p
            .offset
            .checked_add(bytes.len() as u32)
            .filter(|&end| end as usize <= previous.len())
            .ok_or_else(|| {
                invalid(format!(
                    "offset and length exceed {} bytes for {}",
                    previous.len(),
                    p.part
                ))
            })?;
        let before = &previous[p.offset as usize..end as usize];
        let addr = base
            .checked_add(p.offset)
            .filter(|addr| addr.checked_add(bytes.len() as u32 - 1).is_some())
            .ok_or_else(|| invalid("guest address range overflows"))?;
        let fault = |e: guest_layout::MemFault| {
            RpcError::new(
                code::UNAVAILABLE,
                format!("guest memory at {:#x} faulted", e.addr),
            )
        };
        m.write(addr, &bytes).map_err(fault)?;
        let after = m.bytes(addr, bytes.len()).map_err(fault)?;
        Ok(Poke {
            id: p.id,
            datum: format!("{datum:08x}"),
            part: p.part,
            base: format!("{base:08x}"),
            offset: p.offset,
            before: to_hex(before),
            after: to_hex(&after),
        })
    })?;
    Ok(serde_json::to_value(result).expect("poke serialises"))
}

/// `world.raw` from what was read.
pub(crate) fn answer(id: &str, datum: u32, regions: &[rd::Region]) -> Raw {
    Raw {
        id: id.to_owned(),
        datum: format!("{datum:08x}"),
        parts: regions
            .iter()
            .map(|r| match &r.read {
                Ok((base, bytes)) => RawPart {
                    part: r.part.as_str().to_owned(),
                    base: Some(format!("{base:08x}")),
                    hex: Some(to_hex(bytes)),
                    missing: None,
                },
                Err(why) => RawPart {
                    part: r.part.as_str().to_owned(),
                    missing: Some(why.clone()),
                    ..Default::default()
                },
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_proto::sim::{SessionKind, SessionView};
    use agent_proto::{Kind, Object};
    use guest_layout::testing::FakeGuest;
    use guest_layout::view::{fake, objects};
    use live_telemetry::layout as l;
    use serde_json::json;

    use super::*;
    use crate::{GameStatus, Reader};

    const OBJECTS_HDR: u32 = 0x8010_0000;
    const BODY: u32 = 0x8020_0000;
    const COMPONENTS_HDR: u32 = 0x8030_0000;
    const COMPONENT_DATA: u32 = 0x8031_0000;
    const RIGIDS: u32 = 0x8040_0000;
    const ENTITY: u32 = 0x8041_0000;
    const MOTION: u32 = 0x8042_0000;

    struct Game {
        g: Option<FakeGuest>,
        objects: Vec<Object>,
    }

    impl Reader for Game {
        fn status(&mut self) -> GameStatus {
            GameStatus {
                tick: Some(100),
                ..Default::default()
            }
        }
        fn objects(&mut self) -> Result<Vec<Object>, String> {
            Ok(self.objects.clone())
        }
        fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn GuestMemory)) -> bool {
            match &mut self.g {
                Some(g) => {
                    f(g);
                    true
                }
                None => false,
            }
        }
    }

    /// Player 0's unit: a `0x464`-byte body, physics component 1, one
    /// rigid body with a motion and no shape.
    fn game() -> Game {
        let mut g = FakeGuest::new();
        g.put_u32(guest_layout::points::OBJECTS.base, OBJECTS_HDR);
        let table = fake::datums::<objects::Header>(&mut g, OBJECTS_HDR, 16);
        let (h, hdr) = fake::push(&mut g, &table);
        g.map(BODY, 0x464);
        hdr.set_body(&mut g, BODY).unwrap();
        g.put_u16(hdr.addr + l::OBJECT_BODY_SIZE, 0x464);
        g.put_u32(BODY + 4, 0xdead_beef);
        g.put_u32(BODY + l::BODY_PHYSICS, 0xab00_0001);
        g.put_u32(l::PHYSICS_COMPONENTS_PTR, COMPONENTS_HDR);
        g.put_u32(COMPONENTS_HDR + l::DATUM_DATA, COMPONENT_DATA);
        let comp = COMPONENT_DATA + l::PHYSICS_COMPONENT_SIZE;
        g.map(COMPONENT_DATA, 2 * l::PHYSICS_COMPONENT_SIZE);
        g.put_u32(comp + l::COMPONENT_BODIES, RIGIDS);
        g.put_u32(comp + l::COMPONENT_BODY_COUNT, 1);
        g.put_u32(RIGIDS + l::COMPONENT_BODY_RIGID, ENTITY);
        g.map(ENTITY, 0x200);
        g.put_u32(ENTITY + l::RIGID_MOTION, MOTION);
        g.map(MOTION, 0x140);
        g.put_f32(MOTION + l::MOTION_POSITION + 8, -3.2);
        Game {
            g: Some(g),
            objects: vec![Object {
                id: "player.0".into(),
                kind: Kind::Player,
                unit: Some(format!("{:08x}", h.raw())),
                ..Default::default()
            }],
        }
    }

    struct Harness {
        agent: Agent,
        conn: crate::ConnId,
        rx: std::sync::mpsc::Receiver<String>,
    }

    impl Harness {
        fn new() -> Harness {
            Self::in_session(SessionKind::Local)
        }

        fn in_session(kind: SessionKind) -> Harness {
            let agent = Agent::new();
            let sim = crate::sim::install(
                &agent,
                Box::new(crate::sim::bots::NotLanded),
                Some(Box::new(move |_| SessionView {
                    kind,
                    ..Default::default()
                })),
            );
            install(&agent, &sim);
            let (tx, rx) = std::sync::mpsc::sync_channel(8);
            let conn = agent.connect(tx);
            Harness { agent, conn, rx }
        }

        /// The reply to one call, answered at a boundary of `game`.
        fn call(&self, game: &mut Game, p: Value) -> Value {
            self.send(method::RAW, p);
            self.agent.between_slices(game, 0);
            self.reply()
        }

        fn send(&self, method: &str, params: Value) {
            let req = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
            self.agent.submit(self.conn, &req.to_string());
        }

        fn reply(&self) -> Value {
            let line = self.rx.recv_timeout(Duration::from_secs(5)).unwrap();
            serde_json::from_str(&line).unwrap()
        }
    }

    fn error_code(v: &Value) -> i64 {
        v["error"]["code"].as_i64().unwrap_or_else(|| panic!("{v}"))
    }

    #[test]
    fn answers_every_part_with_hex_and_a_reason_for_the_missing_one() {
        let h = Harness::new();
        let mut game = game();
        let v = h.call(&mut game, json!({"id": "player.0"}));
        let r: Raw = serde_json::from_value(v["result"].clone()).unwrap();
        assert_eq!(r.id, "player.0");
        let names: Vec<&str> = r.parts.iter().map(|p| p.part.as_str()).collect();
        assert_eq!(names, PARTS);

        let object = &r.parts[0];
        assert_eq!(object.base.as_deref(), Some("80200000"));
        let bytes = from_hex(object.hex.as_deref().unwrap()).unwrap();
        assert_eq!(bytes.len(), 0x464);
        assert_eq!(bytes[4..8], 0xdead_beefu32.to_le_bytes());
        assert_eq!(r.parts[2].base.as_deref(), Some("80410000"));
        let motion = from_hex(r.parts[3].hex.as_deref().unwrap()).unwrap();
        let z = (l::MOTION_POSITION + 8) as usize;
        assert_eq!(motion[z..z + 4], (-3.2f32).to_le_bytes());
        // The entity has no shape.
        let shape = &r.parts[4];
        assert!(shape.hex.is_none() && shape.base.is_none());
        assert!(shape.missing.as_deref().unwrap().contains("no shape"));
        // The stamp.
        assert!(v["result"]["at"].is_object(), "{v}");
    }

    #[test]
    fn parts_pick_and_order_the_answer() {
        let h = Harness::new();
        let mut game = game();
        let p = json!({"id": "player.0", "parts": ["motion", "object"], "len": 16});
        let v = h.call(&mut game, p);
        let r: Raw = serde_json::from_value(v["result"].clone()).unwrap();
        let names: Vec<&str> = r.parts.iter().map(|p| p.part.as_str()).collect();
        assert_eq!(names, ["object", "motion"]);
        // The header's size wins over len.
        assert_eq!(r.parts[0].hex.as_ref().unwrap().len(), 2 * 0x464);
    }

    #[test]
    fn bad_params_and_missing_objects_are_refused() {
        let h = Harness::new();
        let mut game = game();
        let bad = [
            json!({}),
            json!({"id": "player.0", "parts": ["havok"]}),
            json!({"id": "player.0", "parts": []}),
            json!({"id": "player.0", "len": 0}),
            json!({"id": "nonsense"}),
            json!({"id": "player.0", "extra": 1}),
        ];
        for p in bad {
            let v = h.call(&mut game, p.clone());
            assert_eq!(error_code(&v), code::INVALID_PARAMS, "{p} {v}");
        }
        let v = h.call(&mut game, json!({"id": "player.3"}));
        assert_eq!(error_code(&v), code::GONE, "{v}");

        // A listed id whose datum is stale.
        game.objects[0].unit = Some("12340007".into());
        let v = h.call(&mut game, json!({"id": "player.0"}));
        assert_eq!(error_code(&v), code::GONE, "{v}");

        // No guest memory: unavailable, not a panic.
        let mut none = game;
        none.g = None;
        let v = h.call(&mut none, json!({"id": "player.0"}));
        assert_eq!(error_code(&v), code::UNAVAILABLE, "{v}");
    }

    fn claim(h: &Harness, game: &mut Game) {
        h.send(agent_proto::method::CLAIM, Value::Null);
        h.agent.between_slices(game, 0);
        assert_eq!(h.reply()["result"]["controller"], true);
    }

    #[test]
    fn poke_writes_at_the_tick_and_returns_before_and_after() {
        let h = Harness::new();
        let mut game = game();
        claim(&h, &mut game);
        h.send(
            method::POKE,
            json!({"id": "player.0", "offset": 4, "hex": "AB"}),
        );
        assert_eq!(game.g.as_ref().unwrap().u32(BODY + 4).unwrap(), 0xdead_beef);
        h.agent.on_tick(&mut game);

        let v = h.reply();
        assert_eq!(v["result"]["before"], "ef", "{v}");
        assert_eq!(v["result"]["after"], "ab");
        assert_eq!(v["result"]["part"], "object");
        assert_eq!(v["result"]["at"]["boundary"], "tick");
        assert_eq!(game.g.as_ref().unwrap().u32(BODY + 4).unwrap(), 0xdead_beab);

        // The maximum length ending at the exact end of a non-default part.
        let p = json!({
            "id": "player.0", "part": "motion", "offset": 0x100, "hex": "12".repeat(64)
        });
        h.send(method::POKE, p);
        h.agent.on_tick(&mut game);
        let v = h.reply();
        assert_eq!(v["result"]["base"], "80420000", "{v}");
        assert_eq!(v["result"]["after"], "12".repeat(64));
        assert_eq!(
            game.g.as_ref().unwrap().bytes(MOTION + 0x100, 64).unwrap(),
            vec![0x12; 64]
        );
    }

    #[test]
    fn poke_refuses_invalid_ranges_and_hex_without_writing() {
        let h = Harness::new();
        let mut game = game();
        claim(&h, &mut game);
        let original = game.g.as_ref().unwrap().bytes(BODY, 0x465).unwrap();
        for (offset, hex) in [
            (0x463, "abcd".to_owned()),
            (u32::MAX, "ab".to_owned()),
            (4, "ab".repeat(65)),
            (4, String::new()),
            (4, "a".to_owned()),
            (4, "+a".to_owned()),
        ] {
            h.send(
                method::POKE,
                json!({"id": "player.0", "offset": offset, "hex": hex}),
            );
            h.agent.on_tick(&mut game);
            let v = h.reply();
            assert_eq!(error_code(&v), code::INVALID_PARAMS, "{v}");
            assert_eq!(
                game.g.as_ref().unwrap().bytes(BODY, 0x465).unwrap(),
                original
            );
        }
    }

    #[test]
    fn poke_refuses_a_connection_without_the_claim() {
        let h = Harness::new();
        let mut game = game();
        let params = json!({"id": "player.0", "offset": 4, "hex": "ab"});
        h.send(method::POKE, params.clone());
        h.agent.on_tick(&mut game);
        assert_eq!(error_code(&h.reply()), code::NOT_CONTROLLER);

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let other = h.agent.connect(tx);
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": agent_proto::method::CLAIM});
        h.agent.submit(other, &req.to_string());
        h.agent.on_tick(&mut game);
        let line = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["result"]["controller"], true);

        h.send(method::POKE, params);
        h.agent.on_tick(&mut game);
        assert_eq!(error_code(&h.reply()), code::NOT_CONTROLLER);
        assert_eq!(game.g.as_ref().unwrap().u32(BODY + 4).unwrap(), 0xdead_beef);
    }

    #[test]
    fn poke_refuses_disallowed_sessions_and_slice_boundaries() {
        let p = json!({"id": "player.0", "offset": 4, "hex": "ab"});
        let mut game = game();
        for kind in [
            SessionKind::CoopHost,
            SessionKind::CoopGuest,
            SessionKind::MpClient,
            SessionKind::Unknown,
        ] {
            let h = Harness::in_session(kind);
            claim(&h, &mut game);
            h.send(method::POKE, p.clone());
            h.agent.on_tick(&mut game);
            let v = h.reply();
            assert_eq!(error_code(&v), code::INVALID_REQUEST, "{v}");
            assert_eq!(v["error"]["data"]["session"]["writes"], false);
        }

        let h = Harness::new();
        claim(&h, &mut game);
        h.send(method::POKE, p);
        h.agent.between_slices(&mut game, 0);
        assert_eq!(error_code(&h.reply()), code::UNAVAILABLE);
        assert_eq!(game.g.as_ref().unwrap().u32(BODY + 4).unwrap(), 0xdead_beef);
    }

    #[test]
    fn the_method_is_listed() {
        let h = Harness::new();
        h.agent.submit(
            h.conn,
            r#"{"jsonrpc":"2.0","id":1,"method":"session.methods"}"#,
        );
        h.agent.between_slices(&mut game(), 0);
        let line = h.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(line.contains(method::RAW), "{line}");
        assert!(line.contains(method::POKE), "{line}");
    }
}
