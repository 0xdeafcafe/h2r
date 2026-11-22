//! The menu as data: [`Row`]s built from registry answers, [`Section`]s
//! that say which calls list a page and how its answers become rows, and
//! [`Menu`], the navigation state machine.
//!
//! The menu never changes anything itself. Every value it shows came from
//! an agent call, and every change is an agent call ([`Call`]): the same
//! `render.toggle`, `edit.revert` or `cheat.set` that `h2 agent` sends.
//! After a change the page is listed again, so what it shows is always
//! what the registry says, whoever changed it.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Value, json};

/// One agent call: a method and its params.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub method: String,
    pub params: Value,
}

impl Call {
    pub fn new(method: &str, params: Value) -> Self {
        Call {
            method: method.to_owned(),
            params,
        }
    }

    /// `method` with `{}`.
    pub fn bare(method: &str) -> Self {
        Self::new(method, json!({}))
    }
}

impl std::fmt::Display for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.method, self.params)
    }
}

/// A call's answer: `None` while it is out, the error's message on failure.
pub type Answer = Option<Result<Value, String>>;

/// How a row's value changes: the call is `method` with `params`, the new
/// value put at `params[key]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Setter {
    pub method: String,
    pub params: Value,
    pub key: String,
}

impl Setter {
    pub fn new(method: &str, params: Value, key: &str) -> Self {
        Setter {
            method: method.to_owned(),
            params,
            key: key.to_owned(),
        }
    }

    /// The call that sets `v`.
    pub fn call(&self, v: Value) -> Call {
        let mut params = match &self.params {
            Value::Object(_) => self.params.clone(),
            _ => json!({}),
        };
        params[self.key.as_str()] = v;
        Call::new(&self.method, params)
    }
}

/// What a row does with left, right and select.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Read-only: shows its value.
    None,
    /// Left, right or select flips it.
    Bool,
    /// Left and right step it within `min..=max`.
    Int {
        step: i64,
        min: i64,
        max: i64,
    },
    Float {
        step: f64,
        min: f64,
        max: f64,
    },
    /// Left and right cycle through these (value, label) pairs; select
    /// goes to the next.
    Choice(Vec<(Value, String)>),
    /// Select makes the call (reload, revert).
    Action(Call),
    /// Select (or right) opens the section with this id.
    Open(String),
}

/// One line of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: String,
    pub value: Value,
    pub edit: Edit,
    /// How a change is made; editable rows without one are read-only.
    pub set: Option<Setter>,
    /// Shown greyed; changing it is refused with this reason (a restart,
    /// a co-op session, a section that is not there).
    pub disabled: Option<String>,
    /// Said under the menu while the row is selected.
    pub about: String,
    /// Destructive: a second select within [`CONFIRM`] makes the change.
    pub confirm: bool,
    /// Makes the toast text from the call's Ok result.
    pub on_ok: Option<OnOk>,
    /// What select resets a number to.
    pub default: Option<Value>,
    /// A divider line over the rows below it: the cursor skips it.
    pub heading: bool,
}

/// Turns an Ok result into the one-line toast ([`Row::on_ok`]).
#[derive(Clone, Copy, Debug)]
pub struct OnOk(pub fn(&Value) -> String);

impl PartialEq for OnOk {
    fn eq(&self, other: &Self) -> bool {
        self.0 as usize == other.0 as usize
    }
}

impl Row {
    fn new(label: impl Into<String>, value: Value, edit: Edit) -> Self {
        Row {
            label: label.into(),
            value,
            edit,
            set: None,
            disabled: None,
            about: String::new(),
            confirm: false,
            on_ok: None,
            default: None,
            heading: false,
        }
    }

    /// A divider over the rows below it; the cursor skips it.
    pub fn heading(label: impl Into<String>) -> Self {
        let mut r = Self::new(label, Value::Null, Edit::None);
        r.heading = true;
        r
    }

    /// Marks the row destructive: it asks for a second select within
    /// [`CONFIRM`] before it makes its call.
    pub fn confirm(mut self) -> Row {
        self.confirm = true;
        self
    }

    /// The toast text from the call's Ok result (the default is
    /// "<label>: done" for an action, "<label>: <value>" for an edit).
    pub fn on_ok(mut self, fmt: fn(&Value) -> String) -> Row {
        self.on_ok = Some(OnOk(fmt));
        self
    }

    /// What select resets a number to.
    pub fn default_value(mut self, v: impl Into<Value>) -> Row {
        self.default = Some(v.into());
        self
    }

    /// An explicit step for a number, instead of the one scaled to its
    /// range.
    pub fn step(mut self, step: f64) -> Row {
        match &mut self.edit {
            Edit::Int { step: s, .. } => *s = step.round().max(1.0) as i64,
            Edit::Float { step: s, .. } if step > 0.0 => *s = step,
            _ => {}
        }
        self
    }

    /// A read-only value.
    pub fn info(label: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::new(label, value.into(), Edit::None)
    }

    pub fn toggle(label: impl Into<String>, on: bool, set: Setter) -> Self {
        Self::new(label, Value::Bool(on), Edit::Bool).with_set(set)
    }

    pub fn int(
        label: impl Into<String>,
        v: i64,
        step: i64,
        range: (i64, i64),
        set: Setter,
    ) -> Self {
        Self::new(
            label,
            json!(v),
            Edit::Int {
                step: step.max(scaled_int(range)),
                min: range.0,
                max: range.1,
            },
        )
        .with_set(set)
    }

    pub fn float(
        label: impl Into<String>,
        v: f64,
        step: f64,
        range: (f64, f64),
        set: Setter,
    ) -> Self {
        Self::new(
            label,
            json!(v),
            Edit::Float {
                step: step.max(scaled_float(range)),
                min: range.0,
                max: range.1,
            },
        )
        .with_set(set)
    }

    pub fn choice(
        label: impl Into<String>,
        v: Value,
        choices: Vec<(Value, String)>,
        set: Setter,
    ) -> Self {
        Self::new(label, v, Edit::Choice(choices)).with_set(set)
    }

    pub fn action(label: impl Into<String>, call: Call) -> Self {
        Self::new(label, Value::Null, Edit::Action(call))
    }

    pub fn open(label: impl Into<String>, section: &str) -> Self {
        Self::new(label, Value::Null, Edit::Open(section.to_owned()))
    }

    pub fn with_set(mut self, set: Setter) -> Self {
        self.set = Some(set);
        self
    }

    pub fn disabled(mut self, why: impl Into<String>) -> Self {
        self.disabled = Some(why.into());
        self
    }

    pub fn about(mut self, about: impl Into<String>) -> Self {
        self.about = about.into();
        self
    }

    /// The value as the menu shows it.
    pub fn shown(&self) -> String {
        match (&self.edit, &self.value) {
            (Edit::Open(_), _) => ">".into(),
            (Edit::Action(_), _) => String::new(),
            (Edit::Choice(c), v) => c
                .iter()
                .find(|(cv, _)| same(cv, v))
                .map_or_else(|| text(v), |(_, l)| l.clone()),
            (Edit::Float { step, .. }, Value::Number(n)) => {
                let places = decimals(*step);
                format!("{:.*}", places, n.as_f64().unwrap_or(0.0))
            }
            (_, v) => text(v),
        }
    }

    /// The value `dir` (-1 left, +1 right) steps would set; `None` when
    /// it can't step (read-only, at a bound, not a value).
    pub fn stepped(&self, dir: i64) -> Option<Value> {
        match &self.edit {
            Edit::Bool => Some(Value::Bool(!self.value.as_bool().unwrap_or(false))),
            Edit::Int { step, min, max } => {
                let now = self.value.as_i64().unwrap_or(*min);
                let v = now.saturating_add(dir * step).clamp(*min, *max);
                (v != now).then(|| json!(v))
            }
            Edit::Float { step, min, max } => {
                let now = self.value.as_f64().unwrap_or(0.0);
                let raw = (now + dir as f64 * step).clamp(*min, *max);
                // Round to the step's places, so steps don't drift.
                let k = 10f64.powi(decimals(*step) as i32 + 1);
                let v = (raw * k).round() / k;
                (v != now).then(|| json!(v))
            }
            Edit::Choice(c) if !c.is_empty() => {
                let n = c.len() as i64;
                let at = c
                    .iter()
                    .position(|(cv, _)| same(cv, &self.value))
                    .map_or(-1, |i| i as i64);
                let next = if at < 0 { 0 } else { (at + dir).rem_euclid(n) };
                Some(c[next as usize].0.clone())
            }
            _ => None,
        }
    }
}

/// Numbers compare by value (`1` and `1.0` are the same choice).
fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// `raw` rounded to the nearest 1, 2, 5 or 10 times a power of ten (a
/// step that suits a range of 64 of them).
pub fn nice_step(raw: f64) -> f64 {
    if !raw.is_finite() || raw <= 0.0 {
        return 0.0;
    }
    let exp = raw.log10().floor();
    let base = 10f64.powf(exp);
    let m = raw / base;
    let nice = if m < 1.5 {
        1.0
    } else if m < 3.5 {
        2.0
    } else if m < 7.5 {
        5.0
    } else {
        10.0
    };
    // Powers of ten round off in the last place: trim them.
    let v = nice * base;
    (v * 1e9).round() / 1e9
}

/// Ranges wider than this are "unbounded": nothing to scale a step to.
const UNBOUNDED: f64 = 1e9;

fn scaled_int(range: (i64, i64)) -> i64 {
    let span = range.1.saturating_sub(range.0) as f64;
    if span > UNBOUNDED {
        return 1;
    }
    (nice_step(span / 64.0).round() as i64).max(1)
}

fn scaled_float(range: (f64, f64)) -> f64 {
    let span = range.1 - range.0;
    if span > UNBOUNDED {
        return 0.0;
    }
    nice_step(span / 64.0)
}

fn decimals(step: f64) -> usize {
    let mut places = 0;
    let mut s = step.abs();
    while places < 4 && s > 0.0 && (s - s.round()).abs() > 1e-9 {
        s *= 10.0;
        places += 1;
    }
    places
}

/// A value as text: `on`/`off`, numbers trimmed, strings bare.
pub fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(true) => "on".into(),
        Value::Bool(false) => "off".into(),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => i.to_string(),
            (None, Some(f)) => {
                let s = format!("{f:.3}");
                s.trim_end_matches('0').trim_end_matches('.').to_owned()
            }
            _ => n.to_string(),
        },
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// How a section's answers become rows. Pure, so each is tested on
/// synthetic answers.
pub type RowsFn = fn(&[Answer]) -> Vec<Row>;

/// A page of the menu: the calls that list it and how their answers
/// become rows. Add one with [`Menu::add_section`].
#[derive(Clone, Debug)]
pub struct Section {
    /// Stable id (`render`, `cheats`); [`Edit::Open`] names it.
    pub id: String,
    pub title: String,
    /// Said under the menu while its row is selected on the first page.
    pub about: String,
    pub calls: Vec<Call>,
    pub rows: RowsFn,
}

/// What the user does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    /// Enter a section, run an action, flip a toggle, reset a number.
    Select,
    /// Up a level; on the first page, close. Clears the search first.
    Back,
    /// A screenful up ([`PAGE`] rows).
    PageUp,
    PageDown,
    /// The first and last row.
    Home,
    End,
    /// Star or unstar the selected row.
    Favourite,
}

/// The input device last used: the footer teaches its controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Device {
    #[default]
    Keyboard,
    /// A pad with a touchpad, driven by its touchpad.
    Touchpad,
    /// Any pad, driven by its buttons.
    Pad,
}

/// What happened, in order (for tests and the log).
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Open,
    Close,
    /// The cursor moved to this row.
    Move(usize),
    /// A section was entered.
    Enter(String),
    /// Up a level.
    Back,
    /// A call went out.
    Call(Call),
    /// A change was asked of a row that can't change, and why not.
    Refused {
        label: String,
        why: String,
    },
    /// A change's answer was an error.
    Failed {
        method: String,
        error: String,
    },
    /// A row was starred (true) or unstarred.
    Star {
        label: String,
        on: bool,
    },
}

/// A listed page is listed again this often while it shows.
pub const REFRESH: Duration = Duration::from_millis(1000);
/// The log keeps this many actions.
const LOG: usize = 256;
/// A result or error shows this long.
pub const TOAST: Duration = Duration::from_millis(3000);
/// A destructive row's second select must come within this.
pub const CONFIRM: Duration = Duration::from_millis(2000);
/// Rows a page up or down moves; the overlay shows as many.
pub const PAGE: usize = 14;
/// Recent keeps this many actions.
pub const RECENT: usize = 5;

/// Where the pinned header's page is kept.
const HEADER: usize = usize::MAX;

/// One line saying what the last change did (or why it did not).
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    until: Duration,
}

/// A starred or recent row: the section's id and the row's label key it;
/// the row itself is the last one seen.
#[derive(Clone, Debug)]
struct Entry {
    section: String,
    label: String,
    row: Row,
}

/// A row as showing, with where it came from.
#[derive(Clone, Debug)]
struct Item {
    row: Row,
    /// The section whose page the row is on (`None`: the first page's own
    /// section rows).
    section: Option<usize>,
    /// The row's own label (a search shows it with its section's).
    key: String,
}

#[derive(Clone, Debug, Default)]
struct Page {
    answers: Vec<Answer>,
    rows: Vec<Row>,
    /// Bumped by each listing and change: older answers are dropped.
    generation: u64,
    asked: Option<Duration>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Level {
    /// `None`: the first page (the sections).
    section: Option<usize>,
    cursor: usize,
}

#[derive(Clone, Debug)]
enum Waiting {
    List {
        section: usize,
        slot: usize,
        generation: u64,
    },
    Change {
        section: Option<usize>,
        method: String,
        label: String,
        /// The value an edit set, as shown; `None` for an action.
        shown: Option<String>,
        on_ok: Option<OnOk>,
    },
}

/// The menu's state. Single-threaded; [`crate::Shared`] puts it behind a
/// lock for the input and render threads.
#[derive(Debug)]
pub struct Menu {
    sections: Vec<Section>,
    header: Option<Section>,
    open: bool,
    stack: Vec<Level>,
    pages: HashMap<usize, Page>,
    next_id: u64,
    waiting: HashMap<u64, Waiting>,
    outbox: Vec<(u64, Call)>,
    log: Vec<Action>,
    toast: Option<Toast>,
    /// Bumped whenever what the menu shows may have changed.
    generation: u64,
    device: Device,
    favs: Vec<Entry>,
    recent: Vec<Entry>,
    /// The search text, over every section's rows.
    filter: String,
    /// Typing a search: the keys that mean something else (F stars, Space
    /// selects) are letters now.
    typing: bool,
    filter_cursor: usize,
    /// The destructive row asked once, and when.
    confirm: Option<(String, Duration)>,
}

impl Default for Menu {
    fn default() -> Self {
        Self::with_sections(crate::sources::all())
    }
}

impl Menu {
    /// The menu with these sections on its first page, in order.
    pub fn with_sections(sections: Vec<Section>) -> Self {
        Menu {
            sections,
            header: None,
            open: false,
            stack: vec![Level {
                section: None,
                cursor: 0,
            }],
            pages: HashMap::new(),
            next_id: 1,
            waiting: HashMap::new(),
            outbox: Vec::new(),
            log: Vec::new(),
            toast: None,
            generation: 0,
            device: Device::default(),
            favs: Vec::new(),
            recent: Vec::new(),
            filter: String::new(),
            typing: false,
            filter_cursor: 0,
            confirm: None,
        }
    }

    /// Adds a section to the first page (a `Players` page over `sim.*`
    /// goes in here). One with the same id is replaced.
    pub fn add_section(&mut self, s: Section) {
        match self.sections.iter_mut().find(|x| x.id == s.id) {
            Some(x) => *x = s,
            None => self.sections.push(s),
        }
        self.changed();
    }

    /// The header shown above every page: its rows are listed like a
    /// section's, with its own calls, and drawn as status lines.
    pub fn set_header(&mut self, s: Section) {
        self.pages.remove(&HEADER);
        self.header = Some(s);
        self.changed();
    }

    /// The header's rows as last listed.
    pub fn header_rows(&self) -> Vec<Row> {
        self.pages
            .get(&HEADER)
            .map(|p| p.rows.clone())
            .unwrap_or_default()
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Changes whenever what the menu shows may have.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn record(&mut self, a: Action) {
        if self.log.len() == LOG {
            self.log.remove(0);
        }
        self.log.push(a);
        self.changed();
    }

    /// Everything that happened, oldest first.
    pub fn log(&self) -> &[Action] {
        &self.log
    }

    pub fn take_log(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.log)
    }

    /// The last error, while it shows.
    pub fn status(&self) -> Option<&str> {
        self.toast
            .as_ref()
            .filter(|t| t.error)
            .map(|t| t.text.as_str())
    }

    /// The last result or error, until [`TOAST`] after it.
    pub fn toast(&self) -> Option<&Toast> {
        self.toast.as_ref()
    }

    fn say(&mut self, text: String, error: bool, now: Duration, hold: Duration) {
        self.toast = Some(Toast {
            text,
            error,
            until: now + hold,
        });
        self.changed();
    }

    /// The device the footer teaches.
    pub fn device(&self) -> Device {
        self.device
    }

    /// Notes the device the last input came from.
    pub fn set_device(&mut self, d: Device) {
        if self.device != d {
            self.device = d;
            self.changed();
        }
    }

    /// The search text.
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Whether keys are typing a search.
    pub fn typing(&self) -> bool {
        self.typing
    }

    /// Opens it where it was left, listing that page again.
    pub fn open(&mut self, now: Duration) {
        if self.open {
            return;
        }
        self.open = true;
        self.record(Action::Open);
        if let Some(s) = self.level().section {
            self.list(s, now);
        }
    }

    pub fn close(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        self.record(Action::Close);
    }

    pub fn toggle(&mut self, now: Duration) {
        if self.open {
            self.close();
        } else {
            self.open(now);
        }
    }

    fn level(&self) -> Level {
        *self.stack.last().expect("the first page is never popped")
    }

    fn level_mut(&mut self) -> &mut Level {
        self.stack
            .last_mut()
            .expect("the first page is never popped")
    }

    /// The section showing, `None` on the first page.
    pub fn current(&self) -> Option<&Section> {
        self.level().section.map(|i| &self.sections[i])
    }

    /// The breadcrumb: section titles from the first page down.
    pub fn path(&self) -> Vec<&str> {
        self.stack
            .iter()
            .filter_map(|l| l.section.map(|i| self.sections[i].title.as_str()))
            .collect()
    }

    fn filtering(&self) -> bool {
        !self.filter.is_empty()
    }

    /// The cursor: an index into [`rows`](Menu::rows).
    pub fn cursor(&self) -> usize {
        if self.filtering() {
            self.filter_cursor
        } else {
            self.level().cursor
        }
    }

    fn set_cursor(&mut self, to: usize) {
        if self.filtering() {
            self.filter_cursor = to;
        } else {
            self.level_mut().cursor = to;
        }
    }

    fn section_of(&self, id: &str) -> Option<usize> {
        self.sections.iter().position(|s| s.id == id)
    }

    fn section_id(&self, s: Option<usize>) -> String {
        s.map(|s| self.sections[s].id.clone()).unwrap_or_default()
    }

    fn entry_item(&self, e: &Entry) -> Option<Item> {
        let section = if e.section.is_empty() {
            None
        } else {
            Some(self.section_of(&e.section)?)
        };
        Some(Item {
            row: e.row.clone(),
            section,
            key: e.label.clone(),
        })
    }

    fn divider(title: &str) -> Item {
        Item {
            row: Row::heading(title),
            section: None,
            key: String::new(),
        }
    }

    /// Every row showing, with where it came from.
    fn items(&self) -> Vec<Item> {
        if self.filtering() {
            return self.filtered();
        }
        match self.level().section {
            None => {
                let mut out = Vec::new();
                for (title, list) in [("Favourites", &self.favs), ("Recent", &self.recent)] {
                    let items: Vec<Item> = list.iter().filter_map(|e| self.entry_item(e)).collect();
                    if !items.is_empty() {
                        out.push(Self::divider(title));
                        out.extend(items);
                    }
                }
                if !out.is_empty() {
                    out.push(Self::divider("Sections"));
                }
                out.extend(self.sections.iter().map(|s| Item {
                    row: Row::open(s.title.clone(), &s.id).about(s.about.clone()),
                    section: None,
                    key: s.title.clone(),
                }));
                out
            }
            Some(i) => match self.pages.get(&i) {
                Some(p) if !p.rows.is_empty() => p
                    .rows
                    .iter()
                    .map(|r| Item {
                        row: r.clone(),
                        section: Some(i),
                        key: r.label.clone(),
                    })
                    .collect(),
                _ => vec![Item {
                    row: Row::info("loading", "..."),
                    section: Some(i),
                    key: "loading".into(),
                }],
            },
        }
    }

    /// The rows whose label or section matches the search, from every
    /// section's last listing.
    fn filtered(&self) -> Vec<Item> {
        let q = self.filter.to_lowercase();
        let mut out = Vec::new();
        for (i, s) in self.sections.iter().enumerate() {
            if s.title.to_lowercase().contains(&q) {
                out.push(Item {
                    row: Row::open(s.title.clone(), &s.id).about(s.about.clone()),
                    section: None,
                    key: s.title.clone(),
                });
            }
            let Some(p) = self.pages.get(&i) else {
                continue;
            };
            for r in &p.rows {
                if r.heading || !r.label.to_lowercase().contains(&q) {
                    continue;
                }
                let mut shown = r.clone();
                shown.label = format!("{} > {}", s.title, r.label);
                out.push(Item {
                    row: shown,
                    section: Some(i),
                    key: r.label.clone(),
                });
            }
        }
        out
    }

    /// The rows showing.
    pub fn rows(&self) -> Vec<Row> {
        self.items().into_iter().map(|i| i.row).collect()
    }

    /// For each of [`rows`](Menu::rows): starred.
    pub fn starred(&self) -> Vec<bool> {
        self.items()
            .iter()
            .map(|i| {
                let id = self.section_id(i.section);
                !i.row.heading
                    && self
                        .favs
                        .iter()
                        .any(|e| e.section == id && e.label == i.key)
            })
            .collect()
    }

    /// The row under the cursor.
    pub fn selected(&self) -> Option<Row> {
        self.items().into_iter().nth(self.cursor()).map(|i| i.row)
    }

    /// What `h2 agent` would send for the selected row: its call, or the
    /// call that flips or steps it.
    pub fn selected_call(&self) -> Option<Call> {
        let row = self.selected()?;
        match &row.edit {
            Edit::Action(c) => Some(c.clone()),
            _ => {
                let set = row.set.as_ref()?;
                let v = row.stepped(1).unwrap_or_else(|| row.value.clone());
                Some(set.call(v))
            }
        }
    }

    /// Moves a cursor that sits on a divider to the next row.
    fn settle(&mut self) {
        let items = self.items();
        let at = self.cursor();
        let to = match items.get(at) {
            Some(i) if i.row.heading => (at..items.len())
                .chain((0..at).rev())
                .find(|&k| !items[k].row.heading)
                .unwrap_or(at),
            Some(_) => at,
            None => items.len().saturating_sub(1),
        };
        if to != at {
            self.set_cursor(to);
        }
    }

    fn send(&mut self, call: Call, w: Waiting) {
        let id = self.next_id;
        self.next_id += 1;
        self.waiting.insert(id, w);
        self.record(Action::Call(call.clone()));
        self.outbox.push((id, call));
    }

    fn section(&self, s: usize) -> &Section {
        if s == HEADER {
            self.header.as_ref().expect("a header page has a header")
        } else {
            &self.sections[s]
        }
    }

    /// Lists section `s` again (its answers come back through [`answer`]).
    ///
    /// [`answer`]: Menu::answer
    fn list(&mut self, s: usize, now: Duration) {
        let calls = self.section(s).calls.clone();
        let page = self.pages.entry(s).or_default();
        page.generation += 1;
        page.asked = Some(now);
        page.answers.resize(calls.len(), None);
        let generation = page.generation;
        for (slot, c) in calls.into_iter().enumerate() {
            self.send(
                c,
                Waiting::List {
                    section: s,
                    slot,
                    generation,
                },
            );
        }
    }

    /// Calls waiting to go out, with their ids.
    pub fn take_outbox(&mut self) -> Vec<(u64, Call)> {
        std::mem::take(&mut self.outbox)
    }

    /// Whether any call is still out.
    pub fn waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// The answer to call `id`.
    pub fn answer(&mut self, id: u64, result: Result<Value, String>, now: Duration) {
        let Some(w) = self.waiting.remove(&id) else {
            return;
        };
        match w {
            Waiting::List {
                section,
                slot,
                generation,
            } => {
                let rows = self.section(section).rows;
                let page = self.pages.entry(section).or_default();
                if page.generation != generation {
                    return;
                }
                if let Some(a) = page.answers.get_mut(slot) {
                    *a = Some(result);
                }
                page.rows = rows(&page.answers);
                let n = page.rows.len();
                let fresh = page.rows.clone();
                if section != HEADER {
                    for l in &mut self.stack {
                        if l.section == Some(section) {
                            l.cursor = l.cursor.min(n.saturating_sub(1));
                        }
                    }
                    let id = self.sections[section].id.clone();
                    for e in self.favs.iter_mut().chain(self.recent.iter_mut()) {
                        if e.section == id
                            && let Some(r) = fresh.iter().find(|r| r.label == e.label)
                        {
                            e.row = r.clone();
                        }
                    }
                }
                self.settle();
                self.changed();
            }
            Waiting::Change {
                section,
                method,
                label,
                shown,
                on_ok,
            } => {
                match result {
                    Ok(v) => {
                        let text = match (on_ok, shown) {
                            (Some(OnOk(f)), _) => f(&v),
                            (None, Some(s)) if !s.is_empty() => format!("{label}: {s}"),
                            _ => format!("{label}: done"),
                        };
                        self.say(text, false, now, TOAST);
                    }
                    Err(e) => {
                        self.say(format!("{method}: {e}"), true, now, TOAST);
                        self.record(Action::Failed { method, error: e });
                    }
                }
                if let Some(s) = section
                    && self.open
                {
                    self.list(s, now);
                }
                self.changed();
            }
        }
    }

    /// Lists the pages that show again when they are older than
    /// [`REFRESH`], and lets an old toast go.
    pub fn tick(&mut self, now: Duration) {
        if self.toast.as_ref().is_some_and(|t| now >= t.until) {
            self.toast = None;
            self.changed();
        }
        if self
            .confirm
            .as_ref()
            .is_some_and(|(_, t)| now.saturating_sub(*t) > CONFIRM)
        {
            self.confirm = None;
        }
        if !self.open {
            return;
        }
        if self
            .waiting
            .values()
            .any(|w| matches!(w, Waiting::Change { .. }))
        {
            return;
        }
        let mut want: Vec<usize> = Vec::new();
        if self.header.is_some() {
            want.push(HEADER);
        }
        if self.filtering() {
            want.extend(0..self.sections.len());
        } else if let Some(s) = self.level().section {
            want.push(s);
        } else {
            for e in self.favs.iter().chain(self.recent.iter()) {
                if let Some(i) = self.section_of(&e.section)
                    && !want.contains(&i)
                {
                    want.push(i);
                }
            }
        }
        for s in want {
            let stale = self
                .pages
                .get(&s)
                .and_then(|p| p.asked)
                .is_none_or(|t| now.saturating_sub(t) >= REFRESH);
            let out = self
                .waiting
                .values()
                .any(|w| matches!(w, Waiting::List { section, .. } if *section == s));
            if stale && !out {
                self.list(s, now);
            }
        }
    }

    /// One navigation step. Ignored while closed.
    pub fn nav(&mut self, n: Nav, now: Duration) {
        if !self.open {
            return;
        }
        if n == Nav::Back {
            self.back();
            return;
        }
        self.settle();
        let items = self.items();
        if items.is_empty() {
            return;
        }
        let last = items.len() - 1;
        let cursor = self.cursor().min(last);
        let pickable = |k: usize| !items[k].row.heading;
        match n {
            Nav::Up | Nav::Down => {
                // Wraps at the top and bottom.
                let len = items.len();
                let mut to = cursor;
                for _ in 0..len {
                    to = if n == Nav::Up {
                        (to + len - 1) % len
                    } else {
                        (to + 1) % len
                    };
                    if pickable(to) {
                        break;
                    }
                }
                self.move_to(to, cursor);
            }
            Nav::PageUp | Nav::PageDown => {
                let raw = if n == Nav::PageUp {
                    cursor.saturating_sub(PAGE)
                } else {
                    (cursor + PAGE).min(last)
                };
                let to = if pickable(raw) {
                    raw
                } else if n == Nav::PageUp {
                    (raw..=last).find(|&k| pickable(k)).unwrap_or(cursor)
                } else {
                    (0..=raw).rev().find(|&k| pickable(k)).unwrap_or(cursor)
                };
                self.move_to(to, cursor);
            }
            Nav::Home => {
                let to = (0..=last).find(|&k| pickable(k)).unwrap_or(cursor);
                self.move_to(to, cursor);
            }
            Nav::End => {
                let to = (0..=last).rev().find(|&k| pickable(k)).unwrap_or(cursor);
                self.move_to(to, cursor);
            }
            Nav::Favourite => {
                if let Some(item) = items.get(cursor) {
                    self.star(item.clone(), now);
                }
            }
            Nav::Left | Nav::Right | Nav::Select => {
                if let Some(item) = items.get(cursor) {
                    self.act(item, n, now);
                }
            }
            Nav::Back => {}
        }
    }

    fn move_to(&mut self, to: usize, from: usize) {
        if to != from {
            self.set_cursor(to);
            self.record(Action::Move(to));
        }
    }

    /// A printable key: the start or the next letter of a search. Before
    /// the search starts, `f` stars the selected row and `/` starts it.
    pub fn type_char(&mut self, c: char, now: Duration) {
        if !self.open || c.is_control() {
            return;
        }
        if !self.typing && matches!(c, 'f' | 'F') {
            self.nav(Nav::Favourite, now);
            return;
        }
        if !self.typing && c == '/' {
            self.typing = true;
            self.changed();
            return;
        }
        self.typing = true;
        self.filter.extend(c.to_lowercase());
        self.filter_cursor = 0;
        self.settle();
        self.changed();
    }

    /// Backspace: a letter of the search, or back.
    pub fn erase(&mut self, now: Duration) {
        if !self.open {
            return;
        }
        if self.filter.pop().is_some() {
            self.filter_cursor = 0;
            self.changed();
        } else if self.typing {
            self.typing = false;
            self.changed();
        } else {
            self.nav(Nav::Back, now);
        }
    }

    fn back(&mut self) {
        if self.typing || self.filtering() {
            self.typing = false;
            self.filter.clear();
            self.filter_cursor = 0;
            self.changed();
            return;
        }
        if self.stack.len() > 1 {
            let left = self.stack.pop().and_then(|l| l.section);
            self.record(Action::Back);
            // The first page grows rows above the sections: land on the
            // section just left.
            if let (Some(i), None) = (left, self.level().section) {
                let title = self.sections[i].title.clone();
                let at = self
                    .items()
                    .iter()
                    .position(|it| it.section.is_none() && !it.row.heading && it.key == title);
                if let Some(at) = at {
                    self.level_mut().cursor = at;
                }
            }
        } else {
            self.close();
        }
    }

    fn star(&mut self, item: Item, now: Duration) {
        if item.row.heading {
            return;
        }
        let id = self.section_id(item.section);
        let mut row = item.row;
        row.label = item.key.clone();
        let at = self
            .favs
            .iter()
            .position(|e| e.section == id && e.label == item.key);
        let on = at.is_none();
        match at {
            Some(at) => {
                self.favs.remove(at);
            }
            None => self.favs.push(Entry {
                section: id,
                label: item.key.clone(),
                row,
            }),
        }
        let text = format!("{} {}", item.key, if on { "starred" } else { "unstarred" });
        self.say(text, false, now, TOAST);
        self.record(Action::Star {
            label: item.key,
            on,
        });
        self.settle();
    }

    /// Notes an action or edit in Recent (not from the pinned rows or the
    /// first page, where the row is already listed).
    fn remember(&mut self, item: &Item, row: &Row) {
        if item.section.is_none() || (self.level().section.is_none() && !self.filtering()) {
            return;
        }
        let id = self.section_id(item.section);
        self.recent
            .retain(|e| !(e.section == id && e.label == item.key));
        let mut row = row.clone();
        row.label = item.key.clone();
        self.recent.insert(
            0,
            Entry {
                section: id,
                label: item.key.clone(),
                row,
            },
        );
        self.recent.truncate(RECENT);
    }

    /// A destructive row's second select, in time.
    fn confirmed(&mut self, item: &Item, now: Duration) -> bool {
        if !item.row.confirm {
            return true;
        }
        let key = format!("{}/{}", self.section_id(item.section), item.key);
        if let Some((k, t)) = &self.confirm
            && *k == key
            && now.saturating_sub(*t) <= CONFIRM
        {
            self.confirm = None;
            return true;
        }
        self.confirm = Some((key, now));
        self.say(
            format!("press again to {}", item.row.label),
            false,
            now,
            CONFIRM,
        );
        false
    }

    fn act(&mut self, item: &Item, n: Nav, now: Duration) {
        let row = &item.row;
        if let Edit::Open(id) = &row.edit {
            if n == Nav::Left {
                return;
            }
            match self.section_of(id) {
                Some(i) => {
                    // From a search the pages stack up from the first.
                    if self.filtering() {
                        self.stack.truncate(1);
                    }
                    self.typing = false;
                    self.filter.clear();
                    self.stack.push(Level {
                        section: Some(i),
                        cursor: 0,
                    });
                    self.record(Action::Enter(id.clone()));
                    self.list(i, now);
                }
                None => self.refuse(row, format!("no section {id:?}"), now),
            }
            return;
        }
        if let Some(why) = &row.disabled {
            if !matches!(row.edit, Edit::None) {
                self.refuse(row, why.clone(), now);
            }
            return;
        }
        let section = item.section;
        let label = item.key.clone();
        match &row.edit {
            Edit::None => {}
            Edit::Action(call) => {
                if n == Nav::Select && self.confirmed(item, now) {
                    self.remember(item, row);
                    let method = call.method.clone();
                    self.send(
                        call.clone(),
                        Waiting::Change {
                            section,
                            method,
                            label,
                            shown: None,
                            on_ok: row.on_ok,
                        },
                    );
                }
            }
            Edit::Int { .. } | Edit::Float { .. } if n == Nav::Select => {
                // Select puts a number back to its default.
                let (Some(d), Some(set)) = (&row.default, &row.set) else {
                    return;
                };
                if same(d, &row.value) {
                    self.say(format!("{label}: already the default"), false, now, TOAST);
                    return;
                }
                if !self.confirmed(item, now) {
                    return;
                }
                self.write(item, set.call(d.clone()), d.clone());
            }
            _ => {
                let dir = if n == Nav::Left { -1 } else { 1 };
                let (Some(v), Some(set)) = (row.stepped(dir), &row.set) else {
                    if row.set.is_none() {
                        self.refuse(row, "read-only".into(), now);
                    }
                    return;
                };
                if !self.confirmed(item, now) {
                    return;
                }
                self.write(item, set.call(v.clone()), v);
            }
        }
    }

    /// Makes an edit's call, showing the new value at once; the listing
    /// after the answer confirms it.
    fn write(&mut self, item: &Item, call: Call, v: Value) {
        let mut changed = item.row.clone();
        changed.value = v;
        if let Some(s) = item.section
            && let Some(p) = self.pages.get_mut(&s)
        {
            p.generation += 1;
            if let Some(r) = p.rows.iter_mut().find(|r| r.label == item.key) {
                r.value = changed.value.clone();
            }
        }
        self.remember(item, &changed);
        let id = self.section_id(item.section);
        for e in self.favs.iter_mut().chain(self.recent.iter_mut()) {
            if e.section == id && e.label == item.key {
                e.row.value = changed.value.clone();
            }
        }
        let method = call.method.clone();
        self.send(
            call,
            Waiting::Change {
                section: item.section,
                method,
                label: item.key.clone(),
                shown: Some(changed.shown()),
                on_ok: item.row.on_ok,
            },
        );
    }

    fn refuse(&mut self, row: &Row, why: String, now: Duration) {
        self.say(format!("{}: {why}", row.label), true, now, TOAST);
        self.record(Action::Refused {
            label: row.label.clone(),
            why,
        });
    }
}
