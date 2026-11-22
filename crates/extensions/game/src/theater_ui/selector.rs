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

pub(super) fn map_title(map: &str) -> String {
    if map.trim().is_empty() {
        return "Unknown map".into();
    }
    live_hle::mods::base_title(&crate::ui_kit::level_art::stem(map))
}

pub(super) fn clock(ticks: u64, rate: u64) -> String {
    let secs = ticks / rate;
    format!("{}:{:02}", secs / 60, secs % 60)
}

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

    fn duration(&self) -> Option<String> {
        self.clip
            .as_ref()
            .map(|clip| clip.to_tick.saturating_sub(clip.from_tick))
            .or_else(|| self.film.duration())
            .map(|ticks| clock(ticks, self.film.tick_rate()))
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
            lines.push(text_fit::line(&map_title(&self.film.map), 280));
        }
        for value in [self.film.mode(), Some(self.film.variant_name())]
            .into_iter()
            .flatten()
        {
            if !value.trim().is_empty() {
                lines.push(text_fit::line(value, 280));
            }
        }
        let count = self.film.scoreboard().len();
        let mut match_info = Vec::new();
        if count > 0 {
            match_info.push(format!(
                "{count} player{}",
                if count == 1 { "" } else { "s" }
            ));
        }
        match_info.extend(self.result(player));
        if !match_info.is_empty() {
            lines.push(text_fit::line(&match_info.join(" / "), 280));
        }
        lines.push(self.film.source_badge().to_owned());
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests;
