//! A screen's behaviour as a pure state machine (docs/game/ui-builder.md,
//! "Behaviour"): a [`Page`] steps a state on an event and says what the
//! glue must do ([`Page::Effect`]); its [`ScreenSpec`] for any state is what
//! the preview draws and the checks read. No guest memory, no game: the
//! table tests step every native screen through its presses, and the
//! editor's UI Screens tool steps the same pages live ([`Live`]).
//!
//! - [`ListPage`]: any screen on [`crate::list_class`], stepped the way its
//!   class routes an event ([`list_class::route`]): the class's own hook,
//!   then the action buttons, then sideways steps, then the Controls
//!   handler (up and down move the focus, A presses the row, B and Back
//!   close). Its rows keep their own state (the profile store, the
//!   lobby); the page's state is the focus.
//! - [`AccountPage`]: the account screen ([`AccountScreen`], already pure).
//! - [`crate::signin_screen::SigninPage`]: the sign-in prompt, with the
//!   HLE's own transitions ([`live_hle::signin`]).
//!
//! **Not traced:** whether the game's list wraps the focus at either end.
//! [`ListPage`] stops it there; the fidelity loop settles it.

use std::fmt::Debug;
use std::sync::Arc;

use crate::account_screen::{self, AccountScreen, Outcome};
use crate::list_class::{self, Button, ListClass, Route};
use crate::ui::Chosen;
use crate::ui_kit::input::Input;
use crate::ui_kit::spec::{ScreenSpec, ValueStyle};
use live_hle::account_setup::SetupError;

/// A screen's behaviour.
pub trait Page {
    type State: Clone + Debug;
    /// What steps it: at least the pad's inputs.
    type Event: Clone + Debug + From<Input>;
    /// What a step asks of the glue.
    type Effect: Clone + Debug + PartialEq;
    fn step(&self, state: &mut Self::State, event: Self::Event) -> Self::Effect;
    /// Whether an effect reports a handled controller input.
    fn handled(effect: &Self::Effect) -> bool;
    /// The screen in `state`, as the preview and the checks take it.
    fn spec(&self, state: &Self::State) -> ScreenSpec;
    /// The named states a preview sheet draws, the first the one it opens
    /// in.
    fn states(&self) -> Vec<(String, Self::State)>;
    /// A line saying what `state` shows, for table tests.
    fn summary(&self, state: &Self::State) -> String;
}

/// `page`'s state named `name`. Panics on a name it doesn't have (a test's
/// typo).
pub fn named<P: Page>(page: &P, name: &str) -> P::State {
    page.states()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no state {name:?}"))
        .1
}

/// `events` in turn on `state`: their effects.
pub fn run<P: Page>(
    page: &P,
    state: &mut P::State,
    events: impl IntoIterator<Item = P::Event>,
) -> Vec<P::Effect> {
    events.into_iter().map(|e| page.step(state, e)).collect()
}

/// The focused row as `label` or `label = value`, `focus N` first.
pub fn focus_line(s: &ScreenSpec) -> String {
    match s.rows.get(s.focus).map(|r| r.cells(ValueStyle::Column)) {
        Some((label, Some(v))) => format!("focus {} {label} = {v}", s.focus),
        Some((label, None)) => format!("focus {} {label}", s.focus),
        None => format!("focus {} (no row)", s.focus),
    }
}

// ---- list screens ----

/// What an event does on a list screen of ours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListEffect {
    /// Nothing changed.
    None,
    /// The focus moved to this row.
    Focus(usize),
    /// The class's own hook took it ([`list_class::Buttons`]).
    Hook,
    /// The class's action runs (X or Start, or A on an [`Chosen::Act`] row).
    Action,
    /// A row stepped (A or a sideways step), or was locked: the log line.
    Stepped(String),
    /// A screen of ours opens.
    Open(&'static str),
    /// A retail screen opens: the log line.
    Retail(String),
    /// The screen closes (B or Back).
    Close,
}

/// A screen on [`crate::list_class`]: its class (rows, hook, action,
/// sideways steps) and its spec.
pub struct ListPage {
    pub class: ListClass,
    spec: Box<dyn Fn() -> ScreenSpec>,
}

/// A list page's state: the focused row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Focus(pub usize);

impl ListPage {
    pub fn new(class: ListClass, spec: impl Fn() -> ScreenSpec + 'static) -> Self {
        ListPage {
            class,
            spec: Box::new(spec),
        }
    }

    fn count(&self) -> usize {
        self.class.behaviour().rows.count()
    }
}

impl Page for ListPage {
    type State = Focus;
    type Event = Input;
    type Effect = ListEffect;

    fn step(&self, focus: &mut Focus, input: Input) -> ListEffect {
        let b = self.class.behaviour();
        let (kind, button) = input.event();
        if let Some(hook) = b.hook
            && hook(kind, button) == Button::Handled
        {
            return ListEffect::Hook;
        }
        let row = focus.0 as i16;
        match list_class::route(kind, button, b.action, b.action_buttons, b.sideways) {
            Route::Action => return ListEffect::Action,
            Route::Sideways { right } => {
                if let Some(line) = b.rows.sideways(row, right) {
                    return ListEffect::Stepped(line);
                }
            }
            Route::Controls => {}
        }
        match input {
            Input::Up if focus.0 > 0 => {
                focus.0 -= 1;
                ListEffect::Focus(focus.0)
            }
            Input::Down if focus.0 + 1 < self.count() => {
                focus.0 += 1;
                ListEffect::Focus(focus.0)
            }
            Input::A => match b.rows.press(row) {
                Chosen::Stepped(line) => ListEffect::Stepped(line),
                Chosen::Open(id) | Chosen::Push(id) => ListEffect::Open(id),
                Chosen::Act if b.action => ListEffect::Action,
                Chosen::Retail { line, .. } => ListEffect::Retail(line),
                Chosen::Act | Chosen::Nothing => ListEffect::None,
            },
            Input::B | Input::Back => ListEffect::Close,
            _ => ListEffect::None,
        }
    }

    fn handled(effect: &ListEffect) -> bool {
        *effect != ListEffect::None
    }

    fn spec(&self, focus: &Focus) -> ScreenSpec {
        ScreenSpec {
            focus: focus.0,
            animation: self.class.animation_style(),
            ..(self.spec)()
        }
    }

    /// The first, middle and last row focused.
    fn states(&self) -> Vec<(String, Focus)> {
        let n = (self.spec)().rows.len();
        let mut at = vec![0, n / 2, n.saturating_sub(1)];
        at.dedup();
        at.into_iter()
            .map(|f| (format!("focus-{f}"), Focus(f)))
            .collect()
    }

    fn summary(&self, focus: &Focus) -> String {
        focus_line(&self.spec(focus))
    }
}

// ---- list screens opened on one another ----

/// List screens of ours that open one another ([`ListEffect::Open`] on a
/// screen it has pushes it; B or Back pops it): what the player walks
/// through, so a table can say that B on the second screen comes back to the
/// first with its focus where it was.
///
/// The rows keep their own state (the screen opened second may read what a
/// press on the first chose), so a state is the presses from the first
/// screen opened, and [`Page::spec`] replays them on the rows before it
/// asks the top screen: a named state then shows what the presses chose,
/// whatever was stepped since. That holds for rows whose presses only
/// choose (a press twice is a press once); rows that toggle a store are
/// stepped through their own [`ListPage`].
pub struct StackPage {
    pages: Vec<(&'static str, ListPage)>,
    named: Vec<(String, Vec<Input>)>,
}

/// A [`StackPage`]'s state: the presses so far and the screens they left
/// open, the top last (none: the first screen closed).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stack {
    pub presses: Vec<Input>,
    pub open: Vec<(&'static str, Focus)>,
}

impl StackPage {
    /// `first` opens first; named state `open` (no presses) shows it.
    pub fn new(first: &'static str, page: ListPage) -> Self {
        StackPage {
            pages: vec![(first, page)],
            named: vec![("open".into(), Vec::new())],
        }
    }
    /// A screen an [`ListEffect::Open`] can push.
    pub fn screen(mut self, name: &'static str, page: ListPage) -> Self {
        self.pages.push((name, page));
        self
    }
    /// A named state: `presses` from the first screen.
    pub fn state(mut self, name: &str, presses: &[Input]) -> Self {
        self.named.push((name.into(), presses.to_vec()));
        self
    }
    fn page(&self, name: &str) -> Option<&ListPage> {
        self.pages.iter().find(|(n, _)| *n == name).map(|(_, p)| p)
    }
    fn fresh(&self) -> Stack {
        Stack {
            presses: Vec::new(),
            open: vec![(self.pages[0].0, Focus::default())],
        }
    }
    /// The state after `presses` from the first screen.
    pub fn after(&self, presses: &[Input]) -> Stack {
        let mut s = self.fresh();
        for &i in presses {
            self.step(&mut s, i);
        }
        s
    }
    /// The screen on top, by name; `None` once the first one closed.
    pub fn top(state: &Stack) -> Option<&'static str> {
        state.open.last().map(|(n, _)| *n)
    }
}

impl Page for StackPage {
    type State = Stack;
    type Event = Input;
    type Effect = ListEffect;

    fn step(&self, s: &mut Stack, input: Input) -> ListEffect {
        let Some((name, focus)) = s.open.last_mut() else {
            return ListEffect::None;
        };
        let Some(page) = self.page(name) else {
            return ListEffect::None;
        };
        s.presses.push(input);
        let e = page.step(focus, input);
        match &e {
            ListEffect::Open(next) if self.page(next).is_some() => {
                let next = self.pages.iter().find(|(n, _)| n == next).unwrap().0;
                s.open.push((next, Focus::default()));
            }
            ListEffect::Close => {
                s.open.pop();
            }
            _ => {}
        }
        e
    }

    fn handled(effect: &ListEffect) -> bool {
        *effect != ListEffect::None
    }

    fn spec(&self, s: &Stack) -> ScreenSpec {
        // The rows' own state as the presses left it.
        let replayed = self.after(&s.presses);
        let (name, focus) = replayed
            .open
            .last()
            .copied()
            .unwrap_or((self.pages[0].0, Focus::default()));
        self.page(name).expect("a pushed screen is ours").spec(&focus)
    }

    fn states(&self) -> Vec<(String, Stack)> {
        self.named
            .iter()
            .map(|(n, presses)| (n.clone(), self.after(presses)))
            .collect()
    }

    /// The screen on top and its focus line; `closed` once the first one
    /// closed.
    fn summary(&self, s: &Stack) -> String {
        match Self::top(s) {
            None => "closed".into(),
            Some(name) => format!("{name} | {}", focus_line(&self.spec(s))),
        }
    }
}

// ---- the account screen ----

/// What steps the account screen: the pad, the keyboard's text, the
/// server's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountEvent {
    Press(Input),
    Typed(Option<String>),
    Finished(Result<Outcome, SetupError>),
}

impl From<Input> for AccountEvent {
    fn from(i: Input) -> Self {
        AccountEvent::Press(i)
    }
}

/// The account screen ([`AccountScreen`]) as a page.
pub struct AccountPage;

impl Page for AccountPage {
    type State = AccountScreen;
    type Event = AccountEvent;
    type Effect = account_screen::Effect;

    fn step(&self, s: &mut AccountScreen, event: AccountEvent) -> account_screen::Effect {
        use account_screen::Press;
        match event {
            AccountEvent::Press(i) => match i {
                Input::Up => s.press(Press::Up),
                Input::Down => s.press(Press::Down),
                Input::A => s.press(Press::A),
                Input::B => s.press(Press::B),
                _ => account_screen::Effect::None,
            },
            AccountEvent::Typed(t) => s.typed(t.as_deref()),
            AccountEvent::Finished(r) => s.finished(r),
        }
    }

    fn handled(effect: &account_screen::Effect) -> bool {
        *effect != account_screen::Effect::None
    }

    fn spec(&self, s: &AccountScreen) -> ScreenSpec {
        crate::ui_kit::screens::account_spec(s, 0)
    }

    /// The menu with each row focused.
    fn states(&self) -> Vec<(String, AccountScreen)> {
        (0..account_screen::MENU.len())
            .map(|f| {
                let mut s = AccountScreen::new(None);
                for _ in 0..f {
                    s.press(account_screen::Press::Down);
                }
                (format!("focus-{f}"), s)
            })
            .collect()
    }

    fn summary(&self, s: &AccountScreen) -> String {
        let spec = self.spec(s);
        format!("{} | {}", spec.title, focus_line(&spec))
    }
}

// ---- driven live ----

/// A page and its state behind one type, for a tool that steps any screen
/// (the editor's UI Screens tool, `h2 ui preview`).
pub trait Live {
    /// Steps the pad's `input`: the effect, as text.
    fn press(&mut self, input: Input) -> String;
    /// Probes an input on this disposable offline model. Checks use the
    /// typed effect its route returned. A worker or I/O action returns None.
    fn audit_press(&mut self, input: Input) -> Option<bool>;
    fn spec(&self) -> ScreenSpec;
    fn summary(&self) -> String;
    /// The named states.
    fn state_names(&self) -> Vec<String>;
    /// Every named state's spec: what the preview sheet draws.
    fn specs(&self) -> Vec<(String, ScreenSpec)>;
    /// Goes to the named state; false when there is none.
    fn go(&mut self, name: &str) -> bool;
}

/// [`Live`] over a [`Page`].
pub struct Driven<P: Page> {
    pub page: P,
    pub state: P::State,
}

impl<P: Page> Driven<P> {
    /// `page` in its first named state.
    pub fn new(page: P) -> Self {
        let state = page
            .states()
            .into_iter()
            .next()
            .expect("a page has a state")
            .1;
        Driven { page, state }
    }
}

impl<P: Page> Live for Driven<P> {
    fn press(&mut self, input: Input) -> String {
        format!("{:?}", self.page.step(&mut self.state, input.into()))
    }
    fn audit_press(&mut self, input: Input) -> Option<bool> {
        Some(P::handled(&self.page.step(&mut self.state, input.into())))
    }
    fn spec(&self) -> ScreenSpec {
        self.page.spec(&self.state)
    }
    fn summary(&self) -> String {
        self.page.summary(&self.state)
    }
    fn state_names(&self) -> Vec<String> {
        self.page.states().into_iter().map(|(n, _)| n).collect()
    }
    fn specs(&self) -> Vec<(String, ScreenSpec)> {
        specs(&self.page)
    }
    fn go(&mut self, name: &str) -> bool {
        match self.page.states().into_iter().find(|(n, _)| n == name) {
            Some((_, s)) => {
                self.state = s;
                true
            }
            None => false,
        }
    }
}

/// Every state's spec of `page`, by name: what the preview sheet draws.
pub fn specs<P: Page>(page: &P) -> Vec<(String, ScreenSpec)> {
    page.states()
        .into_iter()
        .map(|(n, s)| {
            let spec = page.spec(&s);
            (n, spec)
        })
        .collect()
}

/// A shared [`ListPage`] spec source over an `Arc` of something described.
pub fn described<T: crate::ui_kit::spec::Describe + 'static>(
    t: &Arc<T>,
) -> impl Fn() -> ScreenSpec + 'static {
    let t = Arc::clone(t);
    move || t.spec()
}

#[cfg(test)]
mod tests;
