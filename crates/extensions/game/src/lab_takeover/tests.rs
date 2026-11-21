use super::*;
use crate::ui_kit::layout::{self, Rules};
use crate::ui_kit::page::{Focus, ListEffect, Page};
use crate::ui_kit::preview::From;
use crate::ui_kit::spec::check_fallback;

// Exercise the class's real row handlers; the fixture stands in for the
// guest loader and destructor.
fn shown(dialog: &Dialog) {
    dialog.prompt().as_mut().unwrap().screen = Screen::Open(42);
}

#[test]
fn consent_uses_the_signed_ui_update_point() {
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    let agent = agent::Agent::new();
    install(&mut registry, &agent).unwrap();
    let plan = registry.build(&hooks::Base).unwrap().text();
    assert!(
        plan.lines().any(|line| {
            line.contains("UI_UPDATE") && line.contains("open or expire a lab request")
        }),
        "{plan}"
    );
}

#[test]
fn page_has_two_working_answers_and_defaults_to_decline() {
    for (input, wanted) in [
        (vec![Input::A], Decision::Declined),
        (vec![Input::Down, Input::A], Decision::Allowed),
        (vec![Input::B], Decision::Declined),
    ] {
        let dialog = Arc::new(Dialog::default());
        let id = dialog.submit(sample_request(), Instant::now()).unwrap();
        shown(&dialog);
        let page = dialog.page();
        let mut focus = Focus::default();
        let spec = page.spec(&focus);
        assert_eq!(spec.rows[0].label, "Not now");
        assert_eq!(spec.rows[1].label, "Allow");
        assert!(check_fallback(&spec).is_empty());
        let effect = input.into_iter().map(|i| page.step(&mut focus, i)).last();
        assert!(matches!(
            effect,
            Some(ListEffect::Action | ListEffect::Close)
        ));
        dialog.close(42);
        assert_eq!(dialog.status(id, Instant::now()).unwrap(), wanted);
    }
}

#[test]
fn dialog_content_fits_at_either_focus_and_neutralizes_remote_text() {
    for estimate_seconds in [0, 59, u32::MAX] {
        let dialog = Arc::new(Dialog::default());
        let id = dialog
            .submit(
                Request {
                    requester: format!("{}|A\r\n", "W".repeat(60)),
                    revision: format!("|B{}", "W".repeat(60)),
                    operation: Operation::Build,
                    estimate_seconds,
                    ..sample_request()
                },
                Instant::now(),
            )
            .unwrap();
        assert_eq!(
            dialog.status(id, Instant::now()).unwrap(),
            Decision::Pending
        );
        for focus in [Focus(0), Focus(1)] {
            let spec = dialog.page().spec(&focus);
            assert!(check_fallback(&spec).is_empty());
            let preview = From::Synthetic;
            let v = preview.view(&spec, "test").unwrap();
            let report = layout::check(&v, preview.source(), &Rules::for_spec(&spec));
            assert!(report.issues.is_empty(), "{:?}", report.issues);
            let body = v.widget("body").unwrap().text().unwrap();
            assert_eq!(body.lines().count(), 3, "{body}");
            assert!(body.contains("requests a lab build."));
            assert!(!body.contains(['|', '\r']));
        }
    }
}

#[test]
fn an_expired_dialog_cannot_grant_consent_and_clears_on_close() {
    let dialog = Dialog::default();
    let request = sample_request();
    let now = Instant::now();
    let id = dialog.submit(request.clone(), now).unwrap();
    shown(&dialog);
    let expired = now + Duration::from_secs(61);
    assert_eq!(dialog.status(id, expired).unwrap(), Decision::TimedOut);
    assert!(matches!(dialog.press(1), Chosen::Act));
    assert_eq!(dialog.status(id, expired).unwrap(), Decision::TimedOut);
    assert_eq!(
        dialog.submit(request.clone(), expired).unwrap_err().code,
        code::BUSY
    );
    dialog.close(42);
    assert!(dialog.submit(request, expired).unwrap() > id);
}

#[test]
fn agent_takeover_round_trip() {
    let glue = crate::agent_glue::AgentGlue::local();
    let dialog = Arc::new(Dialog::default());
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let conn = glue.agent.connect(tx);
    let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
    let call = |method, params| -> Value {
        let message = json!({"jsonrpc": "2.0", "id": 42, "method": method, "params": params});
        glue.agent.submit(conn, &message.to_string());
        glue.after_slice(&rt, 0);
        let answer: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(answer["id"], 42, "{answer}");
        answer
    };
    assert_eq!(call(takeover::INFO, Value::Null)["result"]["window"], false);
    assert_eq!(
        call(takeover::REQUEST, json!(sample_request()))["error"]["code"],
        code::METHOD_NOT_FOUND
    );
    add_info(&glue.agent, true);
    add_consent(&glue.agent, &dialog);
    let info: takeover::Info =
        serde_json::from_value(call(takeover::INFO, Value::Null)["result"].clone()).unwrap();
    assert_eq!(
        info,
        takeover::Info {
            window: true,
            pid: Some(std::process::id())
        }
    );
    let methods = call(agent_proto::method::METHODS, Value::Null);
    for name in [takeover::INFO, takeover::REQUEST, takeover::STATUS] {
        assert!(methods.to_string().contains(name), "{methods}");
    }
    let request = json!(sample_request());
    let mut invalid = request.clone();
    invalid["timeout_seconds"] = json!(0);
    assert_eq!(
        call(takeover::REQUEST, invalid)["error"]["code"],
        code::INVALID_PARAMS
    );
    let accepted = call(takeover::REQUEST, request.clone());
    let reply: takeover::Requested = serde_json::from_value(accepted["result"].clone()).unwrap();
    let params = json!(takeover::StatusParams { id: reply.id });
    assert_eq!(
        call(takeover::STATUS, params.clone())["result"]["decision"],
        "pending"
    );
    assert_eq!(
        call(takeover::REQUEST, request)["error"]["code"],
        code::BUSY
    );
    shown(&dialog);
    dialog.press(1);
    dialog.close(42);
    let response = call(takeover::STATUS, params);
    let status: takeover::Status = serde_json::from_value(response["result"].clone()).unwrap();
    assert_eq!(status.decision, Decision::Allowed);
    assert_eq!(
        call(takeover::STATUS, json!({"id": reply.id + 1}))["error"]["code"],
        code::GONE
    );
}
