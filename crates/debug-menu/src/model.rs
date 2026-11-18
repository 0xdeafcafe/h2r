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
        }
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
                step,
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
                step,
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
    /// Enter a section, run an action, flip a toggle.
    Select,
    /// Up a level; on the first page, close.
    Back,
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
}

/// A listed page is listed again this often while it shows.
pub const REFRESH: Duration = Duration::from_millis(1000);
/// The log keeps this many actions.
const LOG: usize = 256;

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
    },
}

/// The menu's state. Single-threaded; [`crate::Shared`] puts it behind a
/// lock for the input and render threads.
#[derive(Debug)]
pub struct Menu {
    sections: Vec<Section>,
    open: bool,
    stack: Vec<Level>,
    pages: HashMap<usize, Page>,
    next_id: u64,
    waiting: HashMap<u64, Waiting>,
    outbox: Vec<(u64, Call)>,
    log: Vec<Action>,
    status: Option<String>,
    /// Bumped whenever what the menu shows may have changed.
    generation: u64,
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
            status: None,
            generation: 0,
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

    /// The last error or note, shown at the bottom.
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
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

    pub fn cursor(&self) -> usize {
        self.level().cursor
    }

    /// The rows showing.
    pub fn rows(&self) -> Vec<Row> {
        match self.level().section {
            None => self
                .sections
                .iter()
                .map(|s| Row::open(s.title.clone(), &s.id).about(s.about.clone()))
                .collect(),
            Some(i) => match self.pages.get(&i) {
                Some(p) if !p.rows.is_empty() => p.rows.clone(),
                _ => vec![Row::info("loading", "...")],
            },
        }
    }

    /// The row under the cursor.
    pub fn selected(&self) -> Option<Row> {
        self.rows().into_iter().nth(self.cursor())
    }

    fn send(&mut self, call: Call, w: Waiting) {
        let id = self.next_id;
        self.next_id += 1;
        self.waiting.insert(id, w);
        self.record(Action::Call(call.clone()));
        self.outbox.push((id, call));
    }

    /// Lists section `s` again (its answers come back through [`answer`]).
    ///
    /// [`answer`]: Menu::answer
    fn list(&mut self, s: usize, now: Duration) {
        let calls = self.sections[s].calls.clone();
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
                let rows = self.sections[section].rows;
                let page = self.pages.entry(section).or_default();
                if page.generation != generation {
                    return;
                }
                if let Some(a) = page.answers.get_mut(slot) {
                    *a = Some(result);
                }
                page.rows = rows(&page.answers);
                let n = page.rows.len();
                for l in &mut self.stack {
                    if l.section == Some(section) {
                        l.cursor = l.cursor.min(n.saturating_sub(1));
                    }
                }
                self.changed();
            }
            Waiting::Change { section, method } => {
                match result {
                    Ok(_) => self.status = None,
                    Err(e) => {
                        self.status = Some(format!("{method}: {e}"));
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

    /// Lists the showing page again when it is older than [`REFRESH`].
    pub fn tick(&mut self, now: Duration) {
        if !self.open {
            return;
        }
        let Some(s) = self.level().section else {
            return;
        };
        let stale = self
            .pages
            .get(&s)
            .and_then(|p| p.asked)
            .is_none_or(|t| now.saturating_sub(t) >= REFRESH);
        let out = self.waiting.values().any(|w| match w {
            Waiting::List { section, .. } => *section == s,
            Waiting::Change { .. } => true,
        });
        if stale && !out {
            self.list(s, now);
        }
    }

    /// One navigation step. Ignored while closed.
    pub fn nav(&mut self, n: Nav, now: Duration) {
        if !self.open {
            return;
        }
        let rows = self.rows();
        let cursor = self.cursor().min(rows.len().saturating_sub(1));
        match n {
            Nav::Up | Nav::Down => {
                let to = if n == Nav::Up {
                    cursor.saturating_sub(1)
                } else {
                    (cursor + 1).min(rows.len().saturating_sub(1))
                };
                if to != self.cursor() {
                    self.level_mut().cursor = to;
                    self.record(Action::Move(to));
                }
            }
            Nav::Back => self.back(),
            Nav::Left | Nav::Right | Nav::Select => {
                let Some(row) = rows.get(cursor) else { return };
                self.act(row, n, now);
            }
        }
    }

    fn back(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
            self.record(Action::Back);
        } else {
            self.close();
        }
    }

    fn act(&mut self, row: &Row, n: Nav, now: Duration) {
        if let Edit::Open(id) = &row.edit {
            if n == Nav::Left {
                return;
            }
            match self.sections.iter().position(|s| &s.id == id) {
                Some(i) => {
                    self.stack.push(Level {
                        section: Some(i),
                        cursor: 0,
                    });
                    self.record(Action::Enter(id.clone()));
                    self.list(i, now);
                }
                None => self.refuse(row, format!("no section {id:?}")),
            }
            return;
        }
        if let Some(why) = &row.disabled {
            if !matches!(row.edit, Edit::None) {
                self.refuse(row, why.clone());
            }
            return;
        }
        let section = self.level().section;
        match &row.edit {
            Edit::None => {}
            Edit::Action(call) => {
                if n == Nav::Select {
                    let method = call.method.clone();
                    self.send(call.clone(), Waiting::Change { section, method });
                }
            }
            Edit::Int { .. } | Edit::Float { .. } if n == Nav::Select => {}
            _ => {
                let dir = if n == Nav::Left { -1 } else { 1 };
                let (Some(v), Some(set)) = (row.stepped(dir), &row.set) else {
                    if row.set.is_none() {
                        self.refuse(row, "read-only".into());
                    }
                    return;
                };
                let call = set.call(v.clone());
                // Shown at once; the listing after the answer confirms it.
                if let Some(s) = section
                    && let Some(p) = self.pages.get_mut(&s)
                {
                    p.generation += 1;
                    let cursor = self.stack.last().map_or(0, |l| l.cursor);
                    if let Some(r) = p.rows.get_mut(cursor) {
                        r.value = v;
                    }
                }
                let method = call.method.clone();
                self.send(call, Waiting::Change { section, method });
            }
        }
    }

    fn refuse(&mut self, row: &Row, why: String) {
        self.status = Some(format!("{}: {why}", row.label));
        self.record(Action::Refused {
            label: row.label.clone(),
            why,
        });
    }
}
