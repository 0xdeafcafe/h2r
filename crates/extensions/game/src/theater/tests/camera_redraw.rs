use super::*;

/// A viewer preview uses real replay ticks on its way to a present, but user
/// controls continue to see the pause tick while those ticks are undone.
#[test]
fn paused_camera_holds_its_tick_across_the_preview_and_the_return() {
    let dir = std::env::temp_dir().join(format!("ring-redraw-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("camera.h2film");
    let opts = options(&dir);
    let rec = Session::open(Some(path.clone()), None, host_facts(), b"xbe", &opts)
        .unwrap()
        .unwrap();
    let guest = arena();
    for _ in 0..4 {
        rec.checkpoint(&guest);
    }
    rec.finish(10, true).unwrap();
    let camera = FilmCamera::default();
    camera.attach_guest_view(4096);
    let replay = Session::open(
        None,
        Some(Replay {
            verification: None,
            path,
            render_every: 4,
            speed: 1.,
            seek_tick: 0,
            paused_at_target: false,
            presentation: Default::default(),
            restart: dir.join("restart"),
            camera: camera.clone(),
            window_closed: None,
        }),
        host_facts(),
        b"xbe",
        &opts,
    )
    .unwrap()
    .unwrap();

    replay.checkpoint(&guest);
    let cursor = replay.capture_replay_cursor().unwrap();
    {
        let mut s = replay.lock();
        s.paused = true;
        s.request_camera_redraw();
        assert!(
            !s.running_camera_redraw(),
            "the preview needs a return point"
        );
        s.ring_status.ticks.push(1);
        s.request_camera_redraw();
        assert!(s.redraw_boundary());
    }
    replay.checkpoint(&guest);
    assert!(camera.publish_guest_present(false, None));
    {
        let mut s = replay.lock();
        assert_eq!(s.tick, 2);
        assert_eq!(s.visible_tick(), 1);
        assert!(!s.redraw_boundary());
        assert_eq!(s.restart, Some(1));
        assert!(s.visible_pause());
        assert!(!camera.publish_guest_present(true, None));
    }

    replay.restore_replay_cursor(&cursor).unwrap();
    let mut s = replay.lock();
    s.restart = None;
    s.finish_camera_return();
    assert!(s.camera_redraw.is_none());
    assert_eq!(s.visible_tick(), 1);
    assert!(s.visible_pause());
    assert!(camera.publish_guest_present(true, None));
    drop(s);

    // Scaling is sampled after a present. Prime the requested grid, return
    // to the exact same tick, then deliver the cropped slot from its redraw.
    let normal = (640, 480);
    let requested = (1280, 960);
    let shot = replay
        .queue_guest_redraw(requested, 1, |size, rgba| (size, rgba))
        .unwrap();
    assert!(shot.try_recv().is_err());
    assert_eq!(camera.guest_raster(normal), requested);
    assert!(!camera.publish_guest_present(true, Some(normal)));
    {
        let mut s = replay.lock();
        assert!(!s.redraw_boundary());
        assert_eq!(s.restart.take(), Some(1));
        s.finish_camera_return();
        assert!(s.running_camera_redraw());
    }
    assert!(camera.publish_guest_present(false, Some(requested)));
    let crop = (2, 1);
    let rgba = vec![1, 2, 3, 255, 5, 6, 7, 255];
    camera.sample_guest_present(|| Ok((crop, rgba.clone())));
    assert_eq!(camera.guest_raster(normal), normal);
    assert!(
        shot.try_recv().is_err(),
        "return to the pause before answering"
    );
    {
        let mut s = replay.lock();
        assert!(!s.redraw_boundary());
        assert_eq!(s.restart.take(), Some(1));
        s.finish_camera_return();
    }
    assert_eq!(shot.try_recv().unwrap().unwrap(), (crop, rgba));

    let cancelled = replay
        .queue_guest_redraw(requested, 1, |_, bytes| bytes)
        .unwrap();
    replay.lock().cancel_camera_redraw();
    assert_eq!(camera.guest_raster(normal), normal);
    assert!(cancelled.try_recv().unwrap().is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
