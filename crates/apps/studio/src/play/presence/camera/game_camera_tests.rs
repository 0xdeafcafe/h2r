//! M2 of docs/editor/live-mirror.md over the fake game: the `camera`
//! layer from a scene source, Game camera, frustums and a frame per step.
use super::*;
use crate::api::Api;
use crate::play::{
    testing::fake::until,
    tests::{paired, paired_with},
};
use crate::viewport::mirror::Scene;
use agent_proto::live::view_key;
use scenario_edit::geometry::{Mat4, cross};
use scenario_edit::layers::LayerId;
use std::time::{Duration, Instant};
use studio_proto::{code, method as m};

fn view(key: u32, position: Vec3, forward: Vec3, up: Vec3) -> View {
    View {
        key,
        scene_tick: 100,
        position,
        forward,
        up,
        fov: 1.2,
        near: 0.0625,
        far: 1024.0,
        viewport: [0, 0, 480, 640],
        cut: false,
        drawn: vec![crate::play::testing::fake::UNIT],
    }
}

/// The view matrix the native renderer draws a view with
/// (`frame_scene::RenderCamera::view`), written out independently.
fn scene_view(v: &View) -> Mat4 {
    let unit = |a: Vec3| {
        let l = dot(a, a).sqrt();
        [a[0] / l, a[1] / l, a[2] / l]
    };
    let f = unit(v.forward);
    let r = unit(cross(f, v.up));
    let u = cross(r, f);
    let p = v.position;
    [
        [r[0], u[0], -f[0], 0.0],
        [r[1], u[1], -f[1], 0.0],
        [r[2], u[2], -f[2], 0.0],
        [-dot(r, p), -dot(u, p), dot(f, p), 1.0],
    ]
}

fn assert_takes(cam: &Camera, v: &View) {
    let (got, want) = (cam.view(), scene_view(v));
    for (a, b) in got.iter().flatten().zip(want.iter().flatten()) {
        assert!((a - b).abs() < 1e-5, "{got:?} vs {want:?}");
    }
    assert_eq!(cam.eye(), v.position);
    assert_eq!(cam.fov_y, v.fov);
}

fn store_tick(app: &App) -> Option<u64> {
    app.pairing.mirror.as_ref()?.status(false)["tick"].as_u64()
}

#[test]
fn game_camera_takes_the_sealed_scene_view_exactly() {
    // A rolled game view whose forward is not unit length, and its
    // first-person view.
    let views = vec![
        view(
            view_key(0, 0, 0),
            [10.0, -4.0, 2.7],
            [0.0, 2.0, -0.5],
            [0.2, 0.0, 1.0],
        ),
        view(
            view_key(3, 0, 0),
            [10.0, -4.0, 2.6],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ),
    ];
    let scene = views.clone();
    let (_t, game, mut app, mut play) = paired_with(
        "game-camera",
        "example",
        sim::SessionKind::Local,
        move |g| g.scene(scene),
    );
    until(&mut play, &mut app, "the camera layer", |_, a| {
        a.pairing.shown.as_ref().is_some_and(|s| s.cameras == views)
    });
    let mut api = Api::new();
    let status = api.call(&mut app, m::MIRROR_STATUS, json!({})).unwrap();
    assert_eq!(
        status["stream"]["layers"],
        json!(["objects", "players", "camera"])
    );
    assert_eq!(status["stream"]["refused"], json!({}));

    // Free draws the game view's frustum, not the first-person one.
    app.layers.set(LayerId::Live, true);
    let fov = app.session.camera().fov_y;
    assert_eq!(Scene::shown(&app).frustums.len(), 1);
    let reply = api
        .call(&mut app, m::MIRROR_CAMERA, json!({"mode":"game"}))
        .unwrap();
    assert_eq!(
        reply,
        json!({"mode":"game","target":null,"view":"game:0:0"})
    );
    assert_takes(app.session.camera(), &views[0]);
    assert!(Scene::shown(&app).frustums.is_empty());

    // The game's camera moves and the viewport takes the next one.
    let moved = view(
        view_key(0, 0, 0),
        [12.0, -1.0, 3.0],
        [-1.0, 0.5, 0.1],
        [0.0, -0.1, 1.0],
    );
    game.edits.lock().unwrap().views[0] = moved.clone();
    until(&mut play, &mut app, "the moved game camera", |_, a| {
        a.session.camera().eye() == moved.position
    });
    assert_takes(app.session.camera(), &moved);

    let first_person = api
        .call(
            &mut app,
            m::MIRROR_CAMERA,
            json!({"mode":"game","view":"first_person:0:0"}),
        )
        .unwrap();
    assert_eq!(first_person["view"], "first_person:0:0");
    assert_takes(app.session.camera(), &views[1]);
    let missing = api
        .call(
            &mut app,
            m::MIRROR_CAMERA,
            json!({"mode":"game","view":"game:1:0"}),
        )
        .unwrap_err();
    assert_eq!(missing.code, code::REFUSED);
    assert!(missing.message.contains("game:1:0"), "{}", missing.message);
    let bad = api
        .call(
            &mut app,
            m::MIRROR_CAMERA,
            json!({"mode":"follow","view":"game:0:0"}),
        )
        .unwrap_err();
    assert_eq!(bad.code, code::INVALID_PARAMS);

    // Free keeps the eye where the game's was and gives back the field of view.
    let eye = app.session.camera().eye();
    let free = api
        .call(&mut app, m::MIRROR_CAMERA, json!({"mode":"free"}))
        .unwrap();
    assert_eq!(free, json!({"mode":"free","target":null}));
    let cam = app.session.camera();
    assert!(cam.exact.is_none());
    assert_eq!(cam.fov_y, fov);
    for (a, b) in cam.eye().iter().zip(eye) {
        assert!((a - b).abs() < 1e-4, "{:?} vs {eye:?}", cam.eye());
    }
}

#[test]
fn without_a_scene_source_the_camera_layer_is_refused() {
    let (_t, _game, mut app, mut play) = paired("no-camera", "example", sim::SessionKind::Local);
    until(&mut play, &mut app, "a game sample", |_, a| {
        a.pairing.shown.is_some()
    });
    let mut api = Api::new();
    let status = api.call(&mut app, m::MIRROR_STATUS, json!({})).unwrap();
    assert_eq!(status["stream"]["layers"], json!(["objects", "players"]));
    let why = status["stream"]["refused"]["camera"].as_str().unwrap();
    assert!(why.contains("no sealed frame scene"), "{why}");
    let refused = api
        .call(&mut app, m::MIRROR_CAMERA, json!({"mode":"game"}))
        .unwrap_err();
    assert_eq!(refused.code, code::REFUSED);
    assert!(
        refused.message.contains("the game has no camera"),
        "{}",
        refused.message
    );
    assert_eq!(
        api.call(&mut app, m::MIRROR_CAMERA, json!({})).unwrap(),
        json!({"mode":"free","target":null})
    );
    assert!(app.session.camera().exact.is_none());
    app.layers.set(LayerId::Live, true);
    assert!(Scene::shown(&app).frustums.is_empty());
}

#[test]
fn each_step_sends_one_frame_and_advances_the_store_tick_by_its_ticks() {
    let (_t, _game, mut app, mut play) = paired("mirror-step", "example", sim::SessionKind::Local);
    let mut api = Api::new();
    let paused_capture = |a: &App| {
        a.pairing
            .mirror
            .as_ref()
            .and_then(|m| m.snapshot())
            .and_then(|c| c.latest_header())
            .is_some_and(|(_, h)| h.paused)
    };
    let reply = api
        .call(&mut app, m::MIRROR_CONTROL, json!({"op":"pause"}))
        .unwrap();
    assert_eq!(reply["requested"], true);
    until(&mut play, &mut app, "a paused capture", |p, a| {
        p.paused && paused_capture(a)
    });
    let before = store_tick(&app).unwrap();
    let status = api.call(&mut app, m::MIRROR_STATUS, json!({})).unwrap();
    assert_eq!(status["store"]["tick"], before);

    let reply = api
        .call(&mut app, m::MIRROR_CONTROL, json!({"op":"step"}))
        .unwrap();
    assert_eq!(reply, json!({"requested":true, "tick":before}));
    until(&mut play, &mut app, "the step's frame", |_, a| {
        store_tick(a) == Some(before + 1)
    });
    // Paused again: no frame follows until the next step.
    let settle = Instant::now() + Duration::from_millis(200);
    while Instant::now() < settle {
        play.update(&mut app);
        std::thread::sleep(Duration::from_millis(5));
    }
    let status = api.call(&mut app, m::MIRROR_STATUS, json!({})).unwrap();
    assert_eq!(status["store"]["tick"], before + 1);

    api.call(&mut app, m::MIRROR_CONTROL, json!({"op":"step","ticks":2}))
        .unwrap();
    until(&mut play, &mut app, "the two-tick step's frames", |_, a| {
        store_tick(a) == Some(before + 3)
    });
    let bad = api
        .call(&mut app, m::MIRROR_CONTROL, json!({"op":"step","ticks":0}))
        .unwrap_err();
    assert_eq!(bad.code, code::INVALID_PARAMS);
    api.call(&mut app, m::MIRROR_CONTROL, json!({"op":"resume"}))
        .unwrap();
    until(&mut play, &mut app, "the resumed game", |p, a| {
        !p.paused && store_tick(a).is_some_and(|t| t > before + 3)
    });
}
