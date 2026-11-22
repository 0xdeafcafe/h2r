//! `doc.open {variant: {script}}`, `variant.get`'s `script` and
//! `variant.set {option}`: a variant of a game type script, through the
//! API (docs/game/rules-variants.md, slice 11).

use super::*;

#[test]
fn a_variant_of_a_script_opens_reads_and_edits_by_option_name() {
    let t = Temp::new("script-variant");
    let mut app = app(&t);
    let mut api = Api::new();
    call(
        &mut api,
        &mut app,
        m::DOC_OPEN,
        json!({"variant": {"script": "Superbounce (built in)"}, "discard": true}),
    );
    let got = call(&mut api, &mut app, m::VARIANT_GET, json!({}));
    let script = &got["script"];
    assert_eq!(script["found"], true, "{got}");
    assert_eq!(script["name"], "Superbounce");
    assert_eq!(script["quick"].as_array().unwrap().len(), 5);
    let option = |v: &Value, name: &str| {
        v["script"]["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["name"] == name)
            .cloned()
            .unwrap()
    };
    assert_eq!(option(&got, "bounce_points")["value"], 1);
    assert_eq!(option(&got, "bounce_points")["quick"], true);

    // Two edits of one option are one undo step.
    for n in [4, 3] {
        let set = call(
            &mut api,
            &mut app,
            m::VARIANT_SET,
            json!({"option": "bounce_points", "value": n}),
        );
        assert_eq!(set["value"], n);
    }
    let got = call(&mut api, &mut app, m::VARIANT_GET, json!({}));
    assert_eq!(option(&got, "bounce_points")["value"], 3);
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "variant"}));
    let got = call(&mut api, &mut app, m::VARIANT_GET, json!({}));
    assert_eq!(option(&got, "bounce_points")["value"], 1);

    // An unknown option, an out-of-range value and an unknown script are
    // refused.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::VARIANT_SET,
            json!({"option": "nope", "value": 1})
        )),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::VARIANT_SET,
            json!({"option": "bounce_points", "value": 99})
        )),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::DOC_OPEN,
            json!({"variant": {"script": "no such script"}, "discard": true})
        )),
        code::INVALID_PARAMS
    );
}

#[test]
fn a_variant_with_no_script_reports_none() {
    let t = Temp::new("script-variant-none");
    let mut app = app(&t);
    let mut api = Api::new();
    call(
        &mut api,
        &mut app,
        m::DOC_OPEN,
        json!({"variant": {"new": "slayer"}, "discard": true}),
    );
    let got = call(&mut api, &mut app, m::VARIANT_GET, json!({}));
    assert!(got["script"].is_null(), "{got}");
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::VARIANT_SET,
            json!({"option": "bounce_points", "value": 1})
        )),
        code::INVALID_PARAMS
    );
}
