//! What the campaign picker says about the focused campaign
//! (docs/halo1/campaign-picker.md "The picker's detail pane"): a line
//! about it, the plain facts (release year and studio, its levels), the
//! player's progress, and for Halo 1 the Halo 1 / Halo 2 options the
//! level will start with. Pure: [`Info`] from the built campaigns, the
//! progress file and the settings, and [`Info::text`] the description pane
//! shows. The descriptions are our own lines, not the game's text.

use content::campaign_list::Built;

use crate::campaigns::{CampaignProgress, Progress, RETAIL_CAMPAIGN};
use crate::ui_kit::text::join_sentences;

/// Halo 2's own line for the pane (it has no manifest).
pub const HALO2_DESCRIPTION: &str = "The war comes home to Earth, and a second ring waits.";

/// The difficulties, easy to legendary, as the profile's progress counts
/// them (`campaigns::CampaignProgress::finished`).
pub const DIFFICULTIES: [&str; 4] = ["Easy", "Normal", "Heroic", "Legendary"];

/// Release year and studio, as plain facts, for the campaigns we know: Halo
/// 2 (no key) and Halo 1 (`halo1`). Others say nothing.
pub fn facts(key: Option<&str>) -> Option<(u16, &'static str)> {
    match key {
        None => Some((2004, "Bungie")),
        Some("halo1") => Some((2001, "Bungie")),
        Some(_) => None,
    }
}

/// How far a player is: levels the level select shows (finished, and the
/// next), of how many, and the hardest difficulty the last level was
/// finished on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reached {
    pub reached: usize,
    pub total: usize,
    /// Index into [`DIFFICULTIES`].
    pub beaten: Option<usize>,
}

/// [`Reached`] for a campaign whose levels have `map_ids` (rising, play
/// order) and whose players finished up to `finished` (highest map id per
/// difficulty, `-1` none): the levels the level select lists, as its
/// constructor `0x2c4ea0` does (every level up to the first above the
/// progress), and the campaign beaten on a difficulty when its last level
/// was finished on it.
pub fn reached(map_ids: &[u32], finished: [i64; 4]) -> Reached {
    let best = finished.iter().copied().max().unwrap_or(-1);
    let done = map_ids.iter().filter(|&&id| i64::from(id) <= best).count();
    let beaten = map_ids
        .last()
        .and_then(|&last| (0..4).rev().find(|&d| finished[d] >= i64::from(last)));
    Reached {
        reached: (done + 1).min(map_ids.len()),
        total: map_ids.len(),
        beaten,
    }
}

/// Everything the pane says about one campaign.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub name: String,
    pub description: String,
    pub facts: Option<(u16, &'static str)>,
    /// Its level count.
    pub levels: Option<usize>,
    /// Ours only: Halo 2's progress lives in the profile.
    pub progress: Option<Reached>,
    /// Halo 1's options as `(label, on Halo 1's choice)`.
    pub options: Vec<(&'static str, bool)>,
}

impl Info {
    /// Halo 2's.
    pub fn halo2() -> Self {
        Info {
            name: "Halo 2".into(),
            description: HALO2_DESCRIPTION.into(),
            facts: facts(None),
            levels: None,
            progress: None,
            options: Vec::new(),
        }
    }

    /// One of ours: `progress` is the signed-in players' best, merged
    /// per difficulty; `options` are given for Halo 1 only.
    pub fn ours(b: &Built, progress: &[CampaignProgress], options: Vec<(&'static str, bool)>) -> Self {
        let mut finished = [-1i64; 4];
        for p in progress {
            for d in 0..4 {
                finished[d] = finished[d].max(p.finished[d]);
            }
        }
        let ids: Vec<u32> = b.levels.iter().map(|l| l.map_id).collect();
        Info {
            name: b.names[0].clone(),
            description: b.descriptions[0].clone(),
            facts: facts(Some(&b.key)),
            levels: Some(b.levels.len()),
            progress: Some(reached(&ids, finished)),
            options: if b.key == crate::campaigns::HALO1_KEY {
                options
            } else {
                Vec::new()
            },
        }
    }

    /// The pane's text, wrapped by the guest widget.
    pub fn text(&self) -> String {
        let mut sentences = vec![self.description.clone()];
        if let Some((year, studio)) = self.facts {
            sentences.push(format!("{year}, {studio}"));
        }
        if let Some(n) = self.levels {
            let unit = if n == 1 { "level" } else { "levels" };
            sentences.push(format!("{n} {unit}"));
        }
        if let Some(p) = self.progress {
            sentences.push(match p.beaten {
                Some(d) => format!(
                    "Reached {} of {}, beaten on {}",
                    p.reached, p.total, DIFFICULTIES[d]
                ),
                None => format!("Reached {} of {}", p.reached, p.total),
            });
        }
        sentences.extend(
            self.options
                .iter()
                .map(|(label, h1)| format!("{label}: {}", if *h1 { "Halo 1" } else { "Halo 2" })),
        );
        join_sentences(sentences)
    }
}

/// The picker's rows' infos, in its order (`campaigns::Picker`): Halo 2's
/// for the retail campaign, ours from `built` with `profiles`' progress
/// and the settings' Halo 1 options.
pub fn infos(
    built: &[Built],
    rows: &[(u32, String)],
    progress: &Progress,
    profiles: &[String],
    options: &dyn Fn() -> Vec<(&'static str, bool)>,
) -> Vec<Info> {
    rows.iter()
        .map(|(id, _)| {
            if *id == RETAIL_CAMPAIGN {
                return Info::halo2();
            }
            match built.iter().find(|b| b.campaign_id == *id) {
                Some(b) => {
                    let p: Vec<CampaignProgress> =
                        profiles.iter().filter_map(|n| progress.get(n, &b.key)).collect();
                    Info::ours(b, &p, options())
                }
                None => Info::halo2(),
            }
        })
        .collect()
}

/// The Halo 1 options the campaign starts with, from `store`.
pub fn halo1_options(store: &settings::Store) -> Vec<(&'static str, bool)> {
    crate::h1_options::CAMPAIGN
        .iter()
        .map(|o| (o.label, crate::h1_options::is_halo1(store, o)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use content::campaign_list::{self as cl, CampaignManifest, FIXTURE, HALO1};

    fn built() -> Vec<Built> {
        let ms = [
            CampaignManifest::parse(HALO1).unwrap(),
            CampaignManifest::parse(FIXTURE).unwrap(),
        ];
        cl::build(&ms, &[1], &[0x65]).unwrap()
    }

    /// The level select's rule, per campaign: finished levels and the next
    /// one; beaten on the hardest difficulty the last level fell on.
    #[test]
    fn progress_counts_the_levels_the_level_select_lists() {
        let ids = [0x1001, 0x1002, 0x1003];
        for (finished, want) in [
            ([-1, -1, -1, -1], (1, None)),
            ([0x1001, -1, -1, -1], (2, None)),
            ([-1, -1, 0x1002, -1], (3, None)),
            ([0x1003, -1, -1, -1], (3, Some(0))),
            ([0x1003, 0x1003, 0x1003, 0x1001], (3, Some(2))),
            ([-1, -1, -1, 0x1003], (3, Some(3))),
        ] {
            let r = reached(&ids, finished);
            assert_eq!((r.reached, r.beaten), want, "{finished:x?}");
            assert_eq!(r.total, 3);
        }
    }

    #[test]
    fn the_pane_says_the_line_the_facts_the_progress_and_the_options() {
        let b = built();
        let fresh = Info::ours(&b[0], &[], vec![("Assets", true), ("HUD", false)]);
        assert_eq!(
            fresh.text(),
            "An ancient ring, and the secret it was built to keep. 2001, Bungie. \
             10 levels. Reached 1 of 10. Assets: Halo 1. HUD: Halo 2."
        );
        // Two players: the best of each difficulty.
        let a = CampaignProgress {
            finished: [0x1004, -1, -1, -1],
            last: 0x1005,
            ..Default::default()
        };
        let c = CampaignProgress {
            finished: [-1, -1, 0x100a, -1],
            last: 0x100a,
            ..Default::default()
        };
        let both = Info::ours(&b[0], &[a, c], Vec::new());
        assert_eq!(both.progress.unwrap().reached, 10);
        assert_eq!(
            both.text(),
            "An ancient ring, and the secret it was built to keep. 2001, Bungie. \
             10 levels. Reached 10 of 10, beaten on Heroic."
        );
        // The test campaign: no facts, no options even when given.
        let t = Info::ours(&b[1], &[], vec![("Assets", true)]);
        assert_eq!(t.facts, None);
        assert!(t.options.is_empty());
        assert_eq!(
            t.text(),
            "Retail Halo 2 levels, listed as a second campaign. 3 levels. Reached 1 of 3."
        );
        let single = Info {
            levels: Some(1),
            progress: None,
            ..t
        };
        assert_eq!(
            single.text(),
            "Retail Halo 2 levels, listed as a second campaign. 1 level."
        );
        // Halo 2: its line and facts; its progress is the profile's.
        assert_eq!(
            Info::halo2().text(),
            "The war comes home to Earth, and a second ring waits. 2004, Bungie."
        );
    }

    #[test]
    fn infos_follow_the_picker_s_rows_and_the_signed_in_profiles() {
        let b = built();
        let picker = crate::campaigns::Picker::new(&b);
        let progress = Progress::memory();
        progress.finished("Chief", "halo1", 1, 0x1002);
        progress.finished("Arbiter", "halo1", 3, 0x1004);
        let opts = || vec![("Assets", false), ("HUD", true)];
        let infos = infos(&b, &picker.rows, &progress, &["Chief".into()], &opts);
        let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Halo: Combat Evolved", "Halo 2", "Second Campaign (test)"]);
        assert_eq!(infos[0].progress.unwrap().reached, 3, "Chief's alone");
        assert_eq!(infos[0].options, opts());
        assert_eq!(infos[1], Info::halo2());
        let two = super::infos(&b, &picker.rows, &progress, &["Chief".into(), "Arbiter".into()], &opts);
        assert_eq!(two[0].progress.unwrap().reached, 5);
    }
}
