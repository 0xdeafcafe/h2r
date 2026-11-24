//! The Playtest panel in the bottom dock (docs/editor/playtest.md, "What
//! is measured"; slice PT5): the result `playtest.result` answers, as
//! text, and a switch for each of its layers. [`rows`] makes the text
//! from the shown result alone, so tests read it without a window.
use dear_imgui_rs::{StyleColor, Ui};
use scenario_edit::layers::{LayerId, PlaytestLayer};
use sim_bench::spatial::{
    Measures,
    sight::{BUCKET, UNKNOWN_WEAPON},
};

use crate::app::playtest_shown::Shown;
use crate::app::{Action, App};
use crate::theme;
use crate::viewport::playtest::{count, refused};

/// How a row reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Dim,
    Warn,
}

/// One line of the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub tone: Tone,
    pub text: String,
}

fn row(tone: Tone, text: impl Into<String>) -> Row {
    Row {
        tone,
        text: text.into(),
    }
}

fn s1(v: Option<f32>) -> String {
    v.map_or("-".into(), |v| format!("{v:.1}"))
}

fn bar(n: u32, max: u32) -> String {
    let w = if max == 0 {
        0
    } else {
        (n * 20).div_ceil(max) as usize
    };
    "#".repeat(w)
}

/// The panel's lines for the result on show.
pub fn rows(shown: &Shown) -> Vec<Row> {
    if !shown.is_shown() {
        return vec![row(Tone::Dim, "No playtest yet: playtest.run starts one.")];
    }
    let mut o = vec![row(
        Tone::Plain,
        format!(
            "Run {} ({}): {}, {} seeds, build {}",
            shown.run,
            shown.state,
            if shown.game_type.is_empty() {
                "no game type"
            } else {
                &shown.game_type
            },
            shown.seeds,
            shown.build,
        ),
    )];
    if shown.stale {
        o.push(row(
            Tone::Warn,
            "Stale: the map was saved since. The layers keep their marks, dimmed.",
        ));
    }
    if shown.coarse {
        o.push(row(
            Tone::Warn,
            "Coarse: under eight seeds, so the spread is not worth trusting.",
        ));
    }
    if shown.partial {
        o.push(row(
            Tone::Dim,
            "Matches are still playing; the layers fill as they end.",
        ));
    }
    o.push(row(Tone::Dim, "These are the bots, not people."));
    let Some(m) = &shown.measures else {
        o.push(row(Tone::Dim, "No match has ended yet."));
        return o;
    };
    o.push(row(
        Tone::Plain,
        format!("{} matches, {:.0} game seconds", m.traces, m.seconds),
    ));
    measure_rows(m, &mut o);
    o
}

fn measure_rows(m: &Measures, o: &mut Vec<Row>) {
    let refusal = |what: Result<(), String>, o: &mut Vec<Row>| {
        if let Err(e) = what {
            o.push(row(Tone::Warn, e));
        }
    };
    refusal(m.heat.as_ref().map(|_| ()).map_err(ToString::to_string), o);
    refusal(
        m.spawns.as_ref().map(|_| ()).map_err(ToString::to_string),
        o,
    );
    refusal(m.flow.as_ref().map(|_| ()).map_err(ToString::to_string), o);
    refusal(m.sight.as_ref().map(|_| ()).map_err(ToString::to_string), o);
    refusal(m.items.as_ref().map(|_| ()).map_err(ToString::to_string), o);
    refusal(m.stuck.as_ref().map(|_| ()).map_err(ToString::to_string), o);
    if let Ok(h) = &m.heat {
        o.push(row(
            Tone::Plain,
            format!(
                "Kills {} places, deaths {} places",
                h.kills.len(),
                h.deaths.len()
            ),
        ));
    }
    if let Ok(s) = &m.spawns {
        o.push(row(
            Tone::Plain,
            format!(
                "Spawns ({} spawns): start, uses, life s, spawn deaths, seen",
                s.spawns
            ),
        ));
        for r in &s.starts {
            let name = r
                .start
                .map_or("off-start".to_owned(), |i| format!("start {i}"));
            let seen = r
                .seen_share
                .map_or("-".into(), |v| format!("{:.0}%", v * 100.0));
            o.push(row(
                if r.spawn_deaths > 0 {
                    Tone::Warn
                } else {
                    Tone::Plain
                },
                format!(
                    "  {name}: {}, {}, {}, {seen}",
                    r.uses,
                    s1(r.mean_life_s),
                    r.spawn_deaths
                ),
            ));
        }
        for (team, secs) in &s.opening {
            o.push(row(
                Tone::Plain,
                format!("  team {team} opening: first fight at {secs:.1} s"),
            ));
        }
    }
    if let Ok(s) = &m.sight {
        o.push(row(
            Tone::Plain,
            format!(
                "Fights: {} segments, median {:.1} units",
                s.segments, s.median
            ),
        ));
        for (weapon, buckets) in &s.by_weapon {
            let name = if *weapon == UNKNOWN_WEAPON {
                "unknown weapon".to_owned()
            } else {
                format!("weapon {weapon}")
            };
            let max = buckets.iter().copied().max().unwrap_or(0);
            o.push(row(Tone::Dim, format!("  {name}, per {BUCKET:.0} units:")));
            for (i, n) in buckets.iter().enumerate() {
                if *n > 0 {
                    o.push(row(
                        Tone::Dim,
                        format!("    {:>3.0}+ {:<20} {n}", i as f32 * BUCKET, bar(*n, max)),
                    ));
                }
            }
        }
    }
    if let Ok(i) = &m.items {
        let never = i.placements.iter().filter(|p| p.never).count();
        o.push(row(
            if never > 0 { Tone::Warn } else { Tone::Plain },
            format!(
                "Items: {} pickups, {} placements, {never} never picked",
                i.pickups,
                i.placements.len()
            ),
        ));
    }
    if let Ok(f) = &m.flow {
        let never = f.never.as_ref().map_or(0, Vec::len);
        o.push(row(
            if never > 0 { Tone::Warn } else { Tone::Plain },
            format!(
                "Flow: {} places visited, {never} polygons never entered",
                f.cells.len()
            ),
        ));
    }
    if let Ok(s) = &m.stuck {
        let islands = s.islands.as_ref().map_or(0, Vec::len);
        o.push(row(
            if !s.spots.is_empty() || islands > 0 {
                Tone::Warn
            } else {
                Tone::Plain
            },
            format!(
                "Stuck: {} spots, {islands} unreached islands",
                s.spots.len()
            ),
        ));
    }
}

/// Draw the panel.
pub fn draw(ui: &Ui, app: &App, actions: &mut Vec<Action>) {
    let mut layers = app.layers.clone();
    let mut changed = false;
    if ui.button("Show playtest##playtest-show") {
        layers.apply(scenario_edit::layers::Preset::Playtest);
        changed = true;
    }
    for layer in PlaytestLayer::ALL {
        ui.same_line();
        let id = LayerId::Playtest(layer);
        let mut on = layers.visible(id);
        let measures = app.playtest.measures.as_ref();
        let label = format!(
            "{} ({})##{}",
            layer.label(),
            measures.map_or(0, |m| count(layer, m)),
            id.id()
        );
        if ui.checkbox(label, &mut on) {
            layers.set(id, on);
            changed = true;
        }
        if let Some(why) = measures.and_then(|m| refused(layer, m)) {
            ui.set_item_tooltip(why);
        }
    }
    if changed {
        actions.push(Action::SetLayers(layers));
    }
    ui.separator();
    theme::panel_body(ui, "##playtest-rows", || {
        for r in rows(&app.playtest) {
            match r.tone {
                Tone::Plain => ui.text(&r.text),
                Tone::Dim => ui.text_disabled(&r.text),
                Tone::Warn => {
                    let _c = ui.push_style_color(StyleColor::Text, theme::WARN);
                    ui.text(&r.text);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused_all() -> Measures {
        use sim_bench::spatial::TooFew;
        let few = |measure| TooFew {
            measure,
            what: "events",
            have: 2,
            need: 10,
        };
        Measures {
            traces: 4,
            seconds: 480.0,
            heat: Err(few("heat")),
            spawns: Err(few("spawns")),
            flow: Err(few("flow")),
            sight: Err(few("sight")),
            items: Err(few("items")),
            stuck: Err(few("stuck")),
        }
    }

    /// Each refused measure is a warning row naming it; no measure, no
    /// summary row.
    #[test]
    fn a_refused_measure_is_a_warning_row_and_has_no_summary() {
        let mut o = Vec::new();
        measure_rows(&refused_all(), &mut o);
        assert_eq!(o.len(), 6);
        assert!(
            o.iter()
                .all(|r| r.tone == Tone::Warn && r.text.contains("too few"))
        );
        assert!(o[0].text.starts_with("heat"));
    }

    /// A measure that held shows its summary and flags what to look at:
    /// never-picked items, never-entered polygons and stuck spots.
    #[test]
    fn measures_that_held_are_summarised_and_trouble_is_flagged() {
        use sim_bench::spatial::{
            flow::Flow,
            heat::Heat,
            items::{Items, Placement},
            stuck::Stuck,
        };
        let mut m = refused_all();
        m.heat = Ok(Heat::default());
        m.flow = Ok(Flow {
            never: Some(vec![3, 4]),
            ..Flow::default()
        });
        m.items = Ok(Items {
            pickups: 5,
            loose: 0,
            placements: vec![Placement {
                item: 0,
                at: [0.0; 3],
                respawn_s: None,
                pickups: 0,
                per_minute: 0.0,
                first_pickup_s: None,
                kills: 0,
                never: true,
            }],
        });
        m.stuck = Ok(Stuck::default());
        let mut o = Vec::new();
        measure_rows(&m, &mut o);
        let find = |p: &str| {
            o.iter()
                .find(|r| r.text.starts_with(p))
                .unwrap_or_else(|| panic!("{p}: {o:?}"))
        };
        assert_eq!(find("Kills").tone, Tone::Plain);
        assert_eq!(find("Flow").tone, Tone::Warn);
        assert!(find("Flow").text.contains("2 polygons never entered"));
        assert!(find("Items").text.contains("1 never picked"));
        assert_eq!(find("Stuck").tone, Tone::Plain);
    }

    #[test]
    fn with_no_result_the_panel_says_how_to_start_one() {
        let rows = rows(&Shown::default());
        assert_eq!(rows.len(), 1);
        assert!(rows[0].text.contains("playtest.run"));
    }
}
