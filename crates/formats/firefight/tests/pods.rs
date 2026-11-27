//! Squads that arrive by pod in the director's script (docs/game/firefight.md,
//! "Drop pods"): a pod class falls where a zone has a fall and a pod is
//! free, and every other case is on foot.

mod common;

use common::*;
use firefight::director::PodsSeen;
use firefight::{Arrival, Command, ScriptDirector};

fn pods(free: u8, falls: Vec<bool>) -> PodsSeen {
    PodsSeen { free, falls, classes: ["elite".to_string()].into() }
}

fn first_arrival(pods: PodsSeen) -> Command {
    let mut director = ScriptDirector::new(small_def(1, true), settings(7)).unwrap();
    let mut world = World::new(1, two_zones());
    world.pods = pods;
    let run = run(&mut director, &mut world, 60, |_, _| {}, |_, _, _, _, _| {});
    run.commands
        .into_iter()
        .map(|(_, c)| c)
        .find(|c| matches!(c, Command::DropSquad { .. } | Command::PlaceSquad { .. }))
        .expect("a squad is placed")
}

#[test]
fn a_pod_class_squad_is_dropped_by_pod_where_a_zone_has_a_fall() {
    let c = first_arrival(pods(2, vec![true, true]));
    assert!(matches!(c, Command::DropSquad { arrival: Arrival::Pod, .. }), "{c:?}");
}

#[test]
fn no_free_pod_places_the_squad_on_foot() {
    let c = first_arrival(pods(0, vec![true, true]));
    assert!(matches!(c, Command::PlaceSquad { .. }), "{c:?}");
}

#[test]
fn a_zone_without_a_fall_places_the_squad_on_foot() {
    let c = first_arrival(pods(2, vec![false, false]));
    assert!(matches!(c, Command::PlaceSquad { .. }), "{c:?}");
}
