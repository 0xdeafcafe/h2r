use super::*;
use crate::play::tests::paired;

#[test]
fn reload_waits_for_the_receipts_exact_activation_and_copy_checksum() {
    let (_temp, game, mut app, mut play) = paired("activation", "example", SessionKind::Local);
    // Keep socket traffic at the boundary while delivering the host observations.
    let _boundary = game.boundary.lock().unwrap();
    play.reload_for_play(&mut app, playtest::Reload::Method).unwrap();
    let copy = play.reload_copy.as_ref().unwrap();
    let (id, epoch, checksum) = (copy.id, copy.before_epoch, copy.checksum);
    let receipt = json!({
        "file": copy.file, "map": "example", "queued": true, "checksum": checksum,
        "activation": 3
    });
    play.reload_answer(&mut app, id, Ok(receipt));

    let mut status = serde_json::to_value(play.status.as_ref().unwrap()).unwrap();
    let identity = |epoch: u64, checksum: u32| {
        json!({"name":"example", "epoch":epoch, "checksum":checksum})
    };
    let copy = play.reload_copy.as_ref().unwrap();
    assert!(!copy.reload_complete(&app, &identity(epoch + 1, checksum), Some(4)));
    assert!(!copy.reload_complete(&app, &identity(epoch, checksum), None));
    assert!(copy.reload_complete(&app, &identity(epoch + 1, checksum), None));
    status["at"]["activation"] = json!(2);
    play.on_event(&mut app, link::Event::Poll(status.clone()));
    play.edit_map(&mut app, Ok(identity(epoch + 1, checksum)));
    assert!(play.reload_copy.is_some(), "an old activation matched");
    status["at"]["activation"] = json!(3);
    play.on_event(&mut app, link::Event::Poll(status));
    play.edit_map(&mut app, Ok(identity(epoch, !checksum)));
    assert!(play.reload_copy.is_some(), "a different copy matched");
    let mut later_map = identity(epoch, checksum);
    later_map["at"] = json!({"activation":4});
    play.edit_map(&mut app, Ok(later_map));
    assert!(play.reload_copy.is_some(), "the map changed since the status poll");

    play.edit_map(&mut app, Ok(identity(epoch, checksum)));
    assert!(play.reload_copy.is_none());
    assert_eq!(app.pairing.play_copy, Some((app.session.world_rev(), checksum)));
}
