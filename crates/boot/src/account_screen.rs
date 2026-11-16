//! The in-game Live account screen's logic (docs/live/live.md, "In-game
//! account setup"): make a Live account or recover one on this machine,
//! driven by the gamepad and the retail on-screen keyboard. Pure Rust, no
//! guest memory: the glue that shows it feeds it presses, typed text and
//! the results of [`live_hle::account_setup`] calls, and acts on the
//! [`Effect`] each returns.
//!
//! ```text
//! Menu {Create, Recover, Cancel}
//!   Create:  gamertag -> [invite, if the server needs one] -> Working
//!            -> Write this down (code, key) -> Done
//!   Recover: gamertag -> code -> Working -> Done
//! ```
//!
//! A on a menu row picks it; B goes back a step (from the menu, closes).
//! Each text field is a [`Effect::Keyboard`] request the glue fulfils with
//! [`AccountScreen::typed`] (`None`: the keyboard was cancelled, which is B).
//! An error shows its line and A tries the field again.
//!
//! Text: at most [`MAX_LINES`] lines of at most [`LINE_CHARS`] characters,
//! broken with `\r\n` and never holding `|`, the measurements of the stock
//! dialog's message box ([`crate::live_notice`]).

use live_hle::account_setup::SetupError;
use live_proto::recovery::RecoveryInfo;

/// Lines the dialog's message box holds.
pub const MAX_LINES: usize = 5;
/// Characters per line that measure under the box's 240 pixels.
pub const LINE_CHARS: usize = 40;
/// Gamertag and code field lengths, and the longest invite typed (a
/// short code is 29 with dashes; a signed one is pasted, not typed).
pub const GAMERTAG_CHARS: u16 = 15;
pub const CODE_CHARS: u16 = 7;
pub const INVITE_CHARS: u16 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Create,
    Recover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Gamertag,
    Invite,
    Code,
}

/// The menu's rows, in order.
pub const MENU: [&str; 3] = ["Create a Live account", "Recover my account", "Cancel"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Menu {
        focus: usize,
    },
    /// The keyboard is open for `field`.
    Enter {
        flow: Flow,
        field: Field,
    },
    /// The server call is running.
    Working {
        flow: Flow,
    },
    /// The new account's recovery secret, shown once.
    ShowRecovery {
        info: RecoveryInfo,
    },
    Done,
    /// `error` happened; A types `retry` again.
    Error {
        flow: Flow,
        retry: Field,
        error: SetupError,
    },
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Up,
    Down,
    A,
    B,
}

/// What the retail keyboard is opened with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardRequest {
    pub field: Field,
    pub title: &'static str,
    pub initial: String,
    /// Characters, the terminator not counted.
    pub max_chars: u16,
}

/// The server work to start, off the game thread
/// ([`live_hle::account_setup::Job`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    Create {
        gamertag: String,
        invite: Option<String>,
    },
    Recover {
        gamertag: String,
        code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Redraw: the step changed.
    Redraw,
    Keyboard(KeyboardRequest),
    Start(Work),
    /// Leave the screen. `signed_up` when an account was saved, so the
    /// glue reloads the accounts ([`live_hle::Hle::reload_accounts`]).
    Close {
        signed_up: bool,
    },
}

/// How the server work ended. The glue saves the entry before reporting
/// `Ok`; a failed save is `Err(SetupError::Save)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Created(Option<RecoveryInfo>),
    Recovered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountScreen {
    pub step: Step,
    /// From `GET /v1/server` before the screen opens; `None` when the
    /// server did not say. A "needs an invite" refusal sets it.
    pub invite_only: Option<bool>,
    pub gamertag: String,
    pub invite: String,
    pub code: String,
    saved: bool,
}

impl AccountScreen {
    pub fn new(invite_only: Option<bool>) -> Self {
        AccountScreen {
            step: Step::Menu { focus: 0 },
            invite_only,
            gamertag: String::new(),
            invite: String::new(),
            code: String::new(),
            saved: false,
        }
    }

    fn keyboard(&mut self, flow: Flow, field: Field) -> Effect {
        self.step = Step::Enter { flow, field };
        let (title, initial, max_chars) = match field {
            Field::Gamertag => ("Gamertag", self.gamertag.clone(), GAMERTAG_CHARS),
            Field::Invite => ("Invite code", self.invite.clone(), INVITE_CHARS),
            Field::Code => ("Recovery code", String::new(), CODE_CHARS),
        };
        Effect::Keyboard(KeyboardRequest {
            field,
            title,
            initial,
            max_chars,
        })
    }

    fn start(&mut self, flow: Flow) -> Effect {
        self.step = Step::Working { flow };
        Effect::Start(match flow {
            Flow::Create => Work::Create {
                gamertag: self.gamertag.clone(),
                invite: (!self.invite.is_empty()).then(|| self.invite.clone()),
            },
            Flow::Recover => Work::Recover {
                gamertag: self.gamertag.clone(),
                code: self.code.clone(),
            },
        })
    }

    fn close(&mut self) -> Effect {
        self.step = Step::Closed;
        Effect::Close {
            signed_up: self.saved,
        }
    }

    pub fn press(&mut self, press: Press) -> Effect {
        match (self.step.clone(), press) {
            (Step::Menu { focus }, Press::Up) => {
                self.step = Step::Menu {
                    focus: (focus + MENU.len() - 1) % MENU.len(),
                };
                Effect::Redraw
            }
            (Step::Menu { focus }, Press::Down) => {
                self.step = Step::Menu {
                    focus: (focus + 1) % MENU.len(),
                };
                Effect::Redraw
            }
            (Step::Menu { focus: 0 }, Press::A) => self.keyboard(Flow::Create, Field::Gamertag),
            (Step::Menu { focus: 1 }, Press::A) => self.keyboard(Flow::Recover, Field::Gamertag),
            (Step::Menu { .. }, Press::A | Press::B) => self.close(),
            // The keyboard owns input while open; B there arrives as typed(None).
            (Step::Enter { .. } | Step::Working { .. } | Step::Closed, _) => Effect::None,
            (Step::ShowRecovery { .. }, Press::A) => {
                self.step = Step::Done;
                Effect::Redraw
            }
            // The secret is shown once: B does not skip past it by accident.
            (Step::ShowRecovery { .. }, _) => Effect::None,
            (Step::Done, Press::A | Press::B) => self.close(),
            (Step::Error { flow, retry, .. }, Press::A) => self.keyboard(flow, retry),
            (Step::Error { .. }, Press::B) => {
                self.step = Step::Menu { focus: 0 };
                Effect::Redraw
            }
            (_, Press::Up | Press::Down) => Effect::None,
        }
    }

    /// The keyboard closed: `Some(text)` accepted, `None` cancelled.
    pub fn typed(&mut self, text: Option<&str>) -> Effect {
        let Step::Enter { flow, field } = self.step.clone() else {
            return Effect::None;
        };
        let Some(text) = text else {
            // Back one field, or to the menu.
            return match (flow, field) {
                (Flow::Create, Field::Invite) => self.keyboard(flow, Field::Gamertag),
                (Flow::Recover, Field::Code) => self.keyboard(flow, Field::Gamertag),
                _ => {
                    self.step = Step::Menu {
                        focus: if flow == Flow::Create { 0 } else { 1 },
                    };
                    Effect::Redraw
                }
            };
        };
        let text = text.trim().to_string();
        match field {
            Field::Gamertag => {
                if live_proto::Gamertag::new(&text).is_err() {
                    return self.fail(flow, Field::Gamertag, SetupError::BadGamertag);
                }
                self.gamertag = text;
                match flow {
                    Flow::Create if self.invite_only == Some(true) => {
                        self.keyboard(flow, Field::Invite)
                    }
                    Flow::Create => self.start(flow),
                    Flow::Recover => self.keyboard(flow, Field::Code),
                }
            }
            Field::Invite => {
                if text.is_empty() {
                    return self.fail(flow, Field::Invite, SetupError::NeedsInvite);
                }
                self.invite = text;
                self.start(flow)
            }
            Field::Code => {
                if live_proto::recovery::parse_code(&text).is_none() {
                    return self.fail(flow, Field::Code, SetupError::WrongCode);
                }
                self.code = text;
                self.start(flow)
            }
        }
    }

    fn fail(&mut self, flow: Flow, retry: Field, error: SetupError) -> Effect {
        self.step = Step::Error { flow, retry, error };
        Effect::Redraw
    }

    /// The server work ended.
    pub fn finished(&mut self, result: Result<Outcome, SetupError>) -> Effect {
        let Step::Working { flow } = self.step else {
            return Effect::None;
        };
        match result {
            Ok(Outcome::Created(info)) => {
                self.saved = true;
                self.step = match info {
                    Some(info) => Step::ShowRecovery { info },
                    None => Step::Done,
                };
            }
            Ok(Outcome::Recovered) => {
                self.saved = true;
                self.step = Step::Done;
            }
            Err(error) => {
                let retry = match (&error, flow) {
                    (SetupError::NeedsInvite | SetupError::InviteInvalid, _) => {
                        self.invite_only = Some(true);
                        Field::Invite
                    }
                    (
                        SetupError::WrongCode
                        | SetupError::WrongCodeLeft { .. }
                        | SetupError::LockedOut { .. },
                        Flow::Recover,
                    ) => {
                        Field::Code
                    }
                    _ => Field::Gamertag,
                };
                self.step = Step::Error { flow, retry, error };
            }
        }
        Effect::Redraw
    }

    /// The screen's header.
    pub fn title(&self) -> &'static str {
        match &self.step {
            Step::Menu { .. } | Step::Closed => "Live account",
            Step::Enter {
                flow: Flow::Create, ..
            }
            | Step::Working {
                flow: Flow::Create, ..
            } => "Create a Live account",
            Step::Enter { .. } | Step::Working { .. } => "Recover my account",
            Step::ShowRecovery { .. } => "Write this down",
            Step::Done => "All set",
            Step::Error { .. } => "That didn't work",
        }
    }

    /// The body text at unix second `now` (the recovery code counts down),
    /// laid out for the dialog: see [`layout`].
    pub fn text(&self, now: i64) -> String {
        layout(&self.lines(now))
    }

    fn lines(&self, now: i64) -> Vec<String> {
        match &self.step {
            Step::Menu { focus } => MENU
                .iter()
                .enumerate()
                .map(|(i, r)| format!("{}{r}", if i == *focus { "> " } else { "  " }))
                .collect(),
            Step::Enter { .. } => vec!["Type on the keyboard.".into()],
            Step::Working { flow: Flow::Create } => {
                vec![format!("Creating {}...", self.gamertag)]
            }
            Step::Working {
                flow: Flow::Recover,
            } => {
                vec![format!("Recovering {}...", self.gamertag)]
            }
            Step::ShowRecovery { info } => recovery_lines(info, now),
            Step::Done => vec![
                format!("{} is on this machine.", self.gamertag),
                "Sign in to Xbox Live to play.".into(),
            ],
            Step::Error { error, .. } => {
                let mut l = wrap(&error.to_string(), LINE_CHARS);
                l.push("A: try again   B: back".into());
                l
            }
            Step::Closed => Vec::new(),
        }
    }
}

/// The once-shown recovery page: the current code with its seconds left,
/// the authenticator key in groups of four, and how to use it.
pub fn recovery_lines(info: &RecoveryInfo, now: i64) -> Vec<String> {
    let left = (info.expires_at - now).clamp(0, info.period.max(0));
    let groups: Vec<String> = info
        .secret
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    let mut lines = vec![
        format!("Code now: {} ({left} s left)", info.code),
        "Authenticator key:".into(),
    ];
    lines.extend(groups.chunks(4).map(|g| g.join(" ")));
    lines.push("New device: Recover, gamertag + code".into());
    lines
}

/// Greedy word wrap to `width` characters; a word longer than a line is
/// cut.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            lines.push(head);
        }
        if line.is_empty() {
            line = word;
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(&word);
        } else {
            lines.push(std::mem::replace(&mut line, word));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Lines as the dialog takes them: each wrapped to [`LINE_CHARS`], `|`
/// (a markup escape) and control characters replaced, at most
/// [`MAX_LINES`], joined with `\r\n`.
pub fn layout(lines: &[String]) -> String {
    lines
        .iter()
        .flat_map(|l| {
            let clean: String = l
                .chars()
                .map(|c| if c == '|' || c.is_control() { '?' } else { c })
                .collect();
            if clean.chars().count() <= LINE_CHARS {
                vec![clean]
            } else {
                wrap(&clean, LINE_CHARS)
            }
        })
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> RecoveryInfo {
        RecoveryInfo {
            secret: "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".into(),
            otpauth: "otpauth://totp/x".into(),
            code: "123-456".into(),
            period: 30,
            expires_at: 1_000_020,
        }
    }

    fn keyboard_field(e: &Effect) -> Field {
        match e {
            Effect::Keyboard(k) => k.field,
            other => panic!("not a keyboard request: {other:?}"),
        }
    }

    fn fits(text: &str) {
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert!(lines.len() <= MAX_LINES, "{text:?}");
        assert!(
            lines
                .iter()
                .all(|l| l.chars().count() <= LINE_CHARS && !l.contains(['\r', '\n', '|'])),
            "{text:?}"
        );
    }

    #[test]
    fn create_on_an_open_server_then_shows_the_secret_once() {
        let mut s = AccountScreen::new(Some(false));
        assert_eq!(keyboard_field(&s.press(Press::A)), Field::Gamertag);
        assert_eq!(
            s.typed(Some("  Pete ")),
            Effect::Start(Work::Create {
                gamertag: "Pete".into(),
                invite: None
            })
        );
        assert_eq!(s.title(), "Create a Live account");
        assert_eq!(s.press(Press::A), Effect::None, "busy while working");
        s.finished(Ok(Outcome::Created(Some(info()))));
        assert_eq!(s.title(), "Write this down");
        assert_eq!(s.press(Press::B), Effect::None, "B never skips the secret");
        assert_eq!(s.press(Press::A), Effect::Redraw);
        assert_eq!(s.step, Step::Done);
        assert_eq!(s.press(Press::A), Effect::Close { signed_up: true });
    }

    #[test]
    fn invite_only_servers_ask_for_an_invite_known_or_learnt() {
        let mut s = AccountScreen::new(Some(true));
        s.press(Press::A);
        assert_eq!(keyboard_field(&s.typed(Some("Pete"))), Field::Invite);
        assert_eq!(
            s.typed(Some(" ABCDE-FGHIJ ")),
            Effect::Start(Work::Create {
                gamertag: "Pete".into(),
                invite: Some("ABCDE-FGHIJ".into())
            })
        );
        s.finished(Err(SetupError::InviteInvalid));
        assert!(matches!(
            s.step,
            Step::Error {
                retry: Field::Invite,
                ..
            }
        ));
        let req = s.press(Press::A);
        assert_eq!(keyboard_field(&req), Field::Invite);

        // Unknown policy: the server's refusal turns the invite field on.
        let mut s = AccountScreen::new(None);
        s.press(Press::A);
        s.typed(Some("Pete"));
        s.finished(Err(SetupError::NeedsInvite));
        assert_eq!(s.invite_only, Some(true));
        assert_eq!(keyboard_field(&s.press(Press::A)), Field::Invite);
        // Cancelling the invite goes back to the gamertag, kept.
        match s.typed(None) {
            Effect::Keyboard(k) => {
                assert_eq!((k.field, k.initial.as_str()), (Field::Gamertag, "Pete"))
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn recover_asks_gamertag_then_code_and_retries_a_wrong_code() {
        let mut s = AccountScreen::new(None);
        assert_eq!(s.press(Press::Down), Effect::Redraw);
        assert_eq!(keyboard_field(&s.press(Press::A)), Field::Gamertag);
        assert_eq!(keyboard_field(&s.typed(Some("Pete"))), Field::Code);
        // Not six digits: refused before any request.
        assert_eq!(s.typed(Some("12")), Effect::Redraw);
        assert!(matches!(
            s.step,
            Step::Error {
                retry: Field::Code,
                error: SetupError::WrongCode,
                ..
            }
        ));
        assert_eq!(keyboard_field(&s.press(Press::A)), Field::Code);
        assert_eq!(
            s.typed(Some("123-456")),
            Effect::Start(Work::Recover {
                gamertag: "Pete".into(),
                code: "123-456".into()
            })
        );
        s.finished(Err(SetupError::LockedOut { seconds: 240 }));
        assert!(
            s.text(0)
                .replace("\r\n", " ")
                .contains("Try again in 4 min.")
        );
        s.press(Press::A);
        s.typed(Some("654321"));
        s.finished(Ok(Outcome::Recovered));
        assert_eq!(s.step, Step::Done);
        assert_eq!(
            s.text(0),
            "Pete is on this machine.\r\nSign in to Xbox Live to play."
        );
        assert_eq!(s.press(Press::B), Effect::Close { signed_up: true });
    }

    #[test]
    fn back_and_cancel_walk_out_without_saving() {
        let mut s = AccountScreen::new(None);
        s.press(Press::A);
        assert_eq!(s.typed(None), Effect::Redraw);
        assert_eq!(s.step, Step::Menu { focus: 0 });
        s.press(Press::Down);
        s.press(Press::A);
        s.typed(Some("Pete"));
        // Code cancelled: back to the gamertag; then to the menu on Recover.
        assert_eq!(keyboard_field(&s.typed(None)), Field::Gamertag);
        s.typed(None);
        assert_eq!(s.step, Step::Menu { focus: 1 });
        s.press(Press::Down);
        assert_eq!(s.press(Press::A), Effect::Close { signed_up: false });
        let mut s = AccountScreen::new(None);
        assert_eq!(s.press(Press::Up), Effect::Redraw);
        assert_eq!(s.step, Step::Menu { focus: 2 });
        assert_eq!(s.press(Press::B), Effect::Close { signed_up: false });
        // A bad gamertag never reaches the server.
        let mut s = AccountScreen::new(None);
        s.press(Press::A);
        s.typed(Some("sixteen chars xx"));
        assert!(matches!(
            s.step,
            Step::Error {
                error: SetupError::BadGamertag,
                ..
            }
        ));
        assert_eq!(s.press(Press::B), Effect::Redraw);
        assert_eq!(s.step, Step::Menu { focus: 0 });
        // Late results and keyboard replies are ignored.
        assert_eq!(s.finished(Ok(Outcome::Recovered)), Effect::None);
        assert_eq!(s.typed(Some("x")), Effect::None);
    }

    #[test]
    fn the_recovery_page_fits_and_counts_down() {
        let lines = recovery_lines(&info(), 1_000_003);
        assert_eq!(
            lines,
            [
                "Code now: 123-456 (17 s left)",
                "Authenticator key:",
                "ABCD EFGH IJKL MNOP",
                "QRST UVWX YZ23 4567",
                "New device: Recover, gamertag + code",
            ]
        );
        let mut s = AccountScreen::new(None);
        s.step = Step::ShowRecovery { info: info() };
        fits(&s.text(1_000_003));
        assert!(
            s.text(1_000_050)
                .starts_with("Code now: 123-456 (0 s left)")
        );
    }

    #[test]
    fn every_page_fits_the_dialog() {
        let mut s = AccountScreen::new(None);
        fits(&s.text(0));
        assert_eq!(
            s.text(0),
            "> Create a Live account\r\n  Recover my account\r\n  Cancel"
        );
        for error in [
            SetupError::GamertagTaken,
            SetupError::NeedsInvite,
            SetupError::InviteInvalid,
            SetupError::WrongCode,
            SetupError::LockedOut { seconds: 86_400 },
            SetupError::Unreachable,
            SetupError::Insecure,
            SetupError::Server(500),
            SetupError::Save,
        ] {
            s.step = Step::Error {
                flow: Flow::Create,
                retry: Field::Gamertag,
                error,
            };
            fits(&s.text(0));
        }
        s.gamertag = "abcdefghijklmno".into();
        s.step = Step::Working {
            flow: Flow::Recover,
        };
        fits(&s.text(0));
    }

    #[test]
    fn layout_wraps_cleans_and_caps() {
        assert_eq!(wrap("a bb ccc", 4), ["a bb", "ccc"]);
        assert_eq!(wrap(&"x".repeat(9), 4), ["xxxx", "xxxx", "x"]);
        let long = vec!["word ".repeat(30), "a|b\nc".into()];
        let text = layout(&long);
        fits(&text);
        assert_eq!(text.split("\r\n").count(), MAX_LINES);
        assert_eq!(layout(&["a|b\nc".into()]), "a?b?c");
    }
}
