//! The Game types dialog (docs/game/forge-map-variants-ui.md, "The
//! Properties page"): **All game types**, a row an engine and **Done**. A
//! on an engine ticks or unticks it, and the tick is the row's value
//! column ([`ON`], [`OFF`]); A on **All game types** ticks every engine, or
//! clears them when every engine is ticked. **Done** takes the set the
//! ticks name ([`done`]): the document's sentence for a set an entry cannot
//! hold ([`forge_doc::games::encode`]) is the Done row's description, the
//! dialog's status line, and the dialog stays; a set it can hold is sent
//! as [`super::sheet::Choice::Games`], which the session writes as one undo
//! step (`Session::set_games`).

use forge_doc::games::{self, ENGINES, GameSet};

use super::sheet::{Act, Kind, Sheet, SheetRow};

/// A ticked row's value.
pub const ON: &str = "On";
/// An unticked row's value.
pub const OFF: &str = "Off";

/// What a tick row stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    /// Every engine.
    All,
    /// One engine, by its index in the variant record ([`ENGINES`]).
    Engine(i32),
}

/// The row of **All game types**.
const ALL_ROW: usize = 0;

const ALL_HELP: &str = "Every game type. Press A again to clear them.";
const DONE_HELP: &str = "Keep these game types.";

fn state(on: bool) -> &'static str {
    if on { ON } else { OFF }
}

/// The dialog showing `set`: the engines' rows in [`ENGINES`]' order.
pub fn sheet(set: GameSet) -> Sheet {
    let mut rows = vec![
        SheetRow::new("All game types", Act::Tick(Tick::All))
            .value(state(set == GameSet::ALL))
            .describe(ALL_HELP),
    ];
    rows.extend(ENGINES.iter().map(|(index, name)| {
        SheetRow::new(name, Act::Tick(Tick::Engine(*index)))
            .value(state(set.has(*index)))
            .describe(&format!("Show it in {name}."))
    }));
    rows.push(SheetRow::new("Done", Act::Done).describe(DONE_HELP));
    Sheet {
        kind: Kind::Games,
        title: "GAME TYPES".into(),
        body: String::new(),
        rows,
        focus: 0,
    }
}

/// The set the ticks of `sheet` name.
pub fn set_of(sheet: &Sheet) -> GameSet {
    let engines: Vec<i32> = sheet
        .rows
        .iter()
        .filter_map(|r| match r.act {
            Act::Tick(Tick::Engine(i)) if r.value.as_deref() == Some(ON) => Some(i),
            _ => None,
        })
        .collect();
    GameSet::of(&engines)
}

/// Presses a tick row: an engine flips, **All game types** ticks every
/// engine unless they all are ticked, then clears them. The value of
/// **All game types** follows the engines, and a refusal on Done is taken
/// back.
pub fn tick(sheet: &mut Sheet, tick: Tick) {
    let before = set_of(sheet);
    let after = match tick {
        Tick::All if before == GameSet::ALL => GameSet::NONE,
        Tick::All => GameSet::ALL,
        Tick::Engine(i) => {
            let others: Vec<i32> = before.engines().into_iter().filter(|e| *e != i).collect();
            let flipped = if before.has(i) {
                others
            } else {
                [others, vec![i]].concat()
            };
            GameSet::of(&flipped)
        }
    };
    for r in &mut sheet.rows {
        match r.act {
            Act::Tick(Tick::Engine(i)) => r.value = Some(state(after.has(i)).into()),
            Act::Tick(Tick::All) => r.value = Some(state(after == GameSet::ALL).into()),
            Act::Done => r.description = Some(DONE_HELP.into()),
            _ => {}
        }
    }
    debug_assert_eq!(sheet.rows[ALL_ROW].act, Act::Tick(Tick::All));
}

/// Done: the set the ticks name when an entry can hold it. Otherwise the
/// document's sentence becomes the Done row's description and nothing is
/// chosen.
pub fn done(sheet: &mut Sheet) -> Result<GameSet, String> {
    let set = set_of(sheet);
    let verdict = games::encode(set).map(|_| set).map_err(|e| e.to_string());
    if let Some(r) = sheet.rows.iter_mut().find(|r| r.act == Act::Done) {
        r.description = Some(match &verdict {
            Ok(_) => DONE_HELP.into(),
            Err(why) => why.clone(),
        });
    }
    verdict
}

#[cfg(test)]
#[path = "games_tests.rs"]
mod tests;
