//! The film browser's filters, ordering and text, shared with its preview.

use std::sync::Arc;

use super::{Entry, State, Theater, adapter::FilmSummary};
use crate::ui_kit::{picker::Pick, text_fit};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Sort {
    #[default]
    Recent,
    Map,
    GameType,
}

impl Sort {
    const ALL: [Self; 3] = [Self::Recent, Self::Map, Self::GameType];

    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "Newest",
            Self::Map => "Map",
            Self::GameType => "Game type",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Filter {
    Map,
    GameType,
}

#[derive(Default)]
pub(super) struct Selector {
    pub sort: Sort,
    pub map: Option<String>,
    pub game_type: Option<String>,
}

impl Selector {
    pub fn clear_filters(&mut self) {
        self.map = None;
        self.game_type = None;
    }

    pub fn apply(&self, entries: &mut Vec<Entry>) {
        entries.retain(|entry| {
            let matches = |filter: Filter, name: &Option<String>| {
                name.as_ref().is_none_or(|name| {
                    filter
                        .value(&entry.film)
                        .is_some_and(|value| value.eq_ignore_ascii_case(name))
                })
            };
            matches(Filter::Map, &self.map) && matches(Filter::GameType, &self.game_type)
        });
        // Compute the textual key once per film and keep newest first within it.
        entries.sort_by_cached_key(|entry| {
            let film = &entry.film;
            let primary = match self.sort {
                Sort::Recent => None,
                Sort::Map => Filter::Map.value(film),
                Sort::GameType => Filter::GameType.value(film),
            };
            (
                self.sort != Sort::Recent && primary.is_none(),
                primary.map(|s| s.to_lowercase()),
                std::cmp::Reverse(film.started_ms),
                film.id.clone(),
            )
        });
    }
}

impl Filter {
    fn value(self, film: &impl FilmSummary) -> Option<String> {
        match self {
            Self::Map => (!film.map().trim().is_empty()).then(|| map_title(film.map())),
            Self::GameType => film
                .game_type()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        }
    }
}

/// The three options at the top of the library list.
#[derive(Clone, Copy)]
pub(super) enum OptionRow {
    Sort,
    Map,
    GameType,
}

impl OptionRow {
    pub const ALL: [Self; 3] = [Self::Sort, Self::Map, Self::GameType];

    pub fn at(row: i16) -> Option<Self> {
        let index = usize::try_from(row).ok()?;
        Self::ALL.get(index).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Sort => "Sort",
            Self::Map => "Map",
            Self::GameType => "Game type",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Sort => "Choose the order of the films.",
            Self::Map => "Show films from one map.",
            Self::GameType => "Show films of one game type.",
        }
    }

    pub fn current(self, selector: &Selector) -> &str {
        match self {
            Self::Sort => selector.sort.label(),
            Self::Map => selector.map.as_deref().unwrap_or("All"),
            Self::GameType => selector.game_type.as_deref().unwrap_or("All"),
        }
    }

    fn choices(self, state: &State) -> (Vec<String>, usize) {
        let (filter, active) = match self {
            Self::Sort => {
                return (
                    Sort::ALL.iter().map(|s| s.label().to_owned()).collect(),
                    state.selector.sort as usize,
                );
            }
            Self::Map => (Filter::Map, &state.selector.map),
            Self::GameType => (Filter::GameType, &state.selector.game_type),
        };
        // Include every map/type in the tab, even when the other filter
        // excludes all its rows. Index zero always clears this filter.
        let mut values: Vec<_> = state
            .tab_entries()
            .iter()
            .filter_map(|entry| filter.value(&entry.film))
            .collect();
        values.sort_by_cached_key(|value| value.to_lowercase());
        values.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        let current = active.as_ref().and_then(|name| {
            values
                .iter()
                .position(|value| value.eq_ignore_ascii_case(name))
        });
        values.insert(0, "All".into());
        (values, current.map_or(0, |i| i + 1))
    }

    pub fn pick(self, theater: &Theater, state: &State) -> Pick {
        let (values, current) = self.choices(state);
        // Keep the offered values with the setter. A Live response may
        // extend the library while the player has the picker open.
        let labels = values
            .iter()
            .map(|v| text_fit::line(v, text_fit::ROW))
            .collect();
        let theater = theater.clone();
        let set = Arc::new(move |i: usize| {
            let Some(value) = values.get(i) else {
                return String::new();
            };
            let mut state = theater.state();
            match self {
                Self::Sort => state.selector.sort = Sort::ALL[i],
                Self::Map => state.selector.map = (i != 0).then(|| value.clone()),
                Self::GameType => state.selector.game_type = (i != 0).then(|| value.clone()),
            }
            state.changed();
            format!("{}: {value}", self.label())
        });
        Pick::new(self.label(), labels, current, self.description(), set)
    }
}

/// Halo 2 levels whose shown name is not their file's stem title-cased:
/// the campaign's chapters and two multiplayer maps.
const LEVEL_NAMES: [(&str, &str); 17] = [
    ("00a_introduction", "The Heretic"),
    ("01a_tutorial", "The Armory"),
    ("01b_spacestation", "Cairo Station"),
    ("03a_oldmombasa", "Outskirts"),
    ("03b_newmombasa", "Metropolis"),
    ("04a_gasgiant", "The Arbiter"),
    ("04b_floodlab", "The Oracle"),
    ("05a_deltaapproach", "Delta Halo"),
    ("05b_deltatowers", "Regret"),
    ("06a_sentinelwalls", "Sacred Icon"),
    ("06b_floodzone", "Quarantine Zone"),
    ("07a_highcharity", "Gravemind"),
    ("07b_forerunnership", "High Charity"),
    ("08a_deltacliffs", "Uprising"),
    ("08b_deltacontrol", "The Great Journey"),
    ("beavercreek", "Beaver Creek"),
    ("cyclotron", "Ivory Tower"),
];

/// The level's name as the game shows it: "Regret" for
/// `05b_deltatowers`, "Ivory Tower" for `cyclotron`.
pub(super) fn map_title(map: &str) -> String {
    if map.trim().is_empty() {
        return "Unknown map".into();
    }
    let stem = crate::ui_kit::level_art::stem(map);
    LEVEL_NAMES
        .iter()
        .find(|(level, _)| level.eq_ignore_ascii_case(&stem))
        .map_or_else(|| live_hle::mods::base_title(&stem), |(_, name)| (*name).to_owned())
}

pub(super) fn clock(ticks: u64, rate: u64) -> String {
    let secs = ticks / rate;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The library's details start at the map picture's left edge and end at
/// the panel's (`pages::LIST_STATUS`, 246 units), with the coarse
/// metric's margin.
pub(super) const DETAIL_WIDTH: i32 = 240;

impl Entry {
    pub fn row_title(&self) -> String {
        if let Some((_, title)) = &self.archive_clip
            && !title.trim().is_empty()
        {
            return text_fit::line(title, text_fit::ROW);
        }
        let map = map_title(&self.film.map);
        let title = match self.film.game_type().filter(|s| !s.trim().is_empty()) {
            Some(kind) => format!("{map} / {kind}"),
            None => map,
        };
        text_fit::line(&title, text_fit::ROW)
    }

    pub(super) fn duration(&self) -> Option<String> {
        self.clip
            .as_ref()
            .map(|clip| clip.to_tick.saturating_sub(clip.from_tick))
            .or_else(|| self.film.duration())
            .map(|ticks| clock(ticks, self.film.tick_rate()))
    }

    pub fn recorded_date(&self) -> String {
        i64::try_from(self.film.started_ms)
            .ok()
            .filter(|ms| *ms > 0)
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|d| d.format("%d %b %Y").to_string())
            .unwrap_or_default()
    }

    pub fn recorded_time(&self) -> String {
        let date = i64::try_from(self.film.started_ms)
            .ok()
            .filter(|ms| *ms > 0)
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|d| d.format("%d %b %y").to_string());
        [date, self.duration()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" / ")
    }

    pub fn result(&self, player: Option<&str>) -> Option<String> {
        self.film
            .result
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "--")
            .map(str::to_owned)
            .or_else(|| {
                let player = player?;
                let score = self
                    .film
                    .scoreboard()
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(player))?
                    .score?;
                Some(format!("{score} pts"))
            })
    }

    pub fn browse_description(&self, player: Option<&str>) -> String {
        let mut lines = Vec::new();
        if self.archive_clip.is_some() {
            lines.push(text_fit::line(&map_title(&self.film.map), DETAIL_WIDTH));
        }
        // A campaign film's mode and variant are both "Campaign": each
        // name is shown once.
        let mut named: Vec<&str> = Vec::new();
        for value in [self.film.mode(), Some(self.film.variant_name())]
            .into_iter()
            .flatten()
            .map(str::trim)
        {
            if !value.is_empty() && !named.iter().any(|n| n.eq_ignore_ascii_case(value)) {
                named.push(value);
                lines.push(text_fit::line(value, DETAIL_WIDTH));
            }
        }
        let count = self.film.players().len();
        let mut match_info = Vec::new();
        if count > 0 {
            match_info.push(format!(
                "{count} player{}",
                if count == 1 { "" } else { "s" }
            ));
        }
        match_info.extend(self.result(player));
        if !match_info.is_empty() {
            lines.push(text_fit::line(&match_info.join(" / "), DETAIL_WIDTH));
        }
        lines.push(self.film.source_badge().to_owned());
        if let Some(note) = self.film.monitor_note() {
            lines.push(text_fit::line(&note, DETAIL_WIDTH));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests;
