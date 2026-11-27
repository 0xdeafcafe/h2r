//! `map.new`'s refusals (docs/editor/agent-api.md, "New maps"): the build
//! itself is `starter-map`'s tests; here the wire says why it can't start.

use super::*;

#[test]
fn map_new_names_why_it_cannot_start() {
    let t = Temp::new("map-new");
    let mut app = app(&t);
    let mut api = Api::new();
    // The invented source has no coagulation or 05a_deltaapproach.
    for kind in ["multiplayer", "campaign"] {
        let r = api.call(&mut app, m::MAP_NEW, json!({"name": "Yard", "kind": kind}));
        let e = r.as_ref().expect_err("refused").message.clone();
        assert_eq!(code_of(r), code::REFUSED, "{kind}");
        assert!(e.contains("coagulation") || e.contains("05a_deltaapproach"), "{e}");
    }
    assert_eq!(
        code_of(api.call(&mut app, m::MAP_NEW, json!({"name": "Yard", "kind": "firefight"}))),
        code::INVALID_PARAMS
    );
    assert_eq!(code_of(api.call(&mut app, m::MAP_NEW, json!({"name": "9 lives"}))), code::REFUSED);
    assert_eq!(code_of(api.call(&mut app, m::MAP_NEW, json!({}))), code::INVALID_PARAMS);
    assert!(app.new_map_running().is_none());
    assert!(!t.path("data").join("maps").join("yard").exists(), "nothing written");
}
