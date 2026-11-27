//! The drop ship in the director's script (docs/game/firefight.md, "The
//! drop ship"): a squad arrives by ship when the zone has a path and the
//! ship is free, and every other squad arrives on foot.

mod common;

use common::*;
use firefight::director::ShipSeen;
use firefight::{Arrival, Command, Outcome, ScriptDirector};

/// A ship, and a path for the first of `two_zones`.
fn ship() -> ShipSeen {
    ShipSeen {
        ready: true,
        paths: vec![true, false],
    }
}

fn is_ship(command: &Command) -> bool {
    matches!(
        command,
        Command::DropSquad { .. }
            | Command::ShipHover { .. }
            | Command::ShipUnload
            | Command::ShipLeave { .. }
    )
}

#[test]
fn a_squad_arrives_by_ship_when_the_zone_has_a_path() {
    // One squad of one, never killed: the match stays on its first wave.
    let mut director = ScriptDirector::new(small_def(1, true), settings(7)).unwrap();
    let mut world = World::new(1, two_zones());
    world.ship = ship();
    let run = run(&mut director, &mut world, 600, |_, _| {}, |_, _, _, _, _| {});
    let flown: Vec<_> = run
        .commands
        .iter()
        .filter(|(_, command)| is_ship(command))
        .cloned()
        .collect();
    // The wave starts on tick 2: the hover hold, then the squad by ship.
    // It unloads when the flight in is over (8 s, 240 ticks), leaves 2 s
    // (60 ticks) later, and the hold is off when the flight out is over.
    assert_eq!(
        flown,
        [
            (2, Command::ShipHover { on: true }),
            (
                2,
                Command::DropSquad {
                    slot: 0,
                    class: "elite".into(),
                    count: 1,
                    zone: 0,
                    weapon_tier: 0,
                    // The command carries the arrival the script resolved.
                    arrival: Arrival::Ship,
                }
            ),
            (242, Command::ShipUnload),
            (302, Command::ShipLeave { zone: 0 }),
            (542, Command::ShipHover { on: false }),
        ]
    );
    // The squad's placement is the ship's: no second one on foot.
    assert!(!run
        .commands
        .iter()
        .any(|(_, command)| matches!(command, Command::PlaceSquad { .. })));
}

#[test]
fn a_busy_ship_never_stalls_a_wave() {
    // The ship takes the first squad and is away for the rest of the match
    // (it is out for 600 ticks, the match is 30 long): the four other
    // waves place on foot on the golden's ticks and the match is won.
    let mut director = ScriptDirector::new(small_def(1, true), settings(7)).unwrap();
    let mut world = World::new(1, two_zones());
    world.ship = ship();
    let run = run(
        &mut director,
        &mut world,
        100,
        |tick, world| kill_at_age_three(tick, world),
        |_, _, _, _, _| {},
    );
    assert_eq!(run.outcome, Some(Outcome::Won));
    let by_ship: Vec<u32> = run
        .commands
        .iter()
        .filter_map(|(tick, c)| matches!(c, Command::DropSquad { .. }).then_some(*tick))
        .collect();
    let on_foot: Vec<u32> = run
        .commands
        .iter()
        .filter_map(|(tick, c)| matches!(c, Command::PlaceSquad { .. }).then_some(*tick))
        .collect();
    assert_eq!(by_ship, [2]);
    assert_eq!(on_foot, [8, 14, 20, 26]);
    // Never an unload or a second flight while the ship is out.
    assert!(!run.commands.iter().any(|(_, c)| matches!(c, Command::ShipUnload)));
}

#[test]
fn a_ship_that_is_back_takes_the_next_squad() {
    // Each squad dies at once, the waves are 400 ticks apart: the ship
    // (back after 542 - 2 ticks) is away for the second wave and back for
    // the third.
    let mut def = small_def(1, true);
    for wave in &mut def.rounds[0].waves {
        wave.gap_ticks = 300;
    }
    let mut director = ScriptDirector::new(def, settings(7)).unwrap();
    let mut world = World::new(1, two_zones());
    world.ship = ship();
    let run = run(
        &mut director,
        &mut world,
        3_000,
        |tick, world| kill_at_age_three(tick, world),
        |_, _, _, _, _| {},
    );
    let by_ship: Vec<u32> = run
        .commands
        .iter()
        .filter_map(|(tick, c)| matches!(c, Command::DropSquad { .. }).then_some(*tick))
        .collect();
    // Wave 0 at 300 (the warmup is the first wave's gap), the ship is back at
    // 300 + 540 = 840: waves at 603 and 906 are by foot then by ship.
    assert_eq!(by_ship.first(), Some(&300));
    assert!(by_ship.len() >= 2, "{by_ship:?}");
    assert!(by_ship.iter().all(|t| *t == 300 || *t >= 840), "{by_ship:?}");
}
