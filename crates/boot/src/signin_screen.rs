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
//! `0x149f1e(1 << controller, loader)`. `H2_SIGNIN_PROMPT=0` leaves the
//! prompt out: the waiting sign-in is cancelled at once and fails, with a
//! line saying to use `h2 play --signin-terminal`.
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
use feature::{BuildError, Builder, Manifest};
use live_hle::Hle;
use live_hle::signin::{Phase, Prompt};
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, GuestFault, Runtime, TransferAbi, abi};

use crate::account_screen::{LINE_CHARS, layout, recovery_lines, wrap};
use crate::list_class::{self, Action, Button, Buttons, ListClass, LiveText, Rows, TextBlock};
use crate::signin_keyboard::{self as keyboard, Field, Typing};
use crate::ui::{Chosen, LABEL_SLOT};

/// The screen id and feature id.
pub const SCREEN: &str = "signin-code";
pub const FEATURE: &str = "signin-code";
/// The code's length.
pub const DIGITS: usize = 6;
/// The HLE's `XOnlineTaskContinue` entry (live_hle::entries).
pub const TASK_CONTINUE: u32 = 0x003a_0620;

/// Our string ids: the title, then the label and the description, each a
/// pair the text alternates between.
pub const ID_BASE: u32 = crate::layout_glue::ID_BASE + 0x0500_0000;
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
pub const A_BUTTON: u32 = 0;
pub const B_BUTTON: u32 = 1;

/// The description: the right half of the screen, the level select's
/// description font and colour. `H2_SIGNIN_TEXT_RECT=top,left,bottom,right`
/// moves it without a rebuild.
pub const TEXT_BLOCK: TextBlock = TextBlock {
    rect: [110, -40, -130, 300],
    font: 1,
    colour: [0.388235, 0.580392, 0.756863],
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
        Some(match (kind, button) {
            (list_class::UP_EVENT, _) => Press::Up,
            (list_class::DOWN_EVENT, _) => Press::Down,
            (list_class::LEFT_EVENT, _) => Press::Left,
            (list_class::RIGHT_EVENT, _) => Press::Right,
            (list_class::BUTTON_EVENT, A_BUTTON) => Press::A,
            (list_class::BUTTON_EVENT, B_BUTTON) => Press::B,
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
}

impl CodeEntry {
    pub fn code(&self) -> String {
        self.digits.iter().map(|d| char::from(b'0' + d)).collect()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// `press` on the page of `phase`.
    pub fn press(&mut self, phase: &Phase, press: Press) -> Effect {
        match (phase, press) {
            (Phase::Code { .. }, Press::Up) => {
                self.digits[self.cursor] = (self.digits[self.cursor] + 1) % 10;
                Effect::Redraw
            }
            (Phase::Code { .. }, Press::Down) => {
                self.digits[self.cursor] = (self.digits[self.cursor] + 9) % 10;
                Effect::Redraw
            }
            (Phase::Code { .. }, Press::Left) if self.cursor > 0 => {
                self.cursor -= 1;
                Effect::Redraw
            }
            (Phase::Code { .. }, Press::Right) if self.cursor + 1 < DIGITS => {
                self.cursor += 1;
                Effect::Redraw
            }
            (Phase::Code { .. }, Press::A) => Effect::Submit(self.code()),
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
            l.extend(match error {
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
                "Set up recovery on /dash.".into(),
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
        .buttons(buttons)
}

/// A on the row (or Start): the press, then closing the screen when it is
/// done with.
async fn act(cx: &Cx, rows: &SigninRows, screen: u32) -> Result<(), EntryError> {
    let effect = rows.press(Press::A);
    if let Effect::Keyboard(field) = effect {
        return open_keyboard(cx, rows, field).await;
    }
    if effect == Effect::Close {
        let (window, slot) = cx.read(|m| -> Result<_, MemFault> {
            Ok((m.u32(screen + SCREEN_WINDOW)?, m.u32(screen + SCREEN_SLOT)?))
        })?;
        eprintln!("signin prompt: closing");
        cx.call(
            GuestCall::new(crate::skulls_screen::SCREEN_CLOSE)
                .eax(slot)
                .ecx(window),
        )
        .await?;
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

const SCREEN_WINDOW: u32 = 0x74;
const SCREEN_SLOT: u32 = 0x78;

/// The served strings: a page of ours the locale lookup reads.
pub struct Strings(pub Arc<SigninRows>);

impl feature::Feature for Strings {
    fn manifest(&self) -> Manifest {
        Manifest::new("signin-strings", "The sign-in prompt's text").doc("docs/live/live.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let base = b.guest_alloc(BUFFER * PAGE_BUFFERS, Permissions::RW)?;
        self.0.page.set(base).ok();
        let rows = Arc::clone(&self.0);
        b.locale()
            .source("signin text", move |id| rows.string(base, id));
        Ok(())
    }
}

/// Whether the prompt is on (`H2_SIGNIN_PROMPT=0` leaves it out).
pub fn prompt_on(env: Option<&str>) -> bool {
    !matches!(env.map(str::trim), Some("0" | "off" | "false" | "no"))
}

/// Registers the screen and its strings and wraps the logon's polling so a
/// waiting sign-in opens it. Nothing without Live or a server.
pub fn install(
    rt: &Runtime,
    registry: &mut feature::Registry,
    hle: Option<&Arc<Hle>>,
) -> anyhow::Result<bool> {
    let Some(hle) = hle.filter(|h| h.config().server_url.is_some()) else {
        return Ok(false);
    };
    let on = prompt_on(std::env::var("H2_SIGNIN_PROMPT").ok().as_deref());
    let rows = Arc::new(SigninRows::new(Arc::clone(hle)));
    if on {
        let block = text_block(std::env::var("H2_SIGNIN_TEXT_RECT").ok().as_deref());
        registry
            .add(&Strings(Arc::clone(&rows)))
            .map_err(|e| anyhow::anyhow!("signin strings: {e}"))?;
        match registry.add(&class(Arc::clone(&rows), block)) {
            Ok(true) => {}
            Ok(false) => eprintln!("signin prompt: features.{FEATURE} is off"),
            Err(e) => eprintln!("signin prompt: {e}"),
        }
    }
    wrap_continue(rt, rows, on)
}

/// The description block, moved by `H2_SIGNIN_TEXT_RECT` when it parses.
pub fn text_block(env: Option<&str>) -> TextBlock {
    let Some(v) = env else { return TEXT_BLOCK };
    let n: Vec<i16> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    match <[i16; 4]>::try_from(n) {
        Ok(rect) => {
            eprintln!("signin prompt: text at {rect:?} (H2_SIGNIN_TEXT_RECT)");
            TextBlock { rect, ..TEXT_BLOCK }
        }
        Err(_) => {
            eprintln!("signin prompt: H2_SIGNIN_TEXT_RECT={v:?} is not top,left,bottom,right");
            TEXT_BLOCK
        }
    }
}

/// Wraps the HLE's `XOnlineTaskContinue`: after each call, a sign-in that
/// has just started waiting opens the prompt for its controller (or, with
/// the prompt off, is cancelled).
fn wrap_continue(rt: &Runtime, rows: Arc<SigninRows>, on: bool) -> anyhow::Result<bool> {
    let hle = Arc::clone(&rows.hle);
    let Some(inner) = rt.edit_dispatch(|d| d.override_handler(TASK_CONTINUE)) else {
        eprintln!("signin prompt: no Live entry at {TASK_CONTINUE:#x}; not wrapped");
        return Ok(false);
    };
    rt.edit_dispatch(|d| {
        d.register_override(TASK_CONTINUE, "signin prompt: XOnlineTaskContinue", move |cpu, ctx, _| {
            let done = inner.invoke(cpu, ctx, TASK_CONTINUE);
            let Some(controller) = hle.take_signin_open() else {
                return Ok(done);
            };
            if !on {
                eprintln!(
                    "signin prompt: off (H2_SIGNIN_PROMPT); the sign-in is cancelled. Use h2 play --signin-terminal, or h2 live account recover"
                );
                hle.signin_cancel();
                return Ok(done);
            }
            match list_class::loader_of(SCREEN) {
                Some(loader) if done.kind == abi::RETURN && controller < 4 => {
                    eprintln!("signin prompt: opening for controller {controller}");
                    open_after(cpu, ctx, done, &rows, 1 << controller, loader)
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

/// After the wrapped call returned (`done`, EAX its result): calls
/// `0x149f1e(mask, loader)` (stdcall, `ret 8`), then returns as `done`
/// with EAX restored.
fn open_after(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    done: TransferAbi,
    rows: &SigninRows,
    mask: u32,
    loader: u32,
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
    cpu.esp -= 8;
    m.set_u32(cpu.esp, mask)?;
    m.set_u32(cpu.esp + 4, loader)?;
    ctx.call_guest(
        cpu,
        &mut m,
        engine::points::OPEN_SCREEN.base,
        move |cpu, _| {
            *cpu = saved;
            Ok(done)
        },
    )
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
    fn the_prompt_switch_reads_off_values() {
        assert!(prompt_on(None));
        assert!(prompt_on(Some("1")));
        for off in ["0", "off", " false ", "no"] {
            assert!(!prompt_on(Some(off)));
        }
    }

    #[test]
    fn a_bad_rect_keeps_the_default() {
        assert_eq!(text_block(None), TEXT_BLOCK);
        assert_eq!(text_block(Some("1,2,3")), TEXT_BLOCK);
        assert_eq!(
            text_block(Some("10, -20, -30, 40")).rect,
            [10, -20, -30, 40]
        );
    }
}
