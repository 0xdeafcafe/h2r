//! The film browser's filters, ordering and text, shared with its preview.

use super::{Entry, adapter::FilmSummary};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Sort {
    #[default]
    Recent,
    Map,
    GameType,
}

impl Sort {
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

    pub fn step_sort(&mut self, right: bool) {
        const ORDER: [Sort; 3] = [Sort::Recent, Sort::Map, Sort::GameType];
        self.sort = ORDER[(self.sort as usize + if right { 1 } else { 2 }) % ORDER.len()];
    }

    pub fn step_filter(&mut self, entries: &[Entry], filter: Filter, right: bool) {
        let mut values: Vec<_> = entries
            .iter()
            .filter_map(|entry| filter.value(&entry.film))
            .collect();
        values.sort_by_key(|value| value.to_lowercase());
        values.dedup_by(|a, b| a.eq_ignore_ascii_case(b));

        let active = match filter {
            Filter::Map => &mut self.map,
            Filter::GameType => &mut self.game_type,
        };
        let current = active
            .as_ref()
            .and_then(|name| {
                values
                    .iter()
                    .position(|value| value.eq_ignore_ascii_case(name))
            })
            .map_or(0, |index| index + 1);
        let count = values.len() + 1;
        let next = (current + if right { 1 } else { count - 1 }) % count;
        *active = next.checked_sub(1).map(|i| values[i].clone());
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
            Self::GameType => film.game_type().map(str::to_owned),
        }
    }
}

pub(super) fn map_stem(map: &str) -> String {
    let map = map
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(map)
        .to_ascii_lowercase();
    map.strip_suffix(".map").unwrap_or(&map).to_owned()
}

pub(super) fn map_title(map: &str) -> String {
    if map.trim().is_empty() {
        return "Unknown map".into();
    }
    live_hle::mods::base_title(&map_stem(map))
}

pub(super) fn fitted(text: &str, limit: usize) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= limit {
        clean
    } else {
        format!(
            "{}...",
            clean
                .chars()
                .take(limit.saturating_sub(3))
                .collect::<String>()
        )
    }
}

pub(super) fn clock(ticks: u64) -> String {
    let secs = ticks / 30;
    format!("{}:{:02}", secs / 60, secs % 60)
}

impl Entry {
    pub fn row_title(&self) -> String {
        let map = map_title(&self.film.map);
        let kind = self.film.game_type().unwrap_or("Unknown type");
        format!("{} / {}", fitted(&map, 15), fitted(kind, 12))
    }

    pub fn date(&self) -> String {
        i64::try_from(self.film.started_ms)
            .ok()
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map_or_else(
                || "Unknown date".into(),
                |date| date.format("%d %b %y").to_string(),
            )
    }

    pub fn duration(&self) -> String {
        self.clip
            .as_ref()
            .map(|clip| clip.to_tick.saturating_sub(clip.from_tick))
            .or_else(|| self.film.duration())
            .map_or_else(|| "--:--".into(), clock)
    }

    pub fn result(&self, player: Option<&str>) -> String {
        self.film
            .result
            .clone()
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
            .unwrap_or_else(|| "--".into())
    }

    pub fn player_count(&self) -> String {
        let count = self.film.scoreboard().len();
        match count {
            0 => "Players unknown".into(),
            1 => "1 player".into(),
            _ => format!("{count} players"),
        }
    }

    pub fn browse_description(&self, player: Option<&str>) -> String {
        let film = &self.film;
        let variant = self
            .archive_clip
            .as_ref()
            .map(|(_, title)| title.as_str())
            .unwrap_or_else(|| film.variant_name());
        [
            fitted(film.mode().unwrap_or("Mode unknown"), 28),
            fitted(
                if variant.is_empty() {
                    "Variant unknown"
                } else {
                    variant
                },
                28,
            ),
            format!("{}   {}", self.date(), self.duration()),
            format!(
                "{}   {}",
                self.player_count(),
                fitted(&self.result(player), 9)
            ),
            film.source_badge().into(),
        ]
        .join("\n")
    }
}

#[cfg(test)]
mod tests;
