//! The rules workbench (docs/live/live-api.md "Rules scripts"): check,
//! run and the built-ins are public and rate-limited; an account saves a
//! script that verifies, once per hash; anyone reads it back, its IR and
//! an author's list; a second server on the log reads the same. Run with
//! the server's own timers and with none (the Worker's Durable Object).
mod common;
use axum::{http::Method, http::StatusCode};
use common::*;
use live_proto::lxm;
use live_server::app::Timers;
use live_server::store::{EventStore, MemoryStore};
use serde_json::{Value, json};
use std::sync::Arc;

const BROKEN: &str = "rules \"Broken\" 1\ncarrier slayer\non kill(v, k) {\n    score(k, )\n}\n";

async fn post(h: &Harness, path: &str, body: Value, headers: &[(&str, String)]) -> Resp {
    let mut all = vec![("content-type", "application/json".to_string())];
    all.extend(headers.iter().cloned());
    h.call(Method::POST, path, &all, body.to_string()).await
}

#[tokio::test]
async fn check_reports_the_hash_the_budget_and_errors_with_places() {
    let h = harness().await;
    let r = post(&h, "/v1/rules/check", json!({"source": rules::builtin::SLAYER}), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let v = r.json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["name"], "Slayer");
    let want = rules::compile(rules::builtin::SLAYER).unwrap().id().to_string();
    assert_eq!(v["hash"], want);
    assert_eq!(v["report"]["handlers"][0]["event"], "kill");
    assert!(v["report"]["worst_tick_ops"].as_u64().unwrap() > 0);
    assert_eq!(v["report"]["budget"], rules::vm::TICK_BUDGET);
    assert_eq!(v["report"]["ir_limit"], rules::ir::MAX_IR_BYTES);
    assert_eq!(v["report"]["slots"][0]["scope"], "global");
    assert_eq!(v["report"]["capabilities"], json!([]));
    // Slayer needs nothing of a map.
    assert_eq!(v["report"]["labels"], json!([]));
    assert_eq!(v["report"]["minimal"], json!([]));

    let r = post(&h, "/v1/rules/check", json!({"source": BROKEN}), &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert_eq!(v["ok"], false);
    assert_eq!(v["errors"][0]["line"], 4);
    assert!(v["report"].is_null());

    let caps = "rules \"T\" 1\ncarrier slayer\nteams 2\nrequires player.team\non spawn(p) { set_team(p, team 1) }\n";
    let v = post(&h, "/v1/rules/check", json!({"source": caps}), &[]).await.json();
    assert_eq!(v["report"]["capabilities"][0]["name"], "player.team");
    assert_eq!(v["report"]["capabilities"][0]["words"], "moves players between teams");
    assert_eq!(v["report"]["unhostable"], json!(["set_team"]));

    let big = "x".repeat(live_proto::rules::MAX_SOURCE + 1);
    let r = post(&h, "/v1/rules/check", json!({"source": big}), &[]).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.error());
    let r = post(&h, "/v1/rules/check", json!({"src": "x"}), &[]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn run_traces_a_scenario_on_the_fake_game() {
    let h = harness().await;
    let scenario = json!({
        "players": [{"slot": 0, "team": 0}, {"slot": 1, "team": 1}],
        "seed": 7,
        "round_time": 100,
        "ticks": 30,
        "timeline": [
            {"tick": 3, "events": [{"type": "kill", "victim": 1, "killer": 0}]},
            {"tick": 9, "events": [{"type": "kill", "victim": 0}, {"type": "spawn", "player": 1}]},
        ],
    });
    let body = json!({"source": rules::builtin::SLAYER, "scenario": scenario});
    let r = post(&h, "/v1/rules/run", body.clone(), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let v = r.json();
    assert_eq!(v["ok"], true);
    let ticks: Vec<u64> = v["ticks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["tick"].as_u64().unwrap())
        .collect();
    assert_eq!(ticks, [3, 9]);
    assert_eq!(v["ticks"][0]["ran"], json!(["kill"]));
    assert_eq!(
        v["ticks"][0]["calls"][0],
        json!({"entry": "score", "args": ["player 0", "1"]})
    );
    assert_eq!(v["ticks"][1]["calls"][0]["args"], json!(["player 0", "-1"]));
    assert_eq!(v["last_tick"], 30);
    assert_eq!(v["scores"], json!([{"slot": 0, "score": 0}, {"slot": 1, "score": 0}]));
    // The same request, the same trace.
    assert_eq!(post(&h, "/v1/rules/run", body, &[]).await.json(), v);

    // A map the script has no requirement of runs, under either name, with
    // the things' classes; one with an unknown field or class is refused.
    for key in ["map", "labelled"] {
        let mut with = scenario.clone();
        with[key] = json!([{"label": "hill", "object": 40, "number": 0, "class": "volume"}]);
        let r = post(&h, "/v1/rules/run", json!({"source": rules::builtin::SLAYER, "scenario": with}), &[]).await;
        assert_eq!(r.status, StatusCode::OK, "{key}: {}", r.error());
        assert_eq!(r.json()["ok"], true);
    }
    let mut bad = scenario.clone();
    bad["map"] = json!([{"label": "hill", "object": 40, "class": "wall"}]);
    let r = post(&h, "/v1/rules/run", json!({"source": rules::builtin::SLAYER, "scenario": bad}), &[]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());

    let r = post(
        &h,
        "/v1/rules/run",
        json!({"source": BROKEN, "scenario": {}}),
        &[],
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["ok"], false);
    assert_eq!(r.json()["errors"][0]["line"], 4);

    for bad in [
        json!({"ticks": 36_001}),
        json!({"players": [{"slot": 16}]}),
        json!({"ticks": 5, "timeline": [{"tick": 6, "events": [{"type": "round_end"}]}]}),
        json!({"timeline": [{"tick": 1, "events": [{"type": "fly"}]}]}),
    ] {
        let r = post(
            &h,
            "/v1/rules/run",
            json!({"source": rules::builtin::SLAYER, "scenario": bad}),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.error());
    }
}

#[tokio::test]
async fn builtins_are_listed_with_their_source() {
    let h = harness().await;
    let r = h.get("/v1/rules/builtin").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let v = r.json();
    let items = v["items"].as_array().expect("items");
    let examples = rules::builtin::examples();
    assert_eq!(items.len(), rules::builtin::ALL.len() + examples.len());
    let slayer = items.iter().find(|i| i["name"] == "Slayer").expect("Slayer is listed");
    assert_eq!(slayer["engine"], 2);
    assert_eq!(slayer["source"], rules::builtin::SLAYER);
    assert_eq!(slayer["example"], false);
    // The ports first, then each example by key, with its carrier.
    let first_example = items.iter().position(|i| i["example"] == true).unwrap();
    assert_eq!(first_example, rules::builtin::ALL.len());
    for (key, source) in examples {
        let item = items.iter().find(|i| i["key"] == *key).expect("each example is listed");
        assert_eq!(item["source"], *source);
        let ir = rules::compile(source).unwrap();
        assert_eq!(item["engine"], ir.header.carrier);
        assert_eq!(item["name"], ir.header.name.as_str());
        assert_eq!(item["hash"], ir.id().to_string());
    }
}

const IDE_SRC: &str = "rules \"T\" 1\ncarrier slayer\noption points: int 1..5 = 1\non kill(victim, killer) {\n    score(killer, points)\n    \n}\n";

async fn ide(h: &Harness, body: Value) -> Value {
    let r = post(h, "/v1/rules/ide", body, &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    r.json()
}

#[tokio::test]
async fn ide_answers_each_op_in_lsp_shapes() {
    let h = harness().await;
    let src = IDE_SRC;

    let v = ide(&h, json!({"source": src, "op": "complete", "line": 5, "character": 4})).await;
    let items = v.as_array().expect("an array of items");
    let score = items.iter().find(|i| i["label"] == "score").expect("score");
    assert_eq!(score["kind"], 3);
    assert_eq!(score["insertTextFormat"], 2);
    assert_eq!(score["insertText"], "score(${1:player}, ${2:int})");
    assert_eq!(score["documentation"]["kind"], "markdown");
    assert!(score["detail"].as_str().unwrap().starts_with("score("));
    assert!(items.iter().any(|i| i["label"] == "victim"));

    let v = ide(&h, json!({"source": src, "op": "hover", "line": 4, "character": 6})).await;
    assert_eq!(v["contents"]["kind"], "markdown");
    assert!(v["contents"]["value"].as_str().unwrap().contains("score(player, int)"));
    assert_eq!(v["range"]["start"], json!({"line": 4, "character": 4}));
    assert_eq!(v["range"]["end"], json!({"line": 4, "character": 9}));
    let v = ide(&h, json!({"source": src, "op": "hover", "line": 5, "character": 0})).await;
    assert!(v.is_null());

    let v = ide(&h, json!({"source": src, "op": "signature", "line": 4, "character": 18})).await;
    assert_eq!(v["signatures"][0]["label"], "score(player, int)");
    assert_eq!(v["signatures"][0]["parameters"], json!([{"label": "player"}, {"label": "int"}]));
    assert_eq!(v["activeSignature"], 0);
    assert_eq!(v["activeParameter"], 1);

    let v = ide(&h, json!({"source": src, "op": "definition", "line": 4, "character": 20})).await;
    assert_eq!(
        v,
        json!([{"range": {"start": {"line": 2, "character": 7}, "end": {"line": 2, "character": 13}}}])
    );
    let v = ide(&h, json!({"source": src, "op": "references", "line": 4, "character": 11})).await;
    let lines: Vec<u64> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, [3, 4]);

    let v = ide(&h, json!({"source": src, "op": "tokens"})).await;
    assert_eq!(v["legend"], json!(rules::ide::LEGEND));
    assert_eq!(
        v["data"][0],
        json!({"line": 0, "start": 0, "length": 5, "type": 0, "typeName": "keyword"})
    );

    let v = ide(&h, json!({"source": src, "op": "symbols"})).await;
    let names: Vec<&str> = v.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["T", "points", "on kill"]);
    assert!(v[2].get("selectionRange").is_some());

    let messy = "rules \"T\" 1\ncarrier slayer\non kill(v,k){score(k,1)}\n";
    let v = ide(&h, json!({"source": messy, "op": "format"})).await;
    assert_eq!(
        v,
        json!({"text": "rules \"T\" 1\ncarrier slayer\non kill(v, k) {\n    score(k, 1)\n}\n"})
    );
    // Source that does not parse is not formatted.
    let v = ide(&h, json!({"source": "rules \"T\" 1\non tick {\n", "op": "format"})).await;
    assert!(v["error"].as_str().unwrap().starts_with("3:1:"), "{v}");
    assert_eq!(v["line"], 3);
    assert!(v.get("text").is_none());

    let v = ide(&h, json!({"source": BROKEN, "op": "diagnostics"})).await;
    assert_eq!(v[0]["severity"], 1);
    assert_eq!(v[0]["range"]["start"]["line"], 3);

    // `killer` may be none: a quick fix at the use, as a code action.
    let v = ide(&h, json!({"source": src, "op": "actions", "line": 4, "character": 12})).await;
    let actions = v.as_array().expect("an array of actions");
    assert!(!actions.is_empty(), "{v}");
    assert!(actions.iter().all(|a| a["kind"] == "quickfix" && a["diagnostic"]["code"] == "null.arg"), "{v}");
    assert!(actions[0]["edits"][0].get("newText").is_some(), "{v}");
    let v = ide(
        &h,
        json!({"source": src, "op": "actions", "line": 0, "character": 0, "end_line": 0, "end_character": 3}),
    )
    .await;
    assert_eq!(v, json!([]));

    // Renaming the option edits its declaration and its use.
    let v = ide(&h, json!({"source": src, "op": "rename", "line": 2, "character": 8, "new_name": "pts"})).await;
    let lines: Vec<u64> = v["edits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            assert_eq!(e["newText"], "pts");
            e["range"]["start"]["line"].as_u64().unwrap()
        })
        .collect();
    assert_eq!(lines, [2, 4]);

    // The place is required where an op reads one; an unknown op and an
    // unknown field are refused. A fresh server: the calls above spent
    // most of the route's rate.
    let h = harness().await;
    for bad in [
        json!({"source": src, "op": "hover"}),
        json!({"source": src, "op": "rename", "line": 2, "character": 8}),
        json!({"source": src, "op": "rename", "line": 2, "character": 8, "new_name": "on"}),
        json!({"source": src, "op": "actions", "line": 0, "character": 0, "end_line": 1}),
        json!({"source": src, "op": "inlay", "line": 0, "character": 0}),
        json!({"source": src, "op": "tokens", "extra": 1}),
        json!({"op": "tokens"}),
    ] {
        let r = post(&h, "/v1/rules/ide", bad.clone(), &[]).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.error());
    }
    let big = "x".repeat(live_proto::rules::MAX_SOURCE + 1);
    let r = post(&h, "/v1/rules/ide", json!({"source": big, "op": "tokens"}), &[]).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);

    let r = h.call(Method::GET, "/v1/rules/ide/legend", &[], "").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), json!({"legend": rules::ide::LEGEND}));
}

#[tokio::test]
async fn ide_requests_are_rate_limited_apart_from_checks() {
    let h = harness().await;
    let mut limited = false;
    for _ in 0..(live_server::rule_scripts::RATE.1 + 5) {
        let r = post(&h, "/v1/rules/ide", json!({"source": "x", "op": "tokens"}), &[]).await;
        if r.status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
        assert_eq!(r.status, StatusCode::OK);
    }
    assert!(limited, "the burst runs out");
    // A check still has its own bucket.
    let r = post(&h, "/v1/rules/check", json!({"source": "x"}), &[]).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn checks_are_rate_limited_per_client() {
    let h = harness().await;
    let mut limited = false;
    for _ in 0..(live_server::rule_scripts::RATE.1 + 5) {
        let r = post(&h, "/v1/rules/check", json!({"source": "x"}), &[]).await;
        if r.status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
        assert_eq!(r.status, StatusCode::OK);
    }
    assert!(limited, "the burst runs out");
}

async fn save(h: &Harness, who: Option<&User>, source: &str) -> Resp {
    let headers: Vec<_> = who
        .map(|u| h.bearer(u, lxm::CONTENT_WRITE))
        .into_iter()
        .collect();
    post(h, "/v1/rules", json!({"source": source}), &headers).await
}

async fn flow(timers: Timers) {
    let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
    let h = start_with(store.clone(), live_server::Policy::default(), timers).await;
    let me = h.create("Scripter").await;
    let other = h.create("Other").await;
    let hash = rules::compile(rules::builtin::SLAYER).unwrap().id().to_string();

    assert_eq!(
        save(&h, None, rules::builtin::SLAYER).await.status,
        StatusCode::UNAUTHORIZED
    );
    let r = save(&h, Some(&me), BROKEN).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());

    let r = save(&h, Some(&me), rules::builtin::SLAYER).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let v = r.json();
    assert_eq!(v["saved"], true);
    assert_eq!(v["script"]["hash"], hash);
    assert_eq!(v["script"]["gamertag"], "Scripter");
    // The same hash again, by anyone, saves nothing.
    let r = save(&h, Some(&other), rules::builtin::SLAYER).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    assert_eq!(r.json()["saved"], false);
    assert_eq!(r.json()["script"]["gamertag"], "Scripter");

    // Nobody posts the event by hand.
    let forged = h.event(
        &me,
        &live_proto::rules::stream(&"ab".repeat(32)),
        live_proto::rules::RULES_SAVED,
        json!({"hash": "ab".repeat(32), "name": "X", "version": 1, "author": me.xuid, "source": ""}),
    );
    assert_eq!(h.post_event(&me, forged).await.status, StatusCode::FORBIDDEN);

    for h in [&h, &start_with(store.clone(), live_server::Policy::default(), timers).await] {
        let r = h.get(&format!("/v1/rules/{hash}")).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        assert_eq!(r.json()["source"], rules::builtin::SLAYER);
        assert_eq!(r.json()["name"], "Slayer");

        let r = h.get(&format!("/v1/rules/{hash}/ir")).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(
            r.body,
            rules::compile(rules::builtin::SLAYER).unwrap().encode()
        );

        for author in ["Scripter".to_string(), me.xuid.to_string()] {
            let r = h.get(&format!("/v1/rules?author={author}")).await;
            assert_eq!(r.status, StatusCode::OK, "{}", r.error());
            assert_eq!(r.json()["items"][0]["hash"], hash);
        }
        let r = h.get("/v1/rules?author=Other").await;
        assert_eq!(r.json()["items"], json!([]));
        assert_eq!(h.get("/v1/rules?author=Nobody").await.status, StatusCode::NOT_FOUND);
        assert_eq!(h.get("/v1/rules").await.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            h.get(&format!("/v1/rules/{}", "cd".repeat(32))).await.status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(h.get("/v1/rules/nope").await.status, StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn saving_with_the_servers_own_timers() {
    flow(Timers::Spawned).await;
}

#[tokio::test]
async fn saving_with_no_timers() {
    flow(Timers::Driven).await;
}

#[tokio::test]
async fn simulate_plays_a_generated_match_and_sums_up_a_batch() {
    let h = harness().await;
    let body = json!({"source": rules::builtin::SLAYER, "players": 8, "teams": 2, "seed": 5, "matches": 6});
    let r = post(&h, "/v1/rules/simulate", body.clone(), &[]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let v = r.json();
    assert_eq!(v["ok"], true);
    assert_eq!(v["teams"], 2);
    assert_eq!(v["summary"]["asked"], 6);
    assert_eq!(v["summary"]["ran"], 6);
    assert_eq!(v["summary"]["budget"], rules::vm::TICK_BUDGET);
    assert!(v["summary"]["worst_tick_ops"].as_u64().unwrap() > 0);
    let wins: u64 = v["summary"]["wins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["matches"].as_u64().unwrap())
        .sum();
    assert_eq!(wins, 6);
    assert_eq!(v["summary"]["wins"][0]["winner"]["kind"], "team");
    let seeds: Vec<u64> = v["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["seed"].as_u64().unwrap())
        .collect();
    assert_eq!(seeds, [5, 6, 7, 8, 9, 10]);
    // The shown match is the first seed's, as a scenario and its trace.
    let m = &v["match"];
    assert_eq!(m["seed"], 5);
    assert_eq!(m["end"], v["rows"][0]["end"]);
    assert_eq!(m["winner"], v["rows"][0]["winner"]);
    assert_eq!(m["scenario"]["players"].as_array().unwrap().len(), 8);
    assert_eq!(m["scenario"]["players"][3]["unit"], 4096 + 3);
    assert_eq!(m["scenario"]["variant"]["team_play"], 1);
    assert_eq!(m["scenario"]["ticks"], m["trace"]["last_tick"]);
    let first = &m["scenario"]["timeline"][0]["events"][0];
    assert!(first["type"] == "kill" || first["type"] == "spawn", "{first}");
    assert!(m["trace"]["limit"] == "score" || m["trace"]["limit"] == "time");
    // The same request, the same answer.
    assert_eq!(post(&h, "/v1/rules/simulate", body, &[]).await.json(), v);

    // The match's scenario runs again through `run`, to the same scores.
    let again = post(
        &h,
        "/v1/rules/run",
        json!({"source": rules::builtin::SLAYER, "scenario": m["scenario"]}),
        &[],
    )
    .await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.error());
    assert_eq!(again.json()["scores"], m["trace"]["scores"]);
    assert_eq!(again.json()["limit"], m["trace"]["limit"]);

    // One match: no summary, the match alone.
    let v = post(
        &h,
        "/v1/rules/simulate",
        json!({"source": rules::builtin::SLAYER, "seed": 7}),
        &[],
    )
    .await
    .json();
    assert!(v["summary"].is_null());
    assert_eq!(v["rows"], json!([]));
    assert_eq!(v["match"]["seed"], 7);
    assert_eq!(v["teams"], 0);
}

#[tokio::test]
async fn simulate_refuses_a_bad_setup_and_reports_a_script_that_does_not_build() {
    let h = harness().await;
    let r = post(&h, "/v1/rules/simulate", json!({"source": BROKEN}), &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["ok"], false);
    assert_eq!(r.json()["errors"][0]["line"], 4);
    for bad in [
        json!({"players": 0}),
        json!({"players": 17}),
        json!({"teams": 9}),
        json!({"matches": 0}),
        json!({"matches": 101}),
        json!({"length": 0}),
        json!({"length": 1201}),
        json!({"score_limit": 0}),
        json!({"nope": 1}),
    ] {
        let mut body = bad.clone();
        body["source"] = json!(rules::builtin::SLAYER);
        let r = post(&h, "/v1/rules/simulate", body, &[]).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.error());
    }
    let big = "x".repeat(live_proto::rules::MAX_SOURCE + 1);
    let r = post(&h, "/v1/rules/simulate", json!({"source": big}), &[]).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.error());
}

#[tokio::test]
async fn simulate_shares_the_workbench_bucket_and_a_batch_costs_more() {
    let h = harness().await;
    let mut batches = 0;
    let mut limited = false;
    for _ in 0..(live_server::rule_scripts::RATE.1 + 5) {
        let r = post(
            &h,
            "/v1/rules/simulate",
            json!({"source": rules::builtin::SLAYER, "players": 2, "matches": 2, "length": 5}),
            &[],
        )
        .await;
        if r.status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        batches += 1;
    }
    assert!(limited, "the burst runs out");
    // A batch takes two of the bucket's 20.
    assert!(batches <= live_server::rule_scripts::RATE.1 / 2 + 1, "{batches}");
}
