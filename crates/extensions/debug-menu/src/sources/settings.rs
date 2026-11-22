//! Settings: every setting `settings.list {}` gives (no prefix), grouped by
//! what it is for into sub-pages (Video, Controls, Co-op, Skulls, ...).
//!
//! The first page of Settings lists the groups that have settings. Each
//! group is a page of its own that asks for the whole list and keeps the
//! keys whose prefix it owns; a key no group owns lands in Other, so a
//! setting added later is never missing from the menu.
//!
//! A setting that applies after a restart is greyed ("takes effect on
//! restart"), and so is one locked for this run by a flag or env var.

use serde_json::{Value, json};

use super::method::{SETTINGS_LIST, SETTINGS_SET};
use super::{get, humanize, list, problem, str_of};
use crate::model::{Answer, Call, Row, Section, Setter, text};

/// (id, title, key prefixes), in the order shown. A key goes to the first
/// group with a prefix of it.
const GROUPS: [(&str, &str, &[&str]); 8] = [
    ("video", "Video", &["graphics."]),
    (
        "controls",
        "Controls",
        &["controls.", "pad.", "pad_speaker.", "steam_pad.", "touch."],
    ),
    ("coop", "Co-op", &["coop."]),
    ("skulls", "Skulls", &["skulls."]),
    ("theater", "Theater", &["theater."]),
    ("superbounce", "Superbounce", &["superbounce."]),
    (
        "halo1",
        "Halo 1",
        &["halo1.", "campaign.halo1", "multiplayer.halo1"],
    ),
    (
        "modes",
        "Game modes",
        &["campaign.", "multiplayer.", "variants.", "live."],
    ),
];

/// The group a key belongs to, as an index into [`GROUPS`]; `GROUPS.len()`
/// is Other.
fn group_of(key: &str) -> usize {
    GROUPS
        .iter()
        .position(|(_, _, ps)| ps.iter().any(|p| key.starts_with(p)))
        .unwrap_or(GROUPS.len())
}

pub fn section() -> Section {
    Section {
        id: "settings".into(),
        title: "Settings".into(),
        about: "every setting, by what it is for".into(),
        calls: vec![Call::bare(SETTINGS_LIST)],
        rows: group_rows,
    }
}

pub fn pages() -> Vec<Section> {
    let mut out = vec![
        page::<0>(),
        page::<1>(),
        page::<2>(),
        page::<3>(),
        page::<4>(),
        page::<5>(),
        page::<6>(),
        page::<7>(),
    ];
    out.push(Section {
        id: "settings/other".into(),
        title: "Other".into(),
        about: "settings no group owns".into(),
        calls: vec![Call::bare(SETTINGS_LIST)],
        rows: settings_in::<8>,
    });
    out
}

fn page<const G: usize>() -> Section {
    let (id, title, _) = GROUPS[G];
    Section {
        id: format!("settings/{id}"),
        title: title.into(),
        about: format!("{title} settings"),
        calls: vec![Call::bare(SETTINGS_LIST)],
        rows: settings_in::<G>,
    }
}

/// The first page: a row for each group that has settings, saying how many.
fn group_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Settings", e)],
    };
    let mut counts = [0usize; GROUPS.len() + 1];
    for s in list(&v, "settings") {
        counts[group_of(&str_of(&s, "key"))] += 1;
    }
    let mut out: Vec<Row> = GROUPS
        .iter()
        .enumerate()
        .filter(|(i, _)| counts[*i] > 0)
        .map(|(i, (id, title, _))| {
            Row::open(*title, &format!("settings/{id}")).about(format!("{} settings", counts[i]))
        })
        .collect();
    if counts[GROUPS.len()] > 0 {
        out.push(Row::open("Other", "settings/other").about(format!(
            "{} settings",
            counts[GROUPS.len()]
        )));
    }
    if out.is_empty() {
        out.push(Row::info("No settings", ""));
    }
    out
}

/// A group's page: the settings it owns.
fn settings_in<const G: usize>(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Settings", e)],
    };
    let rows: Vec<Row> = list(&v, "settings")
        .iter()
        .filter(|s| group_of(&str_of(s, "key")) == G)
        .map(setting_row)
        .collect();
    if rows.is_empty() {
        return vec![Row::info("No settings", "")];
    }
    rows
}

/// A `settings.list` entry: `{key, label, kind, value, apply, locked?,
/// choices?, steps?, min?, max?, help}`.
pub fn setting_row(s: &Value) -> Row {
    let key = str_of(s, "key");
    let label = if s["label"].is_string() {
        str_of(s, "label")
    } else {
        humanize(key.rsplit('.').next().unwrap_or(&key))
    };
    let set = Setter::new(SETTINGS_SET, json!({ "key": key }), "value");
    let v = s["value"].clone();
    let row = match str_of(s, "kind").as_str() {
        "bool" => Row::toggle(label, v.as_bool().unwrap_or(false), set),
        "choice" => {
            let choices = list(s, "choices")
                .iter()
                .map(|c| (c["value"].clone(), str_of(c, "label")))
                .collect();
            Row::choice(label, v, choices, set)
        }
        "int" => {
            // The Video screen's steps (`{value, label}` or bare numbers),
            // as a choice.
            let steps: Vec<(Value, String)> = list(s, "steps")
                .iter()
                .map(|x| match x.get("value") {
                    Some(v) => (v.clone(), str_of(x, "label")),
                    None => (x.clone(), text(x)),
                })
                .collect();
            if steps.len() > 1 {
                Row::choice(label, v, steps, set)
            } else {
                let min = s["min"].as_i64().unwrap_or(i64::MIN);
                let max = s["max"].as_i64().unwrap_or(i64::MAX);
                Row::int(label, v.as_i64().unwrap_or(min), 1, (min, max), set)
            }
        }
        _ => Row::info(label, v),
    };
    let help = str_of(s, "help");
    let row = match s["apply"].as_str() {
        Some("next_map") => row.about(format!("{help} Applies at the next map.").trim().to_owned()),
        _ => row.about(help),
    };
    match (s["apply"].as_str(), s["locked"].as_str()) {
        (_, Some(by)) => row.disabled(format!("set by {by} for this run")),
        (Some("restart"), _) => row.disabled("takes effect on restart"),
        _ => match s["restart"].as_str() {
            Some(why) => row.disabled(format!("takes effect on restart: {why}")),
            None => row,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Game, page, row};
    use super::*;
    use crate::model::Edit;

    #[test]
    fn every_setting_lands_in_a_group_with_no_prefix_asked() {
        let mut g = Game::default();
        let rows = page(&mut g, "settings");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Video", "Controls", "Co-op", "Skulls", "Superbounce", "Halo 1", "Other"]
        );
        assert_eq!(row(&rows, "Video").edit, Edit::Open("settings/video".into()));
        assert_eq!(row(&rows, "Video").about, "2 settings");
        // The list was asked for whole.
        assert!(g.made("settings.list").iter().all(|c| c.params == json!({})));
    }

    #[test]
    fn video_lists_readable_rows_and_greys_restart_only_ones() {
        let mut g = Game::default();
        let rows = page(&mut g, "settings/video");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Field of view", "Resolution"]);
        let fov = row(&rows, "Field of view");
        assert_eq!(fov.shown(), "70");
        assert_eq!(fov.stepped(-1), Some(json!(60)));
        assert_eq!(fov.disabled, None);
        let res = row(&rows, "Resolution");
        assert_eq!(res.disabled.as_deref(), Some("takes effect on restart"));
        assert_eq!(
            fov.set.as_ref().unwrap().call(json!(60)).to_string(),
            r#"settings.set {"key":"graphics.fov","value":60}"#
        );
    }

    #[test]
    fn locked_and_next_map_settings_say_so() {
        let mut g = Game::default();
        let sb = page(&mut g, "settings/superbounce");
        assert_eq!(sb[0].disabled.as_deref(), Some("set by env for this run"));
        let coop = page(&mut g, "settings/coop");
        assert_eq!(coop[0].label, "Max players");
        assert!(coop[0].about.contains("next map"));
        assert!(coop[0].disabled.is_none());
        assert_eq!(page(&mut g, "settings/skulls")[0].label, "Envy");
    }

    #[test]
    fn a_key_no_group_owns_goes_to_other() {
        let mut g = Game::default();
        let rows = page(&mut g, "settings/other");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Brand new");
        assert_eq!(group_of("halo1.hud"), 6);
        assert_eq!(group_of("campaign.halo1_assets"), 6);
        assert_eq!(group_of("campaign.other"), 7);
        assert_eq!(group_of("zzz.q"), 8);
    }

    #[test]
    fn a_setting_without_a_label_is_named_from_its_key() {
        let r = setting_row(&json!({"key": "touch.auto_fire", "kind": "bool", "value": true, "help": "x"}));
        assert_eq!(r.label, "Auto fire");
    }
}
