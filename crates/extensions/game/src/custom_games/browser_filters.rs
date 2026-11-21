//! The Live browser's filter choices and their presentation. The retail
//! browser still owns discovery and joining. Its filter page is in the
//! lobby-dialog window, leaving the higher dialogs window for each picker.

mod guest;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use engine::memory::{GuestMemory, MemFault};
use live_hle::Hle;
use live_proto::hopper::variant::GameType;
use live_proto::session::{BrowserFilters, BrowserKind, CampaignTitle, MapCategory};

use crate::list_class::{self, Button, ListClass, Rows};
use crate::ui::{Chosen, LABEL_SLOT, VALUE_SLOT};
use crate::ui_kit::{
    dialog::{DialogBase, DialogSpec},
    dialog_tag,
    ids::{self, Family},
    input::{Glyph, Input, Prompt, footer_legend},
    page::{ListPage, described},
    picker::{self, Pick},
    screen::SlotTexts,
    spec::{Describe, ListPlace, Press, RowSpec, ScreenSpec},
    text_fit,
    view::Opener,
};

pub const SCREEN: &str = "browser.filters";
const FEATURE: &str = "browser-filters";
const FILE: &str = "live-browser-filters.toml";
const TEXTS: Family = ids::family("browser filters");
const BASE: DialogBase = DialogBase::QuickOptions;
const PAGE_TITLE: u32 = 0;
const PAGE_FOOTER: u32 = 1;
const PAGE_ROWS: u32 = 2;
const RETAIL_TITLE: u32 = PAGE_ROWS + 3 * FilterRow::ALL.len() as u32;
const RETAIL_FOOTER: u32 = RETAIL_TITLE + 1;
const _: () = assert!(RETAIL_FOOTER < TEXTS.len);

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
    let page = Arc::new(FilterPage::new(Arc::clone(&browser), Vec::new()));
    if registry.add(&page.class())? && registry.add(&guest::Presentation(page))? {
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

    fn at(row: i16) -> Option<Self> {
        Self::ALL.get(usize::try_from(row).ok()?).copied()
    }

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

    fn help(self) -> &'static str {
        match self {
            Self::Kind => "Choose what to play.",
            Self::Clankers => "Choose games with or without bots.",
            Self::GameType => "Choose a multiplayer game type.",
            Self::Variant => "Choose a variant from open games.",
            Self::MapCategory => "Choose where maps come from.",
            Self::Space => "Find a game with a public seat.",
            Self::Campaign => "Choose a co-op campaign.",
            Self::Difficulty => "Choose a co-op difficulty.",
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

fn prompts() -> Vec<Prompt> {
    vec![
        Prompt::new(&[Glyph::A], "Select"),
        Prompt::new(&[Glyph::B], "Back"),
        Prompt::new(&[Glyph::X], "Clear filters"),
    ]
}

struct FilterPage {
    browser: Arc<Browser>,
    sample_variants: Vec<String>,
    texts: SlotTexts,
}

impl FilterPage {
    fn new(browser: Arc<Browser>, sample_variants: Vec<String>) -> Self {
        Self {
            browser,
            sample_variants,
            texts: SlotTexts::new(TEXTS),
        }
    }

    fn class(self: &Arc<Self>) -> ListClass {
        let browser = Arc::clone(&self.browser);
        let rows = Arc::clone(self) as Arc<dyn Rows>;
        ListClass::with_rows(FEATURE, SCREEN, rows)
            .dialog(BASE)
            .layout(dialog_tag::base_layout(BASE, FilterRow::ALL.len()))
            .description(BASE.help().expect("Quick Options has a help line"))
            .footer(self.texts.set(PAGE_FOOTER, &footer_legend(&prompts())))
            .buttons(Arc::new(move |kind, button| {
                if Input::from_event(kind, button) != Some(Input::X) {
                    return Button::Pass;
                }
                browser.clear();
                Button::Handled
            }))
    }

    fn pick(&self, row: FilterRow) -> Pick {
        let filters = self.browser.filters();
        let variants = self.browser.hle().map_or_else(
            || self.sample_variants.clone(),
            |hle| hle.browser_status().variants,
        );
        let choices = row.choices(&filters, &variants);
        let current = selected(&choices, &filters);
        let labels = choices
            .iter()
            .map(|c| text_fit::line(&c.label, text_fit::ROW))
            .collect();
        let browser = Arc::clone(&self.browser);
        let set = Arc::new(move |i| {
            let Some(choice) = choices.get(i) else {
                return String::new();
            };
            browser.update(|filters| row.apply(filters, &choice.filters));
            format!("{}: {}", row.label(), choice.label)
        });
        Pick::new(row.label(), labels, current, row.help(), set)
    }

    fn row(row: FilterRow, filters: &BrowserFilters) -> RowSpec {
        let choices = row.choices(filters, &[]);
        let current = &choices[selected(&choices, filters)];
        RowSpec::info(row.label(), &text_fit::line(&current.label, text_fit::ROW))
            .press(Press::Open(picker::NAME.into()))
            .describe(row.help())
    }

    fn page(self: Arc<Self>) -> ListPage {
        ListPage::new(self.class(), described(&self))
    }
}

impl Rows for FilterPage {
    fn title(&self) -> u32 {
        self.texts.set(PAGE_TITLE, "Game Filters")
    }

    fn count(&self) -> usize {
        FilterRow::ALL.len()
    }

    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some(filter) = FilterRow::at(row) else {
            return Vec::new();
        };
        let s = Self::row(filter, &self.browser.filters());
        let slot = PAGE_ROWS + 3 * row as u32;
        vec![
            (LABEL_SLOT, self.texts.set(slot, &s.label)),
            (
                VALUE_SLOT,
                self.texts
                    .set(slot + 1, &s.plain_value().unwrap_or_default()),
            ),
        ]
    }

    fn press(&self, row: i16) -> Chosen {
        FilterRow::at(row)
            .and_then(|filter| picker::offer_list(self.pick(filter)))
            .map_or(Chosen::Nothing, Chosen::Open)
    }

    fn description(&self, row: i16) -> Option<u32> {
        let filter = FilterRow::at(row)?;
        Some(
            self.texts
                .set(PAGE_ROWS + 3 * row as u32 + 2, filter.help()),
        )
    }

    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.texts.write(m)
    }
}

impl Describe for FilterPage {
    fn spec(&self) -> ScreenSpec {
        let filters = self.browser.filters();
        let mut s = ScreenSpec::new(SCREEN, "Game Filters");
        s.list = ListPlace::from(Some(dialog_tag::base_layout(BASE, self.count())));
        s.style = BASE.style();
        s.rows = FilterRow::ALL.map(|row| Self::row(row, &filters)).into();
        s.description = BASE.help().map(Into::into);
        s.prompts = prompts();
        s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B, Input::X];
        s.footer = true;
        s.dialog = Some(DialogSpec {
            base: BASE,
            rows: s.rows.clone(),
            ..DialogSpec::new("", Opener::XBOX_LIVE_MENU)
        });
        s
    }
}

pub(crate) fn sample_page() -> ListPage {
    Arc::new(FilterPage::new(
        Arc::new(Browser::new(BrowserFilters::default(), None, None)),
        vec!["Team Slayer".into(), "One Flag".into()],
    ))
    .page()
}

pub(crate) use guest::handle_event;

#[cfg(test)]
mod tests;
