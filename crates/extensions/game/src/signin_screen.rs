//! The in-game sign-in prompt (docs/live/live.md, "In-game sign-in"): when
//! the player signs in to Live with an identity whose key for this server
//! is not on this machine, the Live HLE keeps the logon pending
//! ([`live_hle::signin`]) and this screen asks for the 6-digit recovery
//! code, on the controller, in the game.
//!
//! **The screen** is our own list class ([`crate::list_class`]) with one
//! row: its label is the step ("Enter the code", "Checking...") and the
//! description holds the page (who, where the code is, the digits, the
//! last error). The d-pad edits the code: up and down change the digit
//! under the cursor, left and right move it. A (the row's press, or Start)
//! signs in, makes the account when the server doesn't have it, or closes
//! the screen once done; B backs out, which fails the waiting logon.
//!
//! **Text that changes.** The label and description are string ids of
//! ours served from guest buffers ([`TEXT_IDS`]). Each change writes the
//! new text into the other buffer of a pair and switches to its id, so the
//! game never keeps a stale string under the same id. The list's row text
//! (asked every frame while the screen is up) writes it and sets the
//! description again when it changed ([`crate::list_class::Rows::live_text`]).
//!
//! **Opening it.** The logon's own polling opens it: a wrapper around the
//! HLE's `XOnlineTaskContinue` (`0x003a0620`) asks
//! [`live_hle::Hle::take_signin_open`] after each call and, the first time
//! a sign-in waits, opens this screen for that controller with
//! `0x149f1e(1 << controller, loader)`.
//!
//! **A new account** (the app builds' "New Account" identity,
//! [`live_hle::LiveConfig::offer_new_account`]): the page asks for a
//! gamertag, then an invite when the server wants one; A opens the game's
//! own keyboard for it ([`crate::signin_keyboard`]) and the typed text
//! goes to [`Hle::signin_typed`]. A taken gamertag turns the page into
//! the code prompt above.
//!
//! The pure part ([`CodeEntry`], [`page`]) is tested here; the guest side
//! needs a game run (the checklist in docs/live/live.md).

use std::sync::{Arc, Mutex};

use engine::entry::{Cx, EntryError, GuestCall};
use engine::memory::MemFault;
use hooks::{BuildError, Builder, Manifest};
use live_hle::Hle;
use live_hle::signin::{Phase, Prompt};
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, GuestFault, Runtime, TransferAbi, abi};

use crate::account_screen::{LINE_CHARS, layout, recovery_lines, wrap};
use crate::list_class::{self, Action, Button, Buttons, ListClass, LiveText, Rows, TextBlock};
use crate::signin_keyboard::{self as keyboard, Field, Typing};
use crate::ui::{Chosen, LABEL_SLOT};
use crate::ui_kit::dialog::{Cells, DialogSpec};
use crate::ui_kit::input::{Glyph, Prompt as Legend};
use crate::ui_kit::spec::{Describe, RowSpec, ScreenSpec};
use crate::ui_kit::view::Opener;
use live_hle::account_setup::SetupError;
use live_hle::signin::{Call, Typed};

/// The screen id and feature id.
pub const SCREEN: &str = "signin-code";
pub const FEATURE: &str = "signin-code";
/// The code's length.
pub const DIGITS: usize = 6;
/// The HLE's `XOnlineTaskContinue` entry (live_hle::entries).
pub const TASK_CONTINUE: u32 = 0x003a_0620;

/// Our string ids: the title, then the label and the description, each a
/// pair the text alternates between.
pub const ID_BASE: u32 = crate::ui_kit::ids::family("signin").base;
pub const TITLE_ID: u32 = ID_BASE + 0x10;
pub const TEXT_IDS: [u32; 4] = [
    ID_BASE + 0x11,
    ID_BASE + 0x12,
    ID_BASE + 0x13,
    ID_BASE + 0x14,
];
pub const TITLE: &str = "Xbox Live sign-in";
/// Bytes per served string buffer (UTF-8 and its NUL).
pub const BUFFER: u32 = 0x200;
/// The page's buffers: the title, [`TEXT_IDS`]'s four, the keyboard's
/// title and subtitle ([`keyboard::Typing::title`]), then the keyboard's
/// UTF-16 text.
const KEYBOARD_TITLES: u32 = 5;
const KEYBOARD_TEXT: u32 = 7;
const PAGE_BUFFERS: u32 = 8;

/// Button words in a button event (`list_class::BUTTON_EVENT`): A, B.
/// Directions are not button events: the controller update raises them as
/// event kinds 1 to 4 (`list_class::UP_EVENT` and on), from the d-pad or
/// the left stick, again every 250 ms while held.
pub const A_BUTTON: u32 = crate::ui_kit::input::A_BUTTON;
pub const B_BUTTON: u32 = crate::ui_kit::input::B_BUTTON;

/// The description: the right half of the screen, the level select's
/// description font and colour.
pub const TEXT_BLOCK: TextBlock = TextBlock {
    rect: [110, -40, -130, 300],
    font: 1,
    colour: [0.388235, 0.580392, 0.756863],
    animation: 0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
}

impl Press {
    /// An event's kind and button words: a direction (d-pad or stick), or
    /// A or B.
    pub fn from_event(kind: u32, button: u32) -> Option<Press> {
        use crate::ui_kit::input::Input;
        Some(match Input::from_event(kind, button)? {
            Input::Up => Press::Up,
            Input::Down => Press::Down,
            Input::Left => Press::Left,
            Input::Right => Press::Right,
            Input::A => Press::A,
            // Back closes the screen as B does, so it cancels as B does:
            // without it the screen went and the logon stayed pending.
            Input::B | Input::Back => Press::B,
            _ => return None,
        })
    }
}

/// What a press asks of the glue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    /// The page changed.
    Redraw,
    /// Check this code ([`Hle::signin_submit`]).
    Submit(String),
    /// Make the gamertag on the server ([`Hle::signin_create`]).
    Create,
    /// Back out ([`Hle::signin_cancel`]), and close.
    Cancel,
    /// Close the screen ([`Hle::signin_dismiss`]).
    Close,
    /// Open the keyboard for this field.
    Keyboard(Field),
}

/// The code being typed with the d-pad.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeEntry {
    digits: [u8; DIGITS],
    cursor: usize,
    /// The last error is hidden: the code was edited since (it shows
    /// again once a new answer comes, after the next A).
    quiet: bool,
}

impl CodeEntry {
    pub fn code(&self) -> String {
        self.digits.iter().map(|d| char::from(b'0' + d)).collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The code page's error, unless the code was edited since it came.
    pub fn error<'e>(&self, error: &'e Option<SetupError>) -> Option<&'e SetupError> {
        error.as_ref().filter(|_| !self.quiet)
    }

    /// `press` on the page of `phase`.
    pub fn press(&mut self, phase: &Phase, press: Press) -> Effect {
        match (phase, press) {
            (Phase::Code { .. }, Press::Up) => {
                self.digits[self.cursor] = (self.digits[self.cursor] + 1) % 10;
                self.quiet = true;
                Effect::Redraw
            }
            (Phase::Code { .. }, Press::Down) => {
                self.digits[self.cursor] = (self.digits[self.cursor] + 9) % 10;
                self.quiet = true;
                Effect::Redraw
            }
            (Phase::Code { .. }, Press::Left) if self.cursor > 0 => {
                self.cursor -= 1;
                self.quiet = true;
                Effect::Redraw
            }
            (Phase::Code { .. }, Press::Right) if self.cursor + 1 < DIGITS => {
                self.cursor += 1;
                self.quiet = true;
                Effect::Redraw
            }
            (Phase::Code { .. }, Press::A) => {
                self.quiet = false;
                Effect::Submit(self.code())
            }
            (Phase::Gamertag { .. }, Press::A) => Effect::Keyboard(Field::Gamertag),
            (Phase::Invite { .. }, Press::A) => Effect::Keyboard(Field::Invite),
            (Phase::Create { error }, Press::A) if !needs_invite(error) => Effect::Create,
            (p, Press::A | Press::B) if p.finished() => Effect::Close,
            (_, Press::B) => Effect::Cancel,
            _ => Effect::None,
        }
    }

    /// The code as the page shows it: `1 2 3 - 4 5 6`, the digit under the
    /// cursor in brackets.
    pub fn shown(&self) -> String {
        let mut s = String::new();
        for (i, d) in self.digits.iter().enumerate() {
            if i == DIGITS / 2 {
                s.push_str(" -");
            }
            if !s.is_empty() {
                s.push(' ');
            }
            if i == self.cursor {
                s.push_str(&format!("[{d}]"));
            } else {
                s.push(char::from(b'0' + d));
            }
        }
        s
    }
}

fn needs_invite(error: &Option<live_hle::account_setup::SetupError>) -> bool {
    use live_hle::account_setup::SetupError;
    matches!(
        error,
        Some(SetupError::NeedsInvite | SetupError::InviteInvalid)
    )
}

/// The server as the player reads it: no scheme, no trailing slash, at
/// most [`LINE_CHARS`] less the `/dash` that follows.
pub fn host(url: &str) -> String {
    let h = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .trim_end_matches('/');
    h.chars().take(LINE_CHARS - 6).collect()
}

/// The row's label and the description for `prompt` (`None`: nothing
/// waiting), at unix second `now`. The description is at most five lines
/// of 40 characters ([`layout`]).
pub fn page(prompt: Option<&Prompt>, entry: &CodeEntry, now: i64) -> (String, String) {
    let Some(p) = prompt else {
        return (
            "Sign-in closed".into(),
            layout(&["Nothing is waiting to sign in.".into(), "B: back".into()]),
        );
    };
    let host = host(&p.server_url);
    let tag = &p.gamertag;
    let (label, lines): (String, Vec<String>) = match &p.phase {
        Phase::Code { error } => {
            let from = format!("{host}/dash (or your app):");
            let mut l = vec![
                "Enter the 6-digit code from Account on".into(),
                if from.len() <= LINE_CHARS {
                    from
                } else {
                    format!("{host}/dash:")
                },
                entry.shown(),
            ];
            l.extend(match entry.error(error) {
                Some(e) => wrap(&e.to_string(), LINE_CHARS),
                None => vec!["D-pad: digits. A: sign in. B: cancel.".into()],
            });
            (format!("Sign in as {tag}"), l)
        }
        Phase::Gamertag { error } => {
            let mut l = wrap(&format!("A gamertag on {host}."), LINE_CHARS);
            l.push("Taken? Yours signs in with its code.".into());
            l.extend(match error {
                Some(e) => wrap(&e.to_string(), LINE_CHARS),
                None => vec!["A: type it. B: cancel.".into()],
            });
            ("New Xbox Live account".into(), l)
        }
        Phase::Invite { error } => {
            let mut l = wrap(
                &format!("{host} needs an invite to make {tag}."),
                LINE_CHARS,
            );
            l.extend(match error {
                Some(e) => wrap(&e.to_string(), LINE_CHARS),
                None => vec!["A: type the code. B: cancel.".into()],
            });
            ("Invite code".into(), l)
        }
        Phase::Working => (
            "Checking...".into(),
            vec![format!("Checking with {host}..."), "B: cancel".into()],
        ),
        Phase::Create { error } => {
            let mut l = vec![format!("{tag} isn't on {host} yet.")];
            match error {
                Some(e) if needs_invite(&Some(e.clone())) => {
                    l.extend(wrap(&e.to_string(), LINE_CHARS));
                    l.push(format!("Make it at {host}/dash"));
                    l.push("or: h2 play --invite CODE. B: back".into());
                }
                Some(e) => {
                    l.extend(wrap(&e.to_string(), LINE_CHARS));
                    l.push("A: try again. B: cancel.".into());
                }
                None => l.push("A: make it there now. B: cancel.".into()),
            }
            ("Make the account".into(), l)
        }
        Phase::Created { info: Some(info) } => {
            ("Write this down. A: done".into(), recovery_lines(info, now))
        }
        Phase::Created { info: None } => (
            "Signed in. A: done".into(),
            vec![
                format!("{tag} is made on {host}."),
                "Set up a sign-in code on /dash.".into(),
            ],
        ),
        Phase::Done => (
            "Signed in. A: done".into(),
            vec![
                format!("{tag} is on this machine now."),
                "Signing in to Xbox Live.".into(),
            ],
        ),
        Phase::Cancelled => (
            "Cancelled".into(),
            vec!["Sign-in cancelled.".into(), "B: back".into()],
        ),
    };
    (label, layout(&lines))
}

/// A string as the locale blob holds it: UTF-8, NUL-terminated, cut to fit
/// [`BUFFER`] on a character boundary.
pub fn served(text: &str) -> Vec<u8> {
    let mut end = text.len().min(BUFFER as usize - 1);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut v = text.as_bytes()[..end].to_vec();
    v.push(0);
    v
}

/// The page's state between frames: what is shown, under which id.
#[derive(Default)]
struct Shown {
    entry: CodeEntry,
    label: Option<String>,
    body: Option<String>,
    /// Which of each pair is current.
    label_flip: bool,
    body_flip: bool,
}

/// What the class shows: the HLE's prompt with the code being typed.
pub struct SigninRows {
    hle: Arc<Hle>,
    /// The page of served strings ([`TEXT_IDS`], [`TITLE_ID`]).
    page: Arc<std::sync::OnceLock<u32>>,
    shown: Mutex<Shown>,
    /// The keyboard opened for the page's gamertag or invite.
    typing: Typing,
}

impl SigninRows {
    pub fn new(hle: Arc<Hle>) -> Self {
        SigninRows {
            hle,
            page: Arc::default(),
            shown: Mutex::default(),
            typing: Typing::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shown> {
        self.shown.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A press on the screen: what the HLE is told, and whether the screen
    /// closes.
    pub fn press(&self, press: Press) -> Effect {
        let prompt = self.hle.signin();
        let phase = prompt.map_or(Phase::Cancelled, |p| p.phase);
        let effect = self.lock().entry.press(&phase, press);
        match &effect {
            Effect::Submit(code) => {
                self.hle.signin_submit(code);
            }
            Effect::Create => {
                self.hle.signin_create();
            }
            Effect::Cancel => {
                self.hle.signin_cancel();
                self.hle.signin_dismiss();
            }
            Effect::Close => self.hle.signin_dismiss(),
            Effect::None | Effect::Redraw | Effect::Keyboard(_) => {}
        }
        effect
    }

    /// The keyboard's text buffer, once the page is in.
    pub fn keyboard_buffer(&self) -> Option<u32> {
        self.page.get().map(|base| base + BUFFER * KEYBOARD_TEXT)
    }

    /// Our string ids' addresses in the page at `base`, with the retail
    /// keyboard's titles while one of ours is open.
    pub fn string(&self, base: u32, id: u32) -> Option<u32> {
        string_at(base, id).or_else(|| {
            self.typing
                .title(id)
                .map(|i| base + BUFFER * (KEYBOARD_TITLES + i as u32))
        })
    }

    /// The keyboard's text once it closed, to the HLE.
    fn poll_keyboard(&self, m: &dyn engine::memory::GuestMemory) -> Result<(), MemFault> {
        let Some(buffer) = self.keyboard_buffer() else {
            return Ok(());
        };
        if let Some((field, text)) = self.typing.poll(m, buffer)? {
            eprintln!(
                "signin prompt: the {field:?} keyboard closed{}",
                if text.is_some() {
                    " with text"
                } else {
                    " empty"
                }
            );
            self.hle.signin_typed(text.as_deref());
        }
        Ok(())
    }

    fn ids(&self) -> (u32, u32) {
        let s = self.lock();
        (
            TEXT_IDS[usize::from(s.label_flip)],
            TEXT_IDS[2 + usize::from(s.body_flip)],
        )
    }
}

impl Rows for SigninRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(crate::ui_kit::spec::Describe::spec(self))
    }

    fn title(&self) -> u32 {
        TITLE_ID
    }
    fn count(&self) -> usize {
        1
    }
    fn texts(&self, _row: i16) -> Vec<(u32, u32)> {
        vec![(LABEL_SLOT, self.ids().0)]
    }
    fn press(&self, _row: i16) -> Chosen {
        Chosen::Act
    }
    fn description(&self, _row: i16) -> Option<u32> {
        Some(self.ids().1)
    }
    fn live_text(&self) -> LiveText {
        let Some(&base) = self.page.get() else {
            return LiveText::default();
        };
        let prompt = self.hle.signin();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let mut s = self.lock();
        let (label, body) = page(prompt.as_ref(), &s.entry, now);
        let mut live = LiveText::default();
        if s.label.as_deref() != Some(label.as_str()) {
            s.label_flip = !s.label_flip;
            let id = usize::from(s.label_flip);
            live.writes
                .push((base + BUFFER * (1 + id as u32), served(&label)));
            s.label = Some(label);
        }
        if s.body.as_deref() != Some(body.as_str()) {
            s.body_flip = !s.body_flip;
            let id = 2 + usize::from(s.body_flip);
            live.writes
                .push((base + BUFFER * (1 + id as u32), served(&body)));
            s.body = Some(body);
            live.redraw = true;
        }
        live
    }
    fn write_texts(&self, m: &mut dyn engine::memory::GuestMemory) -> Result<(), MemFault> {
        self.poll_keyboard(m)
    }
}

/// The prompt as a modal over `opener`, the screen the sign-in started
/// from (docs/game/ui-builder.md "Screen kinds"): its title, body, code
/// cells, status line and legend. `call` is the server call a
/// [`Phase::Working`] page waits on: the code stays in its cells while it
/// is checked.
pub fn dialog_for(
    prompt: Option<&Prompt>,
    entry: &CodeEntry,
    call: Option<Call>,
    now: i64,
    opener: Opener,
) -> (String, Vec<Legend>, DialogSpec) {
    let a = |t: &str| Legend::new(&[Glyph::A], t);
    let b = |t: &str| Legend::new(&[Glyph::B], t);
    let cells = |cursor: Option<usize>| Cells {
        text: entry.code().chars().map(String::from).collect(),
        cursor,
        gap_after: Some(DIGITS / 2 - 1),
    };
    let dialog = |body: String, cells: Option<Cells>, status: Option<String>| DialogSpec {
        body,
        cells,
        status,
        opener,
        ..DialogSpec::new("", opener)
    };
    let Some(p) = prompt else {
        return (
            "Sign-in closed".into(),
            vec![b("Back")],
            dialog("Nothing is waiting to sign in.".into(), None, None),
        );
    };
    let host = host(&p.server_url);
    let tag = &p.gamertag;
    let error = |e: &Option<SetupError>| e.as_ref().map(ToString::to_string);
    match &p.phase {
        Phase::Code { error } => (
            format!("Sign in as {tag}"),
            vec![
                Legend::new(&[Glyph::DpadUp, Glyph::DpadDown], "Digit"),
                Legend::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Move"),
                a("Sign in"),
                b("Cancel"),
            ],
            dialog(
                format!("Enter the 6-digit code from Account on {host}/dash, or your app."),
                Some(cells(Some(entry.cursor()))),
                entry.error(error).map(ToString::to_string),
            ),
        ),
        Phase::Working => {
            let (title, body) = match call {
                Some(Call::Create) => (
                    "Make the account".into(),
                    format!("Making {tag} on {host}."),
                ),
                Some(Call::Add { .. }) => (
                    "New Xbox Live account".into(),
                    format!("Making {tag} on {host}."),
                ),
                Some(Call::Code) | None => (
                    format!("Sign in as {tag}"),
                    format!("Checking the code with {host}."),
                ),
            };
            (
                title,
                vec![b("Cancel")],
                dialog(
                    body,
                    (call == Some(Call::Code)).then(|| cells(None)),
                    Some("Checking...".into()),
                ),
            )
        }
        Phase::Gamertag { error: e } => (
            "New Xbox Live account".into(),
            vec![a("Type it"), b("Cancel")],
            dialog(
                format!("Pick a gamertag for {host}. Taken? Yours signs in with its code instead."),
                None,
                error(e),
            ),
        ),
        Phase::Invite { error: e } => (
            "Invite code".into(),
            vec![a("Type it"), b("Cancel")],
            dialog(
                format!("{host} needs an invite to make {tag}."),
                None,
                error(e),
            ),
        ),
        Phase::Create { error: e } if needs_invite(e) => (
            "Make the account".into(),
            vec![b("Back")],
            dialog(
                format!(
                    "{tag} isn't on {host} yet. Make it at {host}/dash, or play with h2 play --invite CODE."
                ),
                None,
                error(e),
            ),
        ),
        Phase::Create { error: e } => (
            "Make the account".into(),
            vec![
                a(if e.is_some() { "Try again" } else { "Make it" }),
                b("Cancel"),
            ],
            dialog(format!("{tag} isn't on {host} yet."), None, error(e)),
        ),
        Phase::Created { info: Some(info) } => (
            "Write this down".into(),
            vec![a("Done")],
            dialog(
                crate::account_screen::recovery_lines(info, now).join("\n"),
                None,
                None,
            ),
        ),
        Phase::Created { info: None } => (
            "Signed in".into(),
            vec![a("Done")],
            dialog(
                format!("{tag} is made on {host}. Set up a sign-in code on /dash."),
                None,
                None,
            ),
        ),
        Phase::Done => (
            "Signed in".into(),
            vec![a("Done")],
            dialog(
                format!("{tag} is on this machine now. Signing in to Xbox Live."),
                None,
                None,
            ),
        ),
        Phase::Cancelled => (
            "Cancelled".into(),
            vec![b("Back")],
            dialog("Sign-in cancelled.".into(), None, None),
        ),
    }
}

/// The prompt as the kit describes it (docs/game/ui-screens.md): one row,
/// the description, the d-pad as digits and cursor (the list the game
/// shows today), and the modal it becomes ([`dialog_for`]), which the
/// preview draws.
pub fn spec_for(
    prompt: Option<&Prompt>,
    entry: &CodeEntry,
    call: Option<Call>,
    now: i64,
    opener: Opener,
) -> ScreenSpec {
    use crate::ui_kit::input::Input;
    let (label, body) = page(prompt, entry, now);
    let mut s = ScreenSpec::new(SCREEN, TITLE);
    s.ids = vec![TITLE_ID];
    s.description = Some(TEXT_BLOCK.into());
    s.inputs = vec![
        Input::Up,
        Input::Down,
        Input::Left,
        Input::Right,
        Input::A,
        Input::B,
        Input::Start,
    ];
    s.rows = vec![
        RowSpec::action(&label)
            .describe(&body.replace("\r\n", "\n"))
            .ids(&[TEXT_IDS[0], TEXT_IDS[2]]),
    ];
    let (title, legend, dialog) = dialog_for(prompt, entry, call, now, opener);
    s.title = title;
    s.prompts = legend;
    s.dialog = Some(dialog);
    s
}

/// What steps the sign-in page: the pad, the keyboard's text (`None`: backed
/// out of), the server's answer to the call running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SigninEvent {
    Press(crate::ui_kit::input::Input),
    Typed(Option<String>),
    Answer(Result<Option<live_proto::recovery::RecoveryInfo>, SetupError>),
}

impl From<crate::ui_kit::input::Input> for SigninEvent {
    fn from(i: crate::ui_kit::input::Input) -> Self {
        SigninEvent::Press(i)
    }
}

/// The prompt and the code being typed, with the server call running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigninState {
    pub prompt: Prompt,
    pub entry: CodeEntry,
    /// The call an [`SigninEvent::Answer`] answers.
    pub call: Option<Call>,
    /// The screen the sign-in started from, which the prompt sits over.
    pub opener: Opener,
}

/// The sign-in prompt as a pure page (docs/game/ui-builder.md
/// "Behaviour"): a press goes through [`CodeEntry::press`] as on the
/// screen, and what the HLE does with the effect, typed text and the
/// server's answers follow [`live_hle::signin`]'s own transitions
/// ([`live_hle::signin::typed`], [`live_hle::signin::refused`],
/// [`live_hle::signin::settled`]). A and Start reach the row's action
/// ([`ACTION_BUTTONS`]); X does nothing; B and Back cancel.
pub struct SigninPage;

impl SigninPage {
    fn start(state: &mut SigninState, call: Call) {
        state.prompt.phase = Phase::Working;
        state.call = Some(call);
    }
}

impl crate::ui_kit::page::Page for SigninPage {
    type State = SigninState;
    type Event = SigninEvent;
    type Effect = Effect;

    fn step(&self, state: &mut SigninState, event: SigninEvent) -> Effect {
        use crate::ui_kit::input::Input;
        match event {
            SigninEvent::Press(input) => {
                let press = match input {
                    // Start submits as A does; X is nothing here.
                    Input::Start => Some(Press::A),
                    i => Press::from_event(i.event().0, i.event().1),
                };
                let Some(press) = press else {
                    return Effect::None;
                };
                let effect = state.entry.press(&state.prompt.phase, press);
                match &effect {
                    Effect::Submit(code) => match live_hle::signin::refused(code) {
                        Some(p) => state.prompt.phase = p,
                        None => Self::start(state, Call::Code),
                    },
                    Effect::Create => Self::start(state, Call::Create),
                    Effect::Cancel if !state.prompt.phase.finished() => {
                        state.prompt.phase = Phase::Cancelled;
                    }
                    _ => {}
                }
                effect
            }
            SigninEvent::Typed(text) => {
                match live_hle::signin::typed(&state.prompt.phase, text.as_deref()) {
                    None | Some(Typed::Keep) => Effect::None,
                    Some(Typed::Phase(p)) => {
                        state.prompt.phase = p;
                        Effect::Redraw
                    }
                    Some(Typed::Gamertag(tag)) => {
                        state.prompt.gamertag = tag;
                        Self::start(state, Call::Add { invite: false });
                        Effect::Redraw
                    }
                    Some(Typed::Invite(_)) => {
                        Self::start(state, Call::Add { invite: true });
                        Effect::Redraw
                    }
                }
            }
            SigninEvent::Answer(result) => {
                let Some(call) = state.call.take() else {
                    return Effect::None;
                };
                if state.prompt.phase == Phase::Cancelled {
                    return Effect::None;
                }
                let p = &state.prompt;
                state.prompt.phase =
                    live_hle::signin::settled(call, result, &p.gamertag, &p.server_url);
                Effect::Redraw
            }
        }
    }

    fn handled(effect: &Effect) -> bool {
        *effect != Effect::None
    }

    fn spec(&self, state: &SigninState) -> ScreenSpec {
        spec_for(
            Some(&state.prompt),
            &state.entry,
            state.call,
            0,
            state.opener,
        )
    }

    /// The code page empty, typed and refused, checking, the new-account
    /// pages, and the end states, over the main menu; then the code page
    /// and a refused code over the Xbox Live menu.
    fn states(&self) -> Vec<(String, SigninState)> {
        let at = |phase: Phase, entry: &CodeEntry| SigninState {
            prompt: Prompt {
                gamertag: "Chief".into(),
                server_url: "https://live.example.net/".into(),
                controller: 0,
                phase,
            },
            entry: entry.clone(),
            call: None,
            opener: Opener::MAIN_MENU,
        };
        let empty = CodeEntry::default();
        let mut typed = CodeEntry::default();
        for press in [
            Press::Up,
            Press::Up,
            Press::Right,
            Press::Down,
            Press::Right,
        ] {
            typed.press(&Phase::Code { error: None }, press);
        }
        // Typed, then answered: the error shows.
        typed.quiet = false;
        let code = |error| Phase::Code { error };
        let mut checking = at(Phase::Working, &typed);
        checking.call = Some(Call::Code);
        let mut states = vec![
            ("code", at(code(None), &empty)),
            ("code-typed", at(code(None), &typed)),
            (
                "code-wrong",
                at(code(Some(SetupError::WrongCodeLeft { left: 2 })), &typed),
            ),
            (
                "code-unreachable",
                at(code(Some(SetupError::Unreachable)), &typed),
            ),
            ("checking", checking),
            ("gamertag", at(Phase::Gamertag { error: None }, &empty)),
            (
                "gamertag-bad",
                at(
                    Phase::Gamertag {
                        error: Some(SetupError::BadGamertag),
                    },
                    &empty,
                ),
            ),
            ("invite", at(Phase::Invite { error: None }, &empty)),
            ("create", at(Phase::Create { error: None }, &empty)),
            ("done", at(Phase::Done, &empty)),
            ("cancelled", at(Phase::Cancelled, &empty)),
        ];
        // The same prompt over the other screen a sign-in starts from.
        let live_menu = |mut s: SigninState| {
            s.opener = Opener::XBOX_LIVE_MENU;
            s
        };
        let wrong = states[2].1.clone();
        states.push(("code-over-live-menu", live_menu(at(code(None), &empty))));
        states.push(("code-wrong-over-live-menu", live_menu(wrong)));
        // A new account's prompt has no gamertag yet.
        for (name, s) in &mut states {
            if name.starts_with("gamertag") {
                s.prompt.gamertag.clear();
            }
        }
        states
            .into_iter()
            .map(|(n, s)| (n.to_string(), s))
            .collect()
    }

    /// The page's label, the code as shown, and the error shown (or `-`).
    fn summary(&self, state: &SigninState) -> String {
        let (label, _) = page(Some(&state.prompt), &state.entry, 0);
        let error = match &state.prompt.phase {
            Phase::Code { error } => state.entry.error(error).map(ToString::to_string),
            Phase::Gamertag { error } | Phase::Invite { error } | Phase::Create { error } => {
                error.as_ref().map(ToString::to_string)
            }
            _ => None,
        };
        format!(
            "{label} | {} | {}",
            state.entry.shown(),
            error.as_deref().unwrap_or("-")
        )
    }
}

impl Describe for SigninRows {
    fn spec(&self) -> ScreenSpec {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let entry = self.lock().entry.clone();
        spec_for(
            self.hle.signin().as_ref(),
            &entry,
            None,
            now,
            Opener::MAIN_MENU,
        )
    }
}

/// Our string ids' addresses in the page at `base`: the title first, then
/// [`TEXT_IDS`] in order.
pub fn string_at(base: u32, id: u32) -> Option<u32> {
    if id == TITLE_ID {
        return Some(base);
    }
    TEXT_IDS
        .iter()
        .position(|t| *t == id)
        .map(|i| base + BUFFER * (1 + i as u32))
}

/// The class's action buttons besides A: only Start, as the legend says.
/// The class default takes X too, and a stray X would send a half-typed
/// code.
pub const ACTION_BUTTONS: [u32; 1] = [list_class::START_BUTTON];

/// The class: one row, the description, our buttons, A as its action.
pub fn class(rows: Arc<SigninRows>, block: TextBlock) -> ListClass {
    let for_buttons = Arc::clone(&rows);
    let buttons: Buttons = Arc::new(move |kind, button| {
        let press = Press::from_event(kind, button);
        eprintln!("signin prompt: event {kind}, button {button} ({press:?})");
        match press {
            // Every direction is ours: up and down change the digit rather
            // than move the one-row list, left and right move the cursor.
            Some(p @ (Press::Up | Press::Down | Press::Left | Press::Right)) => {
                for_buttons.press(p);
                Button::Handled
            }
            // B closes the screen as on any screen; the HLE is told first.
            // A goes on to the list, whose row press runs the action.
            Some(Press::B) => {
                for_buttons.press(Press::B);
                Button::Pass
            }
            _ => Button::Pass,
        }
    });
    let for_action = Arc::clone(&rows);
    let action: Action = Arc::new(move |cx, screen| {
        let rows = Arc::clone(&for_action);
        Box::pin(async move { act(&cx, &rows, screen).await })
    });
    ListClass::with_rows(FEATURE, SCREEN, rows as Arc<dyn Rows>)
        .description(block)
        .action(action)
        .action_buttons(&ACTION_BUTTONS)
        .buttons(buttons)
}

/// A on the row (or Start, never X): the press, then closing the screen when it is
/// done with.
async fn act(cx: &Cx, rows: &SigninRows, screen: u32) -> Result<(), EntryError> {
    let effect = rows.press(Press::A);
    if let Effect::Keyboard(field) = effect {
        return open_keyboard(cx, rows, field).await;
    }
    if effect == Effect::Close {
        eprintln!("signin prompt: closing");
        crate::ui_kit::lifecycle::close(cx, screen).await?;
    }
    Ok(())
}

/// Opens the game's keyboard for `field` on the page's empty text buffer,
/// for the signing-in controller ([`crate::signin_keyboard`]).
pub async fn open_keyboard(cx: &Cx, rows: &SigninRows, field: Field) -> Result<(), EntryError> {
    let (Some(&base), Some(buffer)) = (rows.page.get(), rows.keyboard_buffer()) else {
        eprintln!("signin prompt: no text page; the keyboard is not opened");
        return Ok(());
    };
    let controller = rows.hle.signin().map_or(0, |p| p.controller);
    let [title, subtitle] = field.titles();
    cx.write(|m| -> Result<(), MemFault> {
        m.write(base + BUFFER * KEYBOARD_TITLES, &served(title))?;
        m.write(base + BUFFER * (KEYBOARD_TITLES + 1), &served(subtitle))?;
        m.write(buffer, &keyboard::units(""))
    })?;
    rows.typing.start(field);
    eprintln!(
        "signin prompt: opening the keyboard for the {field:?} (context {}, controller {controller})",
        field.context()
    );
    cx.call(GuestCall::new(keyboard::OPEN).ecx(controller).args([
        field.context(),
        buffer,
        field.max_chars() + 1,
    ]))
    .await?;
    Ok(())
}

/// Why `signin-strings` can't be snapshotted now: a keyboard of ours is
/// open, so the lookup serves our titles for retail ids and the text read
/// back goes to a prompt only the host knows. Otherwise the page is
/// allocated at build and every id's address is fixed.
pub fn snapshot_live(typing: &Typing) -> Option<String> {
    typing
        .field()
        .map(|f| format!("the sign-in keyboard for the {f:?} is open"))
}

/// The served strings: a page of ours the locale lookup reads.
pub struct Strings(pub Arc<SigninRows>);

impl hooks::Feature for Strings {
    fn manifest(&self) -> Manifest {
        Manifest::new("signin-strings", "The sign-in prompt's text").doc("docs/live/live.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let base = b.guest_alloc(BUFFER * PAGE_BUFFERS, Permissions::RW)?;
        self.0.page.set(base).ok();
        let rows = Arc::clone(&self.0);
        b.locale()
            .source("signin text", move |id| rows.string(base, id));
        let rows = Arc::clone(&self.0);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            snapshot_live(&rows.typing)
        }));
        Ok(())
    }
}

/// The window the prompt opens in: the dialogs' (window kind 1, as the
/// game's own dialogs open) with `H2_SIGNIN_DIALOG=1`, else the main
/// menu's, as `0x149f1e` opens a screen. Off until a game run settles it
/// (docs/game/ui-builder.md "The game run").
pub fn window(env: Option<&str>) -> u32 {
    use crate::ui_kit::dialog_tag::{DIALOG_WINDOW, MAIN_WINDOW};
    match env.map(str::trim) {
        Some("1" | "on" | "true" | "yes") => DIALOG_WINDOW,
        _ => MAIN_WINDOW,
    }
}

/// Registers the screen and its strings and wraps the logon's polling so a
/// waiting sign-in opens it. Nothing without Live or a server.
pub fn install(
    rt: &Runtime,
    registry: &mut hooks::Registry,
    hle: Option<&Arc<Hle>>,
) -> anyhow::Result<bool> {
    let Some(hle) = hle.filter(|h| h.config().server_url.is_some()) else {
        return Ok(false);
    };
    let rows = Arc::new(SigninRows::new(Arc::clone(hle)));
    registry
        .add(&Strings(Arc::clone(&rows)))
        .map_err(|e| anyhow::anyhow!("signin strings: {e}"))?;
    match registry.add(&class(Arc::clone(&rows), TEXT_BLOCK)) {
        Ok(true) => {}
        Ok(false) => crate::say_once(format!("signin prompt: features.{FEATURE} is off")),
        Err(e) => eprintln!("signin prompt: {e}"),
    }
    let window = window(std::env::var("H2_SIGNIN_DIALOG").ok().as_deref());
    wrap_continue(rt, rows, window)
}

/// Wraps the HLE's `XOnlineTaskContinue`: after each call, a sign-in that
/// has just started waiting opens the prompt for its controller.
fn wrap_continue(rt: &Runtime, rows: Arc<SigninRows>, window: u32) -> anyhow::Result<bool> {
    let hle = Arc::clone(&rows.hle);
    let Some(inner) = rt.edit_dispatch(|d| d.override_handler(TASK_CONTINUE)) else {
        crate::say_once(format!("signin prompt: no Live entry at {TASK_CONTINUE:#x}; not wrapped"));
        return Ok(false);
    };
    rt.edit_dispatch(|d| {
        d.register_override(TASK_CONTINUE, "signin prompt: XOnlineTaskContinue", move |cpu, ctx, _| {
            let done = inner.invoke(cpu, ctx, TASK_CONTINUE);
            let Some(controller) = hle.take_signin_open() else {
                return Ok(done);
            };
            match list_class::loader_of(SCREEN) {
                Some(loader) if done.kind == abi::RETURN && controller < 4 => {
                    eprintln!(
                        "signin prompt: opening for controller {controller} (window kind {window})"
                    );
                    open_after(cpu, ctx, done, &rows, 1 << controller, loader, window)
                }
                _ => {
                    eprintln!("signin prompt: its screen is off; the sign-in is cancelled");
                    hle.signin_cancel();
                    Ok(done)
                }
            }
        });
    });
    Ok(true)
}

/// After the wrapped call returned (`done`, EAX its result): opens the
/// screen in `window`, then returns as `done` with EAX restored. The main
/// menu's window is `0x149f1e(mask, loader)` (stdcall, `ret 8`); another
/// (the dialogs') is a request of that kind on the stack handed to the
/// loader (stdcall, `ret 4`), as the game's dialogs (`0x19b527`) do.
fn open_after(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    done: TransferAbi,
    rows: &SigninRows,
    mask: u32,
    loader: u32,
    window: u32,
) -> Result<TransferAbi, GuestFault> {
    let saved = *cpu;
    // The title and the first page are in place before the screen's pane
    // is set up (row text keeps them current after).
    let first = rows.live_text();
    let rt = Arc::clone(ctx.runtime());
    let mut m = rt.memory();
    let title = rows.page.get().map(|&base| (base, served(TITLE)));
    for (at, bytes) in title.iter().chain(&first.writes) {
        if let Err(e) = m.write_bytes(*at, bytes, 1) {
            eprintln!("signin prompt: its text page is not mapped ({e:?}); opening without text");
            break;
        }
    }
    use crate::ui_kit::dialog_tag::{MAIN_WINDOW, REQUEST_SIZE, request};
    let target = if window == MAIN_WINDOW {
        cpu.esp -= 8;
        m.set_u32(cpu.esp, mask)?;
        m.set_u32(cpu.esp + 4, loader)?;
        engine::points::OPEN_SCREEN.base
    } else {
        cpu.esp -= REQUEST_SIZE + 4;
        let at = cpu.esp + 4;
        m.write_bytes(at, &request(window, mask as u16, loader), 1)?;
        m.set_u32(cpu.esp, at)?;
        loader
    };
    ctx.call_guest(cpu, &mut m, target, move |cpu, _| {
        *cpu = saved;
        Ok(done)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_hle::account_setup::SetupError;

    fn prompt(phase: Phase) -> Prompt {
        Prompt {
            gamertag: "Roamer".into(),
            server_url: "https://live.forbes.red".into(),
            controller: 0,
            phase,
        }
    }

    fn test_hle() -> Arc<Hle> {
        Hle::with_address(
            live_hle::LiveConfig::default(),
            "127.0.0.1:3074".parse().unwrap(),
        )
        .unwrap()
    }

    fn code() -> Phase {
        Phase::Code { error: None }
    }

    #[test]
    fn the_d_pad_types_the_code_and_a_submits_it() {
        let mut e = CodeEntry::default();
        assert_eq!(e.shown(), "[0] 0 0 - 0 0 0");
        let presses = [
            Press::Up,
            Press::Right,
            Press::Up,
            Press::Up,
            Press::Right,
            Press::Down,
            Press::Right,
            Press::Right,
            Press::Right,
            Press::Right, // past the end: stays
            Press::Up,
        ];
        for p in presses {
            e.press(&code(), p);
        }
        assert_eq!(e.code(), "129001");
        assert_eq!(e.shown(), "1 2 9 - 0 0 [1]");
        assert_eq!(e.press(&code(), Press::Right), Effect::None);
        for _ in 0..9 {
            e.press(&code(), Press::Left);
        }
        assert_eq!(e.cursor(), 0, "stops at the first digit");
        assert_eq!(e.press(&code(), Press::Down), Effect::Redraw);
        assert_eq!(e.code(), "029001", "wraps 1 -> 0");
        e.press(&code(), Press::Down);
        assert_eq!(e.code(), "929001", "and 0 -> 9");
        assert_eq!(e.press(&code(), Press::A), Effect::Submit("929001".into()));
        assert_eq!(e.press(&code(), Press::B), Effect::Cancel);
    }

    #[test]
    fn each_step_answers_its_own_presses() {
        let mut e = CodeEntry::default();
        assert_eq!(e.press(&Phase::Working, Press::Up), Effect::None);
        assert_eq!(e.press(&Phase::Working, Press::A), Effect::None);
        assert_eq!(e.press(&Phase::Working, Press::B), Effect::Cancel);
        let create = Phase::Create { error: None };
        assert_eq!(e.press(&create, Press::A), Effect::Create);
        assert_eq!(e.press(&create, Press::Up), Effect::None);
        let invite = Phase::Create {
            error: Some(SetupError::NeedsInvite),
        };
        assert_eq!(
            e.press(&invite, Press::A),
            Effect::None,
            "no invite to type"
        );
        assert_eq!(e.press(&invite, Press::B), Effect::Cancel);
        for done in [Phase::Done, Phase::Created { info: None }, Phase::Cancelled] {
            assert_eq!(e.press(&done, Press::A), Effect::Close);
            assert_eq!(e.press(&done, Press::B), Effect::Close);
        }
        assert_eq!(e.code(), "000000", "nothing typed off the code page");
        // A new account's text is typed on the keyboard.
        let tag = Phase::Gamertag { error: None };
        assert_eq!(e.press(&tag, Press::A), Effect::Keyboard(Field::Gamertag));
        assert_eq!(e.press(&tag, Press::Up), Effect::None);
        assert_eq!(e.press(&tag, Press::B), Effect::Cancel);
        let invite = Phase::Invite {
            error: Some(SetupError::InviteInvalid),
        };
        assert_eq!(e.press(&invite, Press::A), Effect::Keyboard(Field::Invite));
        assert_eq!(e.press(&invite, Press::B), Effect::Cancel);
        assert_eq!(e.code(), "000000");
    }

    #[test]
    fn the_new_account_pages_ask_for_a_gamertag_then_an_invite() {
        let e = CodeEntry::default();
        let (label, body) = page(Some(&prompt(Phase::Gamertag { error: None })), &e, 0);
        assert_eq!(label, "New Xbox Live account");
        assert!(body.contains("live.forbes.red"), "{body}");
        assert!(body.contains("A: type it"), "{body}");
        let bad = Phase::Gamertag {
            error: Some(SetupError::BadGamertag),
        };
        let (_, body) = page(Some(&prompt(bad)), &e, 0);
        assert!(!body.contains("A: type it"), "the error instead: {body}");
        let (label, body) = page(Some(&prompt(Phase::Invite { error: None })), &e, 0);
        assert_eq!(label, "Invite code");
        assert!(body.contains("needs an invite"), "{body}");
    }

    #[test]
    fn the_keyboard_titles_are_served_only_while_it_is_open() {
        let rows = SigninRows::new(test_hle());
        let base = 0x0010_0000;
        assert_eq!(rows.string(base, TITLE_ID), Some(base));
        assert_eq!(rows.string(base, keyboard::GAMERTAG_TITLE_IDS[0]), None);
        rows.typing.start(Field::Gamertag);
        assert_eq!(
            rows.string(base, keyboard::GAMERTAG_TITLE_IDS[1]),
            Some(base + BUFFER * (KEYBOARD_TITLES + 1))
        );
        assert_eq!(rows.string(base, keyboard::INVITE_TITLE_ID), None);
    }

    /// The code page as a dialog: six cells in two halves, the cursor's
    /// lit, the error in the status line, and a legend of A and B with no
    /// X; checking keeps the code in its cells with no cursor.
    #[test]
    fn the_pages_are_dialogs() {
        use crate::ui_kit::input::Glyph;
        let mut entry = CodeEntry::default();
        let code = Phase::Code {
            error: Some(SetupError::WrongCodeLeft { left: 2 }),
        };
        entry.press(&code, Press::Up);
        entry.press(&code, Press::Right);
        entry.quiet = false;
        let at = |phase: Phase| Prompt {
            gamertag: "Chief".into(),
            server_url: "https://live.example.net/".into(),
            controller: 0,
            phase,
        };
        let (title, legend, d) =
            dialog_for(Some(&at(code.clone())), &entry, None, 0, Opener::MAIN_MENU);
        assert_eq!(title, "Sign in as Chief");
        let c = d.cells.as_ref().unwrap();
        assert_eq!(c.text, ["1", "0", "0", "0", "0", "0"]);
        assert_eq!((c.cursor, c.gap_after), (Some(1), Some(2)));
        assert_eq!(d.status.as_deref(), Some("Wrong code. 2 tries left."));
        let glyphs: Vec<Glyph> = legend.iter().flat_map(|p| p.glyphs.clone()).collect();
        assert!(glyphs.contains(&Glyph::A) && glyphs.contains(&Glyph::B));
        assert!(!glyphs.contains(&Glyph::X));

        let (_, legend, d) = dialog_for(
            Some(&at(Phase::Working)),
            &entry,
            Some(Call::Code),
            0,
            Opener::MAIN_MENU,
        );
        assert_eq!(d.cells.unwrap().cursor, None);
        assert_eq!(d.status.as_deref(), Some("Checking..."));
        assert_eq!(legend.len(), 1, "only B while checking");

        let (title, _, d) = dialog_for(
            Some(&at(Phase::Gamertag { error: None })),
            &entry,
            None,
            0,
            Opener::MAIN_MENU,
        );
        assert_eq!(title, "New Xbox Live account");
        assert!(d.cells.is_none());
        let s = spec_for(Some(&at(Phase::Done)), &entry, None, 0, Opener::MAIN_MENU);
        assert!(s.dialog.is_some());
        assert_eq!(
            s.rows.len(),
            1,
            "the list the game shows today keeps its row"
        );
    }

    /// On the class, Start runs the action as A does; X is not an action
    /// button, so a stray X can't send a half-typed code.
    #[test]
    fn only_a_and_start_submit_on_the_class() {
        use list_class::{BUTTON_EVENT, Route, START_BUTTON, X_BUTTON, route};
        let c = class(Arc::new(SigninRows::new(test_hle())), TEXT_BLOCK);
        let b = c.behaviour();
        let at = |button| route(BUTTON_EVENT, button, b.action, b.action_buttons, b.sideways);
        assert!(matches!(at(START_BUTTON), Route::Action));
        assert!(matches!(at(X_BUTTON), Route::Controls));
    }

    #[test]
    fn the_buttons_map_to_presses() {
        use list_class::{BUTTON_EVENT, DOWN_EVENT, LEFT_EVENT, RIGHT_EVENT, UP_EVENT};
        // Directions by kind, whatever the button word (d-pad 8 to 11 or
        // the stick's 16).
        assert_eq!(Press::from_event(UP_EVENT, 8), Some(Press::Up));
        assert_eq!(Press::from_event(DOWN_EVENT, 16), Some(Press::Down));
        assert_eq!(Press::from_event(LEFT_EVENT, 10), Some(Press::Left));
        assert_eq!(Press::from_event(RIGHT_EVENT, 11), Some(Press::Right));
        assert_eq!(Press::from_event(BUTTON_EVENT, B_BUTTON), Some(Press::B));
        assert_eq!(Press::from_event(BUTTON_EVENT, A_BUTTON), Some(Press::A));
        // A d-pad button word in a button event is not a direction.
        assert_eq!(Press::from_event(BUTTON_EVENT, 8), None);
        assert_eq!(
            Press::from_event(BUTTON_EVENT, list_class::START_BUTTON),
            None
        );
    }

    #[test]
    fn the_code_page_says_who_where_and_what_went_wrong() {
        let e = CodeEntry::default();
        let (label, body) = page(Some(&prompt(code())), &e, 0);
        assert_eq!(label, "Sign in as Roamer");
        assert_eq!(
            body,
            "Enter the 6-digit code from Account on\r\nlive.forbes.red/dash (or your app):\r\n[0] 0 0 - 0 0 0\r\nD-pad: digits. A: sign in. B: cancel."
        );
        let wrong = prompt(Phase::Code {
            error: Some(SetupError::WrongCodeLeft { left: 2 }),
        });
        assert!(
            page(Some(&wrong), &e, 0)
                .1
                .contains("Wrong code. 2 tries left.")
        );
        let locked = prompt(Phase::Code {
            error: Some(SetupError::LockedOut { seconds: 75 }),
        });
        assert!(page(Some(&locked), &e, 0).1.contains("Try again in 75 s."));
        let down = prompt(Phase::Code {
            error: Some(SetupError::Unreachable),
        });
        assert!(
            page(Some(&down), &e, 0)
                .1
                .contains("Can't reach the server.")
        );
    }

    #[test]
    fn every_page_fits_the_box() {
        let info = live_proto::recovery::RecoveryInfo {
            secret: "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".into(),
            otpauth: "otpauth://totp/x".into(),
            code: "123-456".into(),
            period: 30,
            expires_at: 20,
        };
        let errors = [
            None,
            Some(SetupError::WrongCode),
            Some(SetupError::WrongCodeLeft { left: 4 }),
            Some(SetupError::LockedOut { seconds: 86_400 }),
            Some(SetupError::Unreachable),
            Some(SetupError::NeedsInvite),
            Some(SetupError::GamertagTaken),
        ];
        let mut phases = vec![
            Phase::Working,
            Phase::Done,
            Phase::Cancelled,
            Phase::Created { info: None },
            Phase::Created { info: Some(info) },
        ];
        for e in errors {
            phases.push(Phase::Code { error: e.clone() });
            phases.push(Phase::Gamertag { error: e.clone() });
            phases.push(Phase::Invite { error: e.clone() });
            phases.push(Phase::Create { error: e });
        }
        let long = Prompt {
            gamertag: "WWWWWWWWWWWWWWW".into(),
            server_url: "https://a-very-long-host-name.example.org/".into(),
            ..prompt(code())
        };
        for phase in phases {
            for p in [
                prompt(phase.clone()),
                Prompt {
                    phase: phase.clone(),
                    ..long.clone()
                },
            ] {
                let (label, body) = page(Some(&p), &CodeEntry::default(), 10);
                assert!(label.len() <= LINE_CHARS, "{label}");
                let lines: Vec<&str> = body.split("\r\n").collect();
                assert!(
                    lines.len() <= crate::account_screen::MAX_LINES,
                    "{phase:?}: {body}"
                );
                for l in &lines {
                    assert!(l.chars().count() <= LINE_CHARS, "{phase:?}: {l:?}");
                    assert!(!l.contains('|'));
                }
            }
        }
        let (_, closed) = page(None, &CodeEntry::default(), 0);
        assert!(closed.starts_with("Nothing is waiting"));
    }

    #[test]
    fn the_host_drops_the_scheme_and_slash() {
        assert_eq!(host("https://live.forbes.red/"), "live.forbes.red");
        assert_eq!(host("http://127.0.0.1:3000"), "127.0.0.1:3000");
        assert_eq!(
            host(&format!("https://{}", "x".repeat(60))).len(),
            LINE_CHARS - 6
        );
    }

    #[test]
    fn strings_are_nul_terminated_cut_on_a_character_and_found_by_id() {
        assert_eq!(served("ab"), b"ab\0");
        let long = "é".repeat(400);
        let s = served(&long);
        assert!(s.len() <= BUFFER as usize);
        assert!(std::str::from_utf8(&s[..s.len() - 1]).is_ok());
        assert_eq!(string_at(0x1000, TITLE_ID), Some(0x1000));
        assert_eq!(string_at(0x1000, TEXT_IDS[0]), Some(0x1000 + BUFFER));
        assert_eq!(string_at(0x1000, TEXT_IDS[3]), Some(0x1000 + 4 * BUFFER));
        assert_eq!(string_at(0x1000, ID_BASE), None);
        for id in [TITLE_ID, TEXT_IDS[0], TEXT_IDS[3]] {
            assert!(crate::layout_glue::is_ours(id));
        }
    }

    #[test]
    fn the_dialog_switch_picks_the_dialogs_window() {
        use crate::ui_kit::dialog_tag::{DIALOG_WINDOW, MAIN_WINDOW};
        assert_eq!(window(None), MAIN_WINDOW, "off until a game run settles it");
        for off in ["0", "off", ""] {
            assert_eq!(window(Some(off)), MAIN_WINDOW);
        }
        for on in ["1", " on ", "true"] {
            assert_eq!(window(Some(on)), DIALOG_WINDOW);
        }
    }

    #[test]
    fn a_snapshot_waits_while_our_keyboard_is_open() {
        let typing = Typing::default();
        assert_eq!(snapshot_live(&typing), None);
        typing.start(Field::Invite);
        assert!(snapshot_live(&typing).is_some());
    }
}
