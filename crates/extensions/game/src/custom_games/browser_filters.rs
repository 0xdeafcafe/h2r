//! The Live browser's filter choices and their presentation. The retail
//! browser still owns discovery and joining. Its filter page is the shared
//! settings dialog (`crate::ui_kit::settings`) its doc declares,
//! `content/ui/screens/browser.filters.yaml`: a pick row per filter, bound
//! here by name ([`bindings`]), and X CLEAR FILTERS. It is in the
//! lobby-dialog window, leaving the higher dialogs window for each picker.

mod guest;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use live_hle::Hle;
use live_proto::hopper::variant::GameType;
use live_proto::session::{BrowserFilters, BrowserKind, CampaignTitle, MapCategory};

use crate::ui_kit::{
    ids::{self, Family},
    page::{ListPage, described},
    picker::Pick,
    screen::{KitScreen, KitTexts, Then},
    settings::Bindings,
    text_fit,
};

pub const SCREEN: &str = "browser.filters";
const FEATURE: &str = "browser-filters";
const FILE: &str = "live-browser-filters.toml";
/// The filter page's texts (its kit screen's slots).
const TEXTS: Family = ids::family("browser filters");
/// The retail browser's header and footer, which this module rewrites
/// (`guest::Presentation`).
const CHROME: Family = ids::family("browser chrome");
const RETAIL_TITLE: u32 = 0;
const RETAIL_FOOTER: u32 = 1;
const _: () = assert!(RETAIL_FOOTER < CHROME.len);

static CURRENT: Mutex<Option<Arc<Browser>>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn current() -> Option<Arc<Browser>> {
    lock(&CURRENT).clone()
}

/// The saved preferences currently applied to the Live browser.
pub fn active_filters() -> BrowserFilters {
    current().map_or_else(BrowserFilters::default, |browser| browser.filters())
}

/// Register with the value picker when Live browsing is available.
pub fn install(
    registry: &mut hooks::Registry,
    hle: Option<&Arc<Hle>>,
    data_root: &Path,
) -> anyhow::Result<()> {
    *lock(&CURRENT) = None;
    let Some(hle) = hle.filter(|h| h.config().enabled && h.config().server_url.is_some()) else {
        return Ok(());
    };
    let path = data_root.join(FILE);
    let filters = match load(&path) {
        Ok(filters) => filters,
        Err(error) => {
            eprintln!("custom games: {}: {error}", path.display());
            BrowserFilters::default()
        }
    };
    let browser = Arc::new(Browser::new(filters.clone(), Some(path), Some(hle)));
    let page = Arc::new(page(Arc::clone(&browser), Vec::new()));
    if registry.add(&KitTexts(Arc::clone(&page)))?
        && registry.add(&page.class())?
        && registry.add(&guest::Presentation::new(Arc::clone(&browser)))?
    {
        hle.set_browser_filters(filters);
        *lock(&CURRENT) = Some(browser);
    }
    Ok(())
}

fn load(path: &Path) -> anyhow::Result<BrowserFilters> {
    let input = match fs::read_to_string(path) {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BrowserFilters::default());
        }
        Err(error) => return Err(error.into()),
    };
    let filters: BrowserFilters = toml::from_str(&input)?;
    filters.validate().map_err(anyhow::Error::msg)?;
    Ok(filters)
}

fn save(path: &Path, filters: &BrowserFilters) -> anyhow::Result<()> {
    let contents = toml::to_string_pretty(filters)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temp, contents)?;
    fs::rename(&temp, path)?;
    Ok(())
}

struct Browser {
    filters: Mutex<BrowserFilters>,
    path: Option<PathBuf>,
    hle: Option<Weak<Hle>>,
}

impl Browser {
    fn new(filters: BrowserFilters, path: Option<PathBuf>, hle: Option<&Arc<Hle>>) -> Self {
        Self {
            filters: Mutex::new(filters),
            path,
            hle: hle.map(Arc::downgrade),
        }
    }

    fn filters(&self) -> BrowserFilters {
        lock(&self.filters).clone()
    }

    fn active_count(&self) -> usize {
        active_count(&lock(&self.filters))
    }

    fn hle(&self) -> Option<Arc<Hle>> {
        self.hle.as_ref()?.upgrade()
    }

    fn update(&self, change: impl FnOnce(&mut BrowserFilters)) {
        let mut active = lock(&self.filters);
        let mut filters = active.clone();
        change(&mut filters);
        if *active == filters {
            return;
        }
        if let Some(hle) = self.hle() {
            hle.set_browser_filters(filters.clone());
        }
        if let Some(path) = &self.path
            && let Err(error) = save(path, &filters)
        {
            eprintln!("custom games: could not save {}: {error}", path.display());
        }
        *active = filters;
    }

    fn clear(&self) {
        self.update(|filters| *filters = BrowserFilters::default());
    }
}

#[derive(Clone, Copy)]
enum FilterRow {
    Kind,
    Clankers,
    GameType,
    Variant,
    MapCategory,
    Space,
    Campaign,
    Difficulty,
}

struct Choice {
    label: String,
    filters: BrowserFilters,
}

/// Give each choice a snapshot with just its own field changed. The
/// picker holds this list until it closes even if Live offers new names.
fn choices<T, L: Into<String>>(
    current: &BrowserFilters,
    options: impl IntoIterator<Item = (L, T)>,
    set: impl Fn(&mut BrowserFilters, Option<T>),
) -> Vec<Choice> {
    std::iter::once(("Any".to_owned(), None))
        .chain(
            options
                .into_iter()
                .map(|(label, value)| (label.into(), Some(value))),
        )
        .map(|(label, value)| {
            let mut filters = current.clone();
            set(&mut filters, value);
            Choice { label, filters }
        })
        .collect()
}

impl FilterRow {
    const ALL: [Self; 8] = [
        Self::Kind,
        Self::Clankers,
        Self::GameType,
        Self::Variant,
        Self::MapCategory,
        Self::Space,
        Self::Campaign,
        Self::Difficulty,
    ];

    /// The binding its row names in the doc (`bind: browser.<filter>`).
    fn bind(self) -> &'static str {
        match self {
            Self::Kind => "browser.kind",
            Self::Clankers => "browser.clankers",
            Self::GameType => "browser.game_type",
            Self::Variant => "browser.variant",
            Self::MapCategory => "browser.map_category",
            Self::Space => "browser.space",
            Self::Campaign => "browser.campaign",
            Self::Difficulty => "browser.difficulty",
        }
    }

    /// Its name in the log (the page's label is its doc's).
    fn label(self) -> &'static str {
        match self {
            Self::Kind => "Type",
            Self::Clankers => "Clankers",
            Self::GameType => "Game type",
            Self::Variant => "Variant",
            Self::MapCategory => "Map category",
            Self::Space => "Player spaces",
            Self::Campaign => "Campaign",
            Self::Difficulty => "Difficulty",
        }
    }

    fn choices(self, filters: &BrowserFilters, variants: &[String]) -> Vec<Choice> {
        match self {
            Self::Kind => choices(
                filters,
                [
                    ("Multiplayer", BrowserKind::Multiplayer),
                    ("Campaign co-op", BrowserKind::Coop),
                ],
                |f, v| f.kind = v,
            ),
            Self::Clankers => choices(
                filters,
                [("With Clankers", true), ("Without Clankers", false)],
                |f, v| f.clankers = v,
            ),
            Self::GameType => choices(
                filters,
                [
                    ("Slayer", GameType::Slayer),
                    ("CTF", GameType::Ctf),
                    ("Oddball", GameType::Oddball),
                    ("King of the Hill", GameType::King),
                    ("Juggernaut", GameType::Juggernaut),
                    ("Territories", GameType::Territories),
                    ("Assault", GameType::Assault),
                ],
                |f, v| f.game_type = v.map(GameType::engine),
            ),
            Self::Variant => {
                let key = |name: &str| name.trim().to_lowercase();
                let mut names: BTreeMap<_, _> =
                    variants.iter().map(|name| (key(name), name)).collect();
                if let Some(name) = &filters.variant {
                    names.insert(key(name), name);
                }
                choices(
                    filters,
                    names.into_values().map(|name| (name.clone(), name.clone())),
                    |f, v| f.variant = v,
                )
            }
            Self::MapCategory => choices(
                filters,
                [
                    ("Halo 2", MapCategory::Halo2),
                    ("Map Pack", MapCategory::MapPack),
                    ("Mods", MapCategory::Mods),
                    ("Halo: CE", MapCategory::HaloCe),
                ],
                |f, v| f.map_category = v,
            ),
            Self::Space => choices(filters, [("Has space", true)], |f, v| {
                f.has_space = v.unwrap_or(false)
            }),
            Self::Campaign => choices(
                filters,
                [
                    ("Halo 2", CampaignTitle::Halo2),
                    ("Halo: CE", CampaignTitle::HaloCe),
                ],
                |f, v| f.campaign = v,
            ),
            Self::Difficulty => choices(
                filters,
                [("Easy", 0), ("Normal", 1), ("Heroic", 2), ("Legendary", 3)],
                |f, v| f.difficulty = v,
            ),
        }
    }

    fn apply(self, active: &mut BrowserFilters, choice: &BrowserFilters) {
        match self {
            Self::Kind => active.kind = choice.kind,
            Self::Clankers => active.clankers = choice.clankers,
            Self::GameType => active.game_type = choice.game_type,
            Self::Variant => active.variant.clone_from(&choice.variant),
            Self::MapCategory => active.map_category = choice.map_category,
            Self::Space => active.has_space = choice.has_space,
            Self::Campaign => active.campaign = choice.campaign,
            Self::Difficulty => active.difficulty = choice.difficulty,
        }
    }

    fn selected(self, filters: &BrowserFilters) -> bool {
        match self {
            Self::Kind => filters.kind.is_some(),
            Self::Clankers => filters.clankers.is_some(),
            Self::GameType => filters.game_type.is_some(),
            Self::Variant => filters.variant.is_some(),
            Self::MapCategory => filters.map_category.is_some(),
            Self::Space => filters.has_space,
            Self::Campaign => filters.campaign.is_some(),
            Self::Difficulty => filters.difficulty.is_some(),
        }
    }
}

fn selected(choices: &[Choice], filters: &BrowserFilters) -> usize {
    choices
        .iter()
        .position(|c| c.filters == *filters)
        .unwrap_or(0)
}

fn active_count(filters: &BrowserFilters) -> usize {
    FilterRow::ALL
        .into_iter()
        .filter(|row| row.selected(filters))
        .count()
}

/// What a filter row shows beside its label: the choice in force.
fn shown(browser: &Browser, row: FilterRow) -> String {
    let filters = browser.filters();
    let choices = row.choices(&filters, &[]);
    text_fit::line(&choices[selected(&choices, &filters)].label, text_fit::ROW)
}

/// The value picker's pick for `row`: "Any" then the filter's choices,
/// the one in force focused. Its title and text are the row's, set by the
/// kit from the doc. The variants are Live's open games' (or `samples`
/// with no Live).
fn pick(browser: &Arc<Browser>, samples: &[String], row: FilterRow) -> Pick {
    let filters = browser.filters();
    let variants = browser
        .hle()
        .map_or_else(|| samples.to_vec(), |hle| hle.browser_status().variants);
    let choices = row.choices(&filters, &variants);
    let current = selected(&choices, &filters);
    let labels = choices
        .iter()
        .map(|c| text_fit::line(&c.label, text_fit::ROW))
        .collect();
    let held = Arc::clone(browser);
    let set = Arc::new(move |i: usize| {
        let Some(choice) = choices.get(i) else {
            return String::new();
        };
        held.update(|filters| row.apply(filters, &choice.filters));
        format!("{}: {}", row.label(), choice.label)
    });
    Pick::new(row.label(), labels, current, "", set)
}

/// The page's bindings: a pick per filter row, and X's clear.
fn bindings(browser: &Arc<Browser>, samples: Vec<String>) -> Bindings {
    let samples = Arc::new(samples);
    let mut b = Bindings::new();
    for row in FilterRow::ALL {
        let (on, of, list) = (Arc::clone(browser), Arc::clone(browser), Arc::clone(&samples));
        b = b.pick(row.bind(), move || shown(&on, row), move || Ok(pick(&of, &list, row)));
    }
    let clear = Arc::clone(browser);
    b.action("browser.clear", move || {
        clear.clear();
        Then::Stay("browser filters: cleared".into())
    })
}

/// The filter page over `browser`, the shared settings dialog its doc
/// declares. `samples` are the variants offered with no Live.
fn page(browser: Arc<Browser>, samples: Vec<String>) -> KitScreen {
    crate::ui_kit::settings::screen(FEATURE, SCREEN, TEXTS, None, bindings(&browser, samples))
        .unwrap_or_else(|e| panic!("{e}"))
}

pub(crate) fn sample_page() -> ListPage {
    let s = Arc::new(page(
        Arc::new(Browser::new(BrowserFilters::default(), None, None)),
        vec!["Team Slayer".into(), "One Flag".into()],
    ));
    ListPage::new(s.class(), described(&s))
}

pub(crate) use guest::handle_event;

#[cfg(test)]
mod tests;
