use serde_json::json;

use crate::{method as m, parse_call, split};

fn c(line: &str) -> (String, serde_json::Value) {
    parse_call(&split(line).unwrap()).unwrap()
}

#[test]
fn short_forms_name_their_methods_and_params() {
    assert_eq!(c("status"), (m::STATUS.into(), json!(null)));
    assert_eq!(c("methods"), (m::METHODS.into(), json!(null)));
    assert_eq!(
        c("set vehicles:2 position [1, 2, 3.5]"),
        (
            m::SCENARIO_SET.into(),
            json!({"block":"vehicles","index":2,"field":"position","value":[1,2,3.5]})
        )
    );
    assert_eq!(
        c("set netgameflags:0 team 3"),
        (
            m::SCENARIO_SET.into(),
            json!({"block":"netgameflags","index":0,"field":"team","value":3})
        )
    );
    assert_eq!(
        c("move scenery:1 1,2,3"),
        (
            m::SCENARIO_MOVE.into(),
            json!({"block":"scenery","index":1,"position":[1.0,2.0,3.0]})
        )
    );
    assert_eq!(
        c("add weapons 0"),
        (m::SCENARIO_ADD.into(), json!({"block":"weapons","like":0}))
    );
    assert_eq!(
        c("save-as /tmp/x.map map"),
        (
            m::DOC_SAVE_AS.into(),
            json!({"path":"/tmp/x.map","doc":"map"})
        )
    );
    assert_eq!(
        c("undo variant"),
        (m::DOC_UNDO.into(), json!({"doc":"variant"}))
    );
    assert_eq!(c("undo"), (m::DOC_UNDO.into(), json!(null)));
    assert_eq!(c("huds ascension"), (m::HUD_LIST.into(), json!({"map":"ascension"})));
    assert_eq!(c("hud"), (m::HUD_READ.into(), json!(null)));
    assert_eq!(c("render-hud /tmp/h.png"), (m::HUD_RENDER.into(), json!({"out":"/tmp/h.png"})));
    assert_eq!(
        c("tags vehi warthog"),
        (m::TAG_LIST.into(), json!({"group":"vehi","name":"warthog"}))
    );
    assert_eq!(
        c("screen settings/main"),
        (
            m::UI_PICK.into(),
            json!({"screen":"settings","state":"main"})
        )
    );
    assert_eq!(
        c("setting 12 3"),
        (m::VARIANT_SET.into(), json!({"setting":12,"value":3}))
    );
    assert_eq!(
        c("playlist 4"),
        (m::PLAYLIST_GET.into(), json!({"playlist":4}))
    );
}

#[test]
fn a_method_by_name_takes_json_params() {
    assert_eq!(
        c(r#"scenario.get {"block": "vehicles", "index": 0}"#),
        (
            m::SCENARIO_GET.into(),
            json!({"block":"vehicles","index":0})
        )
    );
    assert_eq!(c("doc.list"), (m::DOC_LIST.into(), json!(null)));
}

#[test]
fn bad_short_forms_say_why() {
    for bad in [
        "nope",
        "object vehicles",
        "object vehicles:x",
        "move a:0 1,2",
        "status extra",
        "set a:0 position",
        "scenario.get {not json}",
        "tag",
    ] {
        let words = match split(bad) {
            Ok(w) => w,
            Err(_) => continue,
        };
        assert!(parse_call(&words).is_err(), "{bad}");
    }
}

#[test]
fn requests_and_responses_round_trip_on_the_wire() {
    use agent_proto::{Request, Response, RpcError, ServerLine};
    let r = Request::new(7, m::SCENARIO_SET, json!({"block":"vehicles","index":0}));
    let line = serde_json::to_string(&r).unwrap();
    assert!(!line.contains('\n'));
    let back: Request = serde_json::from_str(&line).unwrap();
    assert_eq!(back, r);
    let ok = Response::ok(json!(7), json!({"changed": true}));
    let back: ServerLine = serde_json::from_str(&serde_json::to_string(&ok).unwrap()).unwrap();
    assert!(matches!(back, ServerLine::Response(r) if r == ok));
    let mut e = RpcError::new(crate::code::REFUSED, "the block is full");
    e.data = Some(json!({"reason": "growth"}));
    let err = Response::err(json!(7), e);
    let back: ServerLine = serde_json::from_str(&serde_json::to_string(&err).unwrap()).unwrap();
    assert!(matches!(back, ServerLine::Response(r) if r == err));
}

#[test]
fn the_socket_lives_in_the_cache_directory() {
    let p = crate::default_socket();
    if std::env::var_os(crate::ENV).is_none() {
        assert!(p.ends_with("h2recomp/studio.sock"), "{}", p.display());
    }
    assert!(crate::output_dir().ends_with("h2recomp/studio"));
}
