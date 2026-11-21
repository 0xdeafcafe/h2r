use super::*;
use crate::play::select;
use agent_proto::editor::method::MARKS;
use scenario_edit::{geometry::Ray, layers::LayerId};

#[test]
fn an_unlinked_preview_picks_and_marks_its_current_document_row() {
    let (_temp, game, mut app, mut play) = object_pair("preview-pick", SessionKind::Local, true);
    game.allow_appended_preview();
    game.presentation(true);
    play.call(agent_proto::method::METHODS, Value::Null, Purpose::Capabilities);
    enable(&mut play, &mut app);
    let pos = [200., 150., 100.];
    let row = app.session.add_entry_at(WEAPONS, None, pos).unwrap();
    app.layers.set(LayerId::Live, true);
    app.layers.set(LayerId::Structure, false);
    let ray = Ray {
        origin: [195., 150., 100.],
        dir: [1., 0., 0.],
    };
    until(&mut play, &mut app, "pick the acknowledged preview", |_, a| {
        select::viewport_pick(a, &ray).is_some_and(|hit| hit.placement == Some((WEAPONS, row)))
    });
    let hit = select::viewport_pick(&app, &ray).unwrap();
    let handle = format!("{:08x}", hit.object);
    select::apply(&mut app, hit, false);
    assert_eq!(app.session.selected(), Some((WEAPONS, row)));
    until(&mut play, &mut app, "mark the salted preview", |_, _| {
        game.calls(MARKS).iter().any(|call| {
            call["marks"].as_array().is_some_and(|marks| {
                marks.iter().any(|m| m["style"] == "selected" && m["object"] == handle)
            })
        })
    });

    // Removing an earlier row swap-moves the preview's document identity.
    app.session.remove_entry(WEAPONS, 0).unwrap();
    let moved = select::viewport_pick(&app, &ray).unwrap();
    assert_eq!(moved.placement, Some((WEAPONS, 0)));
    select::apply(&mut app, hit, false);
    assert_eq!(app.session.selected(), moved.placement);

    // A replacement with the same object index but another salt has no receipt.
    let sample = app.pairing.shown.as_mut().unwrap();
    let preview = sample.objects.iter().position(|o| o.id == hit.object).unwrap();
    sample.objects[preview].id ^= 1 << 16;
    assert!(select::viewport_pick(&app, &ray).unwrap().placement.is_none());
    let sample = app.pairing.shown.as_mut().unwrap();
    sample.objects[preview].id = hit.object;
    sample.epoch += 1;
    app.pairing.stream_map["epoch"] = json!(sample.epoch);
    assert!(select::viewport_pick(&app, &ray).unwrap().placement.is_none());
}
