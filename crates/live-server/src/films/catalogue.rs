//! A sealed engine recording can arrive before any usable match statistics.
//! Keep its selector entry out of the game ledger until a real GameStarted.
use super::*;
use live_proto::film::{Mode, StatsDiscrepancy, Summary};

pub(super) struct Standalone {
    id: String,
    host: String,
    pub(super) position: u64,
    started_at: String,
    players: Vec<Xuid>,
    pub(super) saved_by: BTreeSet<Xuid>,
    blob: String,
    ranges: WebRanges,
    summary: Summary,
    discrepancy: StatsDiscrepancy,
}

impl Standalone {
    pub(super) fn from_receipt(event: &RecordedEvent) -> Option<Self> {
        let d = &event.body.data;
        let started_at = d["catalogue_started_ms"]
            .as_u64()
            .and_then(|ms| {
                time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
            })
            .filter(|t| (0..=9999).contains(&t.year()))
            .map(live_proto::sign::format_time)
            .unwrap_or_else(|| event.body.at.clone());
        let mut summary = serde_json::from_value(d["summary"].clone()).ok()?;
        live_proto::film::privacy::public_summary(&mut summary);
        Some(Self {
            id: d["game"].as_str()?.to_owned(),
            host: d["catalogue_host"].as_str()?.to_owned(),
            position: event.position,
            started_at,
            players: serde_json::from_value(d["catalogue_players"].clone()).ok()?,
            saved_by: BTreeSet::new(),
            blob: d["blob"].as_str()?.to_owned(),
            ranges: serde_json::from_value(d["web_ranges"].clone()).ok()?,
            summary,
            discrepancy: serde_json::from_value(d["stats_discrepancy"].clone()).ok()?,
        })
    }

    fn visible(&self, viewer: Viewer) -> bool {
        if crate::sim_runs::is_sim_id(&self.id) {
            return viewer.is_admin();
        }
        // Without a GameStarted there is no co-op session visibility record.
        if matches!(self.summary.mode, Mode::CoopCampaign | Mode::Campaign) {
            return viewer.is_admin()
                || matches!(viewer, Viewer::Account(id) if self.players.contains(&id));
        }
        true
    }
}

impl Films {
    pub(crate) fn standalone_host_blob(&self, id: &str) -> Option<(&str, &str)> {
        let entry = self.standalone.get(id)?;
        Some((&entry.host, &entry.blob))
    }

    /// Join a late telemetry start to its earlier recording. Saved films use
    /// the real game's position from this point, like every other game.
    pub(crate) fn attach_standalone(&mut self, game: &mut Game) {
        let id = &game.started.game_id;
        if self
            .standalone
            .get(id)
            .is_none_or(|f| f.host != game.started.host)
        {
            return;
        }
        let entry = self.standalone.remove(id).expect("matching film");
        self.standalone_order.remove(&entry.position);
        for player in &entry.saved_by {
            if let Some(pins) = self.saved.get_mut(player)
                && let Some(pin) = pins.remove(&entry.position)
            {
                pins.insert(game.position, pin);
                game.recording.saved_by += 1;
            }
        }
        game.has_host_film = true;
        game.film_blob = Some(entry.blob);
        game.film_summary = Some(entry.summary);
        game.film_web_ranges = Some(entry.ranges);
        game.film_stats_discrepancy = Some(entry.discrepancy);
    }
}

#[derive(Clone, Copy)]
pub(super) enum Source<'a> {
    Game(&'a Game),
    Standalone(&'a Standalone),
}

impl<'a> Source<'a> {
    pub(super) fn position(self) -> u64 {
        match self {
            Self::Game(g) => g.position,
            Self::Standalone(s) => s.position,
        }
    }

    pub(super) fn players(self) -> &'a [Xuid] {
        match self {
            Self::Game(g) => &g.xuids,
            Self::Standalone(s) => &s.players,
        }
    }

    pub(super) fn visible(self, model: &Model, viewer: Viewer) -> bool {
        match self {
            Self::Game(g) => g.shows_to(viewer, |a, b| model.is_friends(a, b)),
            Self::Standalone(s) => s.visible(viewer),
        }
    }

    pub(super) fn ranges(self) -> Option<&'a WebRanges> {
        match self {
            Self::Game(g) => g.film_web_ranges.as_ref(),
            Self::Standalone(s) => Some(&s.ranges),
        }
    }

    pub(super) fn range_receipt(self) -> Option<(WebRanges, Option<String>, bool)> {
        let (blob, ended) = match self {
            Self::Game(g) => (g.film_blob.clone(), g.ended.is_some()),
            // A subsequent real start supplies authoritative visibility.
            // Keep this intermediate entry out of public shared caches.
            Self::Standalone(s) => (Some(s.blob.clone()), false),
        };
        Some((self.ranges()?.clone(), blob, ended))
    }

    pub(super) fn listing(self, player: Xuid) -> Film<'a> {
        let s = match self {
            Self::Game(g) => return Film::new(g, player),
            Self::Standalone(s) => s,
        };
        let rate = film_tick_rate(self.ranges());
        Film {
            game_id: &s.id,
            map: &s.summary.level,
            mode: match s.summary.mode {
                Mode::Campaign | Mode::CoopCampaign => "campaign",
                Mode::Unknown => "unknown",
                Mode::Custom | Mode::Matchmaking | Mode::SystemLink => "multiplayer",
            },
            variant: (!s.summary.variant_name.is_empty())
                .then_some(s.summary.variant_name.as_str()),
            kind: match s.summary.mode {
                Mode::Campaign | Mode::CoopCampaign => Some("campaign"),
                Mode::Matchmaking => Some("matchmaking"),
                Mode::Custom | Mode::SystemLink => Some("custom"),
                Mode::Unknown => None,
            },
            playlist: None,
            difficulty: None,
            online: None,
            // No game stats have been committed. The sealed summary supplies
            // the display roster and scores to both selectors.
            players: Vec::new(),
            scoreboard: &[],
            started_at: &s.started_at,
            start_tick: None,
            duration_s: Some(s.summary.duration_ticks / u64::from(rate)),
            ticks_per_second: rate,
            result: None,
            has_host_film: true,
            summary: Some(&s.summary),
            stats_discrepancy: Some(&s.discrepancy),
        }
    }
}

pub(super) fn recent<'a>(model: &'a Model, player: Xuid, viewer: Viewer) -> Vec<Film<'a>> {
    let eligible = |s: &Source<'_>| s.players().contains(&player) && s.visible(model, viewer);
    let games = model
        .games_newest()
        .filter(|g| g.started.game.sim.is_none())
        .map(Source::Game)
        .filter(eligible)
        .take(RECENT);
    let standalone = model
        .films
        .standalone_order
        .values()
        .rev()
        .filter(|id| !crate::sim_runs::is_sim_id(id))
        .filter_map(|id| model.films.standalone.get(id))
        .map(Source::Standalone)
        .filter(eligible)
        .take(RECENT);
    let mut rows: Vec<_> = games.chain(standalone).collect();
    rows.sort_unstable_by_key(|s| std::cmp::Reverse(s.position()));
    rows.into_iter()
        .take(RECENT)
        .map(|s| s.listing(player))
        .collect()
}
