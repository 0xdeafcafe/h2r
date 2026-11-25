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
    install(&mut registry, &agent, true).unwrap();
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
        let page = dialog.page(&START);
        let mut focus = Focus::default();
        let spec = page.spec(&focus);
        assert_eq!(spec.rows[0].label, "Not now");
        assert_eq!(spec.rows[1].label, "Close and start");
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
                    name: format!("|N{}", "W".repeat(60)),
                    source: Source::Rev {
                        rev: format!("|B{}", "W".repeat(60)),
                        commit: "W".repeat(40),
                    },
                    level: Some(format!("|L{}", "W".repeat(60))),
                    own_data: true,
                    operation: Operation::Sim,
                    builds_image: true,
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
            let spec = dialog.page(&START).spec(&focus);
            assert!(check_fallback(&spec).is_empty());
            let preview = From::Synthetic;
            let v = preview.view(&spec, "test").unwrap();
            let report = layout::check(&v, preview.source(), &Rules::for_spec(&spec));
            assert!(report.issues.is_empty(), "{:?}", report.issues);
            let body = v.widget("body").unwrap().text().unwrap();
            assert_eq!(body.lines().count(), 3, "{body}");
            assert!(body.contains(": build, then sim ("));
            assert!(body.contains("own data"));
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
    assert!(matches!(dialog.press(&START, 1), Chosen::Act));
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
    dialog.press(&START, 1);
    dialog.close(42);
    let response = call(takeover::STATUS, params);
    let status: takeover::Status = serde_json::from_value(response["result"].clone()).unwrap();
    assert_eq!(status.decision, Decision::Allowed);
    assert_eq!(
        call(takeover::STATUS, json!({"id": reply.id + 1}))["error"]["code"],
        code::GONE
    );
}

#[test]
fn the_body_names_the_job_its_source_and_what_it_will_do() {
    let mut request = sample_request();
    assert_eq!(
        body(&request),
        "Lab driver: build, then run (about 4 min)\n\"Menu bands check\"\n\
         lab/menu-bands at cb9b315, level ascension"
    );
    request.builds_image = false;
    request.level = None;
    request.own_data = true;
    request.operation = Operation::Sim;
    request.source = Source::Rev {
        rev: "master".into(),
        commit: "0123456789abcdef".into(),
    };
    request.estimate_seconds = 0;
    assert_eq!(
        body(&request),
        "Lab driver: run a sim (time varies)\n\"Menu bands check\"\n\
         master at 0123456, own data"
    );
    request.source = Source::Current {
        commit: Some("b8eff97".into()),
    };
    assert!(body(&request).ends_with("\ninstalled image at b8eff97, own data"));
    request.source = Source::Current { commit: None };
    assert!(body(&request).ends_with("\ninstalled image, own data"));
}

#[test]
fn a_request_without_a_name_or_source_is_refused() {
    let dialog = Dialog::default();
    for request in [
        Request {
            name: " ".into(),
            ..sample_request()
        },
        Request {
            source: Source::Branch {
                name: String::new(),
                commit: "cb9b315".into(),
            },
            ..sample_request()
        },
    ] {
        let err = dialog.submit(request, Instant::now()).unwrap_err();
        assert_eq!(err.code, code::INVALID_PARAMS);
    }
}

fn running_request() -> Request {
    Request {
        ask: Ask::Running,
        ..sample_request()
    }
}

#[test]
fn the_running_question_has_three_answers_and_defaults_to_finish_this_one() {
    for (input, wanted) in [
        (vec![Input::A], Decision::FinishThis),
        (vec![Input::Down, Input::A], Decision::KeepRunning),
        (
            vec![Input::Down, Input::Down, Input::A],
            Decision::StopAndPause,
        ),
        (vec![Input::B], Decision::FinishThis),
    ] {
        let dialog = Arc::new(Dialog::default());
        let id = dialog.submit(running_request(), Instant::now()).unwrap();
        shown(&dialog);
        let page = dialog.page(&RUNNING);
        let mut focus = Focus::default();
        let spec = page.spec(&focus);
        assert_eq!(spec.name, RUNNING_SCREEN);
        let labels: Vec<_> = spec.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Finish this one", "Keep running", "Stop and pause"]
        );
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
fn the_running_question_times_out_and_ignores_the_start_questions_rows() {
    let dialog = Dialog::default();
    let now = Instant::now();
    let id = dialog.submit(running_request(), now).unwrap();
    shown(&dialog);
    // A press on the other question's rows doesn't answer this one.
    dialog.press(&START, 1);
    assert_eq!(dialog.status(id, now).unwrap(), Decision::Pending);
    let expired = now + Duration::from_secs(61);
    assert_eq!(dialog.status(id, expired).unwrap(), Decision::TimedOut);
    dialog.press(&RUNNING, 2);
    assert_eq!(dialog.status(id, expired).unwrap(), Decision::TimedOut);
}

#[test]
fn the_running_question_fits_three_lines_above_three_rows() {
    let dialog = Arc::new(Dialog::default());
    dialog
        .submit(
            Request {
                requester: format!("{}|A\r\n", "W".repeat(60)),
                name: format!("|N{}", "W".repeat(60)),
                ..running_request()
            },
            Instant::now(),
        )
        .unwrap();
    for focus in [Focus(0), Focus(1), Focus(2)] {
        let spec = dialog.page(&RUNNING).spec(&focus);
        assert!(check_fallback(&spec).is_empty());
        let preview = From::Synthetic;
        let v = preview.view(&spec, "test").unwrap();
        let report = layout::check(&v, preview.source(), &Rules::for_spec(&spec));
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        let body = v.widget("body").unwrap().text().unwrap();
        assert_eq!(body.lines().count(), 3, "{body}");
        assert!(body.ends_with("\nwhile you play. What now?"), "{body}");
        assert!(!body.contains(['|', '\r']));
    }
}

#[test]
fn the_running_body_names_who_runs_the_job() {
    assert_eq!(
        body(&running_request()),
        "Lab driver is running\n\"Menu bands check\"\nwhile you play. What now?"
    );
}

#[test]
fn a_start_request_reads_as_before_and_a_running_one_says_so() {
    let start = serde_json::to_value(sample_request()).unwrap();
    assert!(start.get("ask").is_none(), "{start}");
    let running = serde_json::to_value(running_request()).unwrap();
    assert_eq!(running["ask"], "running");
    let back: Request = serde_json::from_value(running).unwrap();
    assert_eq!(back.ask, Ask::Running);
    for (decision, name) in [
        (Decision::StopAndPause, "stop_and_pause"),
        (Decision::KeepRunning, "keep_running"),
        (Decision::FinishThis, "finish_this"),
    ] {
        assert_eq!(serde_json::to_value(decision).unwrap(), name);
    }
}

fn alongside_request() -> Request {
    Request {
        alongside: true,
        ..sample_request()
    }
}

#[test]
fn start_alongside_is_a_row_only_when_the_request_offers_it() {
    let rows = |request: Request| -> Vec<String> {
        let dialog = Arc::new(Dialog::default());
        dialog.submit(request.clone(), Instant::now()).unwrap();
        shown(&dialog);
        let spec = dialog.page(form(&request)).spec(&Focus::default());
        spec.rows.iter().map(|r| r.label.clone()).collect()
    };
    assert_eq!(
        rows(alongside_request()),
        ["Not now", "Start alongside", "Close and start"]
    );
    assert_eq!(rows(sample_request()), ["Not now", "Close and start"]);
}

#[test]
fn the_alongside_question_answers_by_row_and_defaults_to_decline() {
    for (input, wanted) in [
        (vec![Input::A], Decision::Declined),
        (vec![Input::Down, Input::A], Decision::Alongside),
        (vec![Input::Down, Input::Down, Input::A], Decision::Allowed),
        (vec![Input::B], Decision::Declined),
    ] {
        let dialog = Arc::new(Dialog::default());
        let id = dialog.submit(alongside_request(), Instant::now()).unwrap();
        shown(&dialog);
        let page = dialog.page(&ALONGSIDE);
        let mut focus = Focus::default();
        let spec = page.spec(&focus);
        assert_eq!(spec.name, ALONGSIDE_SCREEN);
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
fn a_dialog_without_the_row_cannot_answer_alongside() {
    let dialog = Dialog::default();
    let now = Instant::now();
    let id = dialog.submit(sample_request(), now).unwrap();
    shown(&dialog);
    // The other start dialog's rows aren't on screen for this request.
    dialog.press(&ALONGSIDE, 1);
    assert_eq!(dialog.status(id, now).unwrap(), Decision::Pending);
    // Its own row 1 is Close and start.
    dialog.press(&START, 1);
    assert_eq!(dialog.status(id, now).unwrap(), Decision::Allowed);
}

#[test]
fn the_alongside_question_fits_three_lines_above_three_rows() {
    let dialog = Arc::new(Dialog::default());
    dialog
        .submit(
            Request {
                requester: format!("{}|A\r\n", "W".repeat(60)),
                name: format!("|N{}", "W".repeat(60)),
                level: Some("W".repeat(60)),
                own_data: true,
                ..alongside_request()
            },
            Instant::now(),
        )
        .unwrap();
    for focus in [Focus(0), Focus(1), Focus(2)] {
        let spec = dialog.page(&ALONGSIDE).spec(&focus);
        assert!(check_fallback(&spec).is_empty());
        let preview = From::Synthetic;
        let v = preview.view(&spec, "test").unwrap();
        let report = layout::check(&v, preview.source(), &Rules::for_spec(&spec));
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        let body = v.widget("body").unwrap().text().unwrap();
        assert_eq!(body.lines().count(), 3, "{body}");
    }
}

#[test]
fn the_alongside_flag_is_left_out_when_offered_and_the_answer_has_a_name() {
    let offered: Request = serde_json::from_str(
        r#"{"requester":"a","name":"b","operation":"run","builds_image":false,
            "source":{"kind":"current","commit":null},"level":null,"own_data":false,
            "estimate_seconds":0,"timeout_seconds":60}"#,
    )
    .unwrap();
    assert!(offered.alongside);
    assert!(serde_json::to_value(&offered).unwrap().get("alongside").is_none());
    assert_eq!(
        serde_json::to_value(sample_request()).unwrap()["alongside"],
        false
    );
    assert_eq!(
        serde_json::to_value(Decision::Alongside).unwrap(),
        "alongside"
    );
}
