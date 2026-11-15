//! Teams through signed events, the router, mailbox projection and replay.
mod common;
use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use common::*;
use http_body_util::BodyExt;
use live_proto::{
    Xuid,
    team::{Action, Identity, MemberProperties, Properties, TeamError},
};
use live_server::{Live, Policy, ServerIdentity, store::MemoryStore};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;
fn create(id: Xuid, name: &str) -> Action {
    Action::Create {
        id,
        properties: Properties {
            name: name.into(),
            ..Default::default()
        },
        member: MemberProperties::default(),
        max_members: 100,
    }
}
async fn write(h: &Harness, user: &User, a: Action) -> Resp {
    h.post_event(
        user,
        h.event(
            user,
            &format!("team-{}", a.id()),
            a.kind(),
            serde_json::to_value(a).unwrap(),
        ),
    )
    .await
}
#[tokio::test]
async fn teams_signed_lifecycle_mailbox_permissions_and_read_endpoints() {
    let h = harness().await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;
    let id = Xuid(0x000a000000000001);
    assert_eq!(
        write(&h, &a, create(id, "Synthetic Clan")).await.status,
        StatusCode::CREATED
    );
    let list = h.get("/v1/accounts/Alpha/teams").await.json();
    assert_eq!(list["items"][0]["id"], id.to_string());
    assert_eq!(
        list["items"][0]["members"][a.xuid.to_string()]["properties"]["privileges"],
        u32::MAX
    );
    assert_eq!(
        write(
            &h,
            &a,
            Action::Recruit {
                id,
                target: Identity {
                    xuid: b.xuid,
                    gamertag: "Bravo".into()
                },
                properties: MemberProperties {
                    privileges: 0x10,
                    data: vec![1, 2]
                }
            }
        )
        .await
        .status,
        StatusCode::CREATED
    );
    let pending = h.get("/v1/accounts/Bravo/teams").await.json();
    assert!(pending["items"][0]["recruits"][b.xuid.to_string()].is_object());
    let log = h
        .get(&format!("/v1/events?stream=account-{}", b.xuid))
        .await
        .json();
    let mailbox = log["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == "TeamRecruitReceived")
        .unwrap();
    assert_eq!(mailbox["author"], "server:live.test");
    assert_eq!(mailbox["data"]["team_id"], id.to_string());
    assert_eq!(
        write(&h, &b, Action::Answer { id, accept: true })
            .await
            .status,
        StatusCode::CREATED
    );
    let detail = h.get(&format!("/v1/teams/{id}")).await.json();
    assert_eq!(detail["team"]["members"].as_object().unwrap().len(), 2);
    let forbidden = write(&h, &b, Action::Delete { id }).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    assert_eq!(forbidden.json()["team_error"], "Privileges");
    assert_eq!(
        write(
            &h,
            &a,
            Action::MemberProperties {
                id,
                user: b.xuid,
                properties: MemberProperties {
                    privileges: 0x1c,
                    data: vec![9]
                }
            }
        )
        .await
        .status,
        StatusCode::CREATED
    );
    let orphan = write(&h, &a, Action::Remove { id, user: a.xuid }).await;
    assert_eq!(orphan.json()["team_error"], "NoAdmin");
    assert_eq!(
        write(
            &h,
            &a,
            Action::Properties {
                id,
                properties: Properties {
                    name: "Edited".into(),
                    motto: "Updated".into(),
                    ..Default::default()
                }
            }
        )
        .await
        .status,
        StatusCode::CREATED
    );
    assert_eq!(
        h.get(&format!("/v1/teams/{id}")).await.json()["team"]["properties"]["motto"],
        "Updated"
    );
    assert_eq!(
        write(&h, &b, Action::Remove { id, user: b.xuid })
            .await
            .status,
        StatusCode::CREATED
    );
    assert_eq!(
        h.get("/v1/accounts/Bravo/teams").await.json()["items"],
        json!([])
    );
    assert_eq!(
        write(&h, &a, Action::Delete { id }).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        h.get(&format!("/v1/teams/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(h.get("/v1/teams").await.json()["items"], json!([]));
}
#[tokio::test]
async fn team_event_stream_actor_target_and_replay_are_checked_before_appending() {
    let h = harness().await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;
    let id = Xuid(0x000a000000000010);
    let wrong = h.event(
        &a,
        "team-000a000000000099",
        "TeamCreated",
        serde_json::to_value(create(id, "Clan")).unwrap(),
    );
    assert_eq!(
        h.post_event(&a, wrong).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        write(&h, &a, create(id, "Clan")).await.status,
        StatusCode::CREATED
    );
    let jws = h.event(
        &a,
        &format!("team-{id}"),
        "TeamPropertiesChanged",
        serde_json::to_value(Action::Properties {
            id,
            properties: Properties {
                name: "Clan".into(),
                ..Default::default()
            },
        })
        .unwrap(),
    );
    assert_eq!(
        h.post_event(&b, jws.clone()).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        h.post_event(&a, jws.clone()).await.status,
        StatusCode::CREATED
    );
    let position = h.live.position();
    assert_eq!(h.post_event(&a, jws).await.status, StatusCode::CONFLICT);
    assert_eq!(h.live.position(), position);
    let spoof = Action::Recruit {
        id,
        target: Identity {
            xuid: b.xuid,
            gamertag: "Forged".into(),
        },
        properties: MemberProperties::default(),
    };
    assert_eq!(write(&h, &a, spoof).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(h.live.position(), position);
    let escalation = Action::MemberProperties {
        id,
        user: a.xuid,
        properties: MemberProperties {
            privileges: 0,
            data: vec![],
        },
    };
    assert_eq!(
        write(&h, &a, escalation).await.json()["team_error"],
        serde_json::to_value(TeamError::NoAdmin).unwrap()
    );
}
#[tokio::test]
async fn teams_replay_and_concurrent_name_uniqueness_preserve_the_read_model() {
    let store = Arc::new(MemoryStore::new());
    let h = start(store.clone(), Policy::default()).await;
    let a = h.create("Alpha").await;
    let (one, two) = tokio::join!(
        write(&h, &a, create(Xuid(0x000a000000000001), "Same")),
        write(&h, &a, create(Xuid(0x000a000000000002), "same"))
    );
    assert!(
        (one.status == StatusCode::CREATED && two.status == StatusCode::CONFLICT)
            || (two.status == StatusCode::CREATED && one.status == StatusCode::CONFLICT)
    );
    let before = h.get("/v1/teams").await.json();
    let replay = Live::start(
        store,
        ServerIdentity::load(&h.dir).unwrap(),
        Policy::default(),
    )
    .await
    .unwrap();
    let response = live_server::http::router(replay)
        .oneshot(
            Request::builder()
                .uri("/v1/teams")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let after: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(after, before);
}
#[tokio::test]
async fn private_team_reads_need_a_read_token() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            private: true,
            ..Default::default()
        },
    )
    .await;
    let a = h.create("Alpha").await;
    let id = Xuid(0x000a000000000001);
    assert_eq!(
        write(&h, &a, create(id, "Clan")).await.status,
        StatusCode::CREATED
    );
    for path in [
        "/v1/teams".to_string(),
        format!("/v1/teams/{id}"),
        "/v1/accounts/Alpha/teams".into(),
    ] {
        assert_eq!(h.get(&path).await.status, StatusCode::UNAUTHORIZED);
        let token = format!(
            "Bearer {}",
            h.token(&a.key, &a.author, live_proto::lxm::READ)
        );
        assert_eq!(
            h.call(
                Method::GET,
                &path,
                &[("authorization", token)],
                Body::empty()
            )
            .await
            .status,
            StatusCode::OK
        );
    }
}
