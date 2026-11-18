//! In-game sign-in for an identity whose key for this server is not on
//! this machine yet (docs/live/live.md, "In-game sign-in").
//!
//! Such an identity is an accounts-file entry marked `needs_code`
//! ([`crate::config`]): `XOnlineGetUsers` lists it like any account, so
//! the player picks it in Halo 2's own sign-in. `XOnlineLogon` for it
//! starts a [`Signin`] instead of the service logon and the task stays
//! pending (`0`), so the game keeps its "signing in" state. The boot's
//! prompt screen reads [`Prompt`] and answers with a code
//! ([`Signin::submit`]), a create ([`Signin::create`]) or a cancel. The
//! server call runs on its own thread ([`Job`]); on success the new entry
//! is saved to the session's accounts file and the shared one, the
//! accounts are read again, and the logon goes on with the saved key as if
//! the account had always been there. No guest memory here.
//!
//! **A new account.** With [`crate::LiveConfig::offer_new_account`] (the
//! app builds, which have no terminal) the list also holds
//! [`crate::NEW_ACCOUNT_TAG`]. Signing in with it starts a [`Signin`] that
//! asks for a gamertag first ([`Phase::Gamertag`], typed on the game's
//! keyboard, [`Signin::typed`]) and tries to make it: free, it is made
//! (with an invite, [`Phase::Invite`], when the server wants one) and the
//! logon goes on as the new account; taken, the identity is saved as
//! waiting for its code ([`account_setup::save_waiting`]) and the prompt
//! asks for the code as for any such identity.

use crate::account_setup::{self, Job, LocalEntry, SetupError};
use live_proto::recovery::RecoveryInfo;
use std::path::PathBuf;

/// What the prompt shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Type the gamertag to make or add; `error` from the last try.
    Gamertag { error: Option<SetupError> },
    /// The server wants an invite to make the account.
    Invite { error: Option<SetupError> },
    /// Type the 6-digit code; `error` from the last try.
    Code { error: Option<SetupError> },
    /// The gamertag is not on the server: A makes it there.
    Create { error: Option<SetupError> },
    /// The server call is running.
    Working,
    /// Made on the server now: its recovery secret, shown once. Sign-in
    /// goes on behind it.
    Created { info: Option<RecoveryInfo> },
    /// Verified; sign-in goes on.
    Done,
    /// The player backed out; the logon fails.
    Cancelled,
}

impl Phase {
    /// Nothing more to ask: the prompt may close.
    pub fn finished(&self) -> bool {
        matches!(self, Phase::Created { .. } | Phase::Done | Phase::Cancelled)
    }
}

/// The prompt for one sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub gamertag: String,
    pub server_url: String,
    /// The controller signing in (the slot in the logon's user array).
    pub controller: u32,
    pub phase: Phase,
}

/// A finished server call: the entry, the secret of a new account, and
/// whether it was saved.
type Outcome = Result<(LocalEntry, Option<RecoveryInfo>), SetupError>;

/// Which server call a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    /// Recover with a code.
    Code,
    /// Make the gamertag the prompt names (offered after a code try).
    Create,
    /// Make a typed gamertag, with `invite` when one was typed; a taken one
    /// is saved as waiting for its code.
    Add { invite: bool },
}

/// One sign-in waiting for a code. Owned by the HLE state; the logon task
/// `handle` is pending while it lives.
pub(crate) struct Signin {
    pub handle: u32,
    pub slot: usize,
    prompt: Prompt,
    /// The session's accounts file first, then the shared one.
    files: Vec<PathBuf>,
    job: Option<(Job<Outcome>, Call)>,
    verified: Option<LocalEntry>,
    opened: bool,
    /// The invite typed for a new account.
    invite: Option<String>,
}

impl Signin {
    pub fn new(
        handle: u32,
        slot: usize,
        gamertag: &str,
        server_url: &str,
        files: Vec<PathBuf>,
    ) -> Self {
        Signin {
            handle,
            slot,
            prompt: Prompt {
                gamertag: gamertag.into(),
                server_url: server_url.into(),
                controller: slot as u32,
                phase: Phase::Code { error: None },
            },
            files,
            job: None,
            verified: None,
            opened: false,
            invite: None,
        }
    }

    /// The sign-in of [`crate::NEW_ACCOUNT_TAG`]: asks for a gamertag first.
    pub fn adding(handle: u32, slot: usize, server_url: &str, files: Vec<PathBuf>) -> Self {
        let mut s = Signin::new(handle, slot, "", server_url, files);
        s.prompt.phase = Phase::Gamertag { error: None };
        s
    }

    /// Text typed on the keyboard while a gamertag or an invite is asked
    /// for; `None` when the keyboard was backed out of (from the invite,
    /// back to the gamertag). False when no text is being asked for.
    pub fn typed(&mut self, text: Option<&str>) -> bool {
        self.poll();
        match typed(&self.prompt.phase, text) {
            None => return false,
            Some(Typed::Keep) => {}
            Some(Typed::Phase(p)) => self.prompt.phase = p,
            Some(Typed::Gamertag(tag)) => {
                self.prompt.gamertag = tag;
                self.invite = None;
                self.add();
            }
            Some(Typed::Invite(invite)) => {
                self.invite = Some(invite);
                self.add();
            }
        }
        true
    }

    /// Makes the typed gamertag; a taken one is saved as waiting.
    fn add(&mut self) {
        let (url, tag, invite, files) = (
            self.prompt.server_url.clone(),
            self.prompt.gamertag.clone(),
            self.invite.clone(),
            self.files.clone(),
        );
        eprintln!("live: making {tag} on {url}");
        let call = Call::Add {
            invite: invite.is_some(),
        };
        self.job = Some((
            Job::spawn(
                move || match account_setup::create(&url, &tag, invite.as_deref()) {
                    Ok((entry, info)) => {
                        save_all(&files, &entry)?;
                        Ok((entry, info))
                    }
                    Err(SetupError::GamertagTaken) => {
                        save_waiting_all(&files, &tag, &url)?;
                        Err(SetupError::GamertagTaken)
                    }
                    Err(e) => Err(e),
                },
            ),
            call,
        ));
        self.prompt.phase = Phase::Working;
    }

    /// The prompt now (a finished server call is taken in first).
    pub fn prompt(&mut self) -> Prompt {
        self.poll();
        self.prompt.clone()
    }

    /// The controller whose prompt should open, once per sign-in.
    pub fn take_open(&mut self) -> Option<u32> {
        (!std::mem::replace(&mut self.opened, true)).then_some(self.prompt.controller)
    }

    /// A typed code. A code that is not six digits is refused here, with
    /// no request. Only while the code is asked for.
    pub fn submit(&mut self, code: &str) -> bool {
        self.poll();
        if !matches!(self.prompt.phase, Phase::Code { .. }) {
            return false;
        }
        if let Some(refused) = refused(code) {
            self.prompt.phase = refused;
            return true;
        }
        let (url, tag, code, files) = (
            self.prompt.server_url.clone(),
            self.prompt.gamertag.clone(),
            code.to_string(),
            self.files.clone(),
        );
        eprintln!("live: checking the code for {tag} with {url}");
        self.job = Some((
            Job::spawn(move || {
                let entry = account_setup::recover(&url, &tag, &code)?;
                save_all(&files, &entry)?;
                Ok((entry, None))
            }),
            Call::Code,
        ));
        self.prompt.phase = Phase::Working;
        true
    }

    /// Make the gamertag on the server (it is not there). Only while that
    /// is offered.
    pub fn create(&mut self) -> bool {
        self.poll();
        if !matches!(self.prompt.phase, Phase::Create { .. }) {
            return false;
        }
        let (url, tag, files) = (
            self.prompt.server_url.clone(),
            self.prompt.gamertag.clone(),
            self.files.clone(),
        );
        eprintln!("live: making {tag} on {url}");
        self.job = Some((
            Job::spawn(move || {
                let (entry, info) = account_setup::create(&url, &tag, None)?;
                save_all(&files, &entry)?;
                Ok((entry, info))
            }),
            Call::Create,
        ));
        self.prompt.phase = Phase::Working;
        true
    }

    /// The player backed out. A server call already running is left to
    /// finish (its account is saved, but this logon fails).
    pub fn cancel(&mut self) {
        self.poll();
        if !self.prompt.phase.finished() {
            eprintln!("live: sign-in for {} cancelled", self.prompt.gamertag);
            self.prompt.phase = Phase::Cancelled;
            self.verified = None;
        }
    }

    /// The verified entry, once, for the logon to go on with.
    pub fn take_verified(&mut self) -> Option<LocalEntry> {
        self.poll();
        self.verified.take()
    }

    pub fn cancelled(&self) -> bool {
        self.prompt.phase == Phase::Cancelled
    }

    /// A verified entry the logon has not taken yet.
    pub fn has_verified(&self) -> bool {
        self.verified.is_some()
    }

    fn poll(&mut self) {
        let Some(result) = self.job.as_ref().and_then(|(job, _)| job.poll()) else {
            return;
        };
        let (_, call) = self.job.take().expect("polled");
        if self.prompt.phase == Phase::Cancelled {
            return;
        }
        let (tag, url) = (&self.prompt.gamertag, &self.prompt.server_url);
        let result = result.map(|(entry, info)| {
            self.verified = Some(entry);
            info
        });
        self.prompt.phase = settled(call, result, tag, url);
    }
}

/// What text typed on the keyboard does ([`typed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// Nothing: the gamertag keyboard backed out of.
    Keep,
    /// The prompt shows this page.
    Phase(Phase),
    /// Make this gamertag ([`Call::Add`], no invite).
    Gamertag(String),
    /// Make the gamertag typed before, with this invite.
    Invite(String),
}

/// Text typed on the page `phase` (`None`: the keyboard backed out of),
/// trimmed; `None` when that page asks for no text. Pure: [`Signin::typed`]
/// and the boot's sign-in page both follow it.
pub fn typed(phase: &Phase, text: Option<&str>) -> Option<Typed> {
    Some(match (phase, text.map(str::trim)) {
        (Phase::Gamertag { .. }, None) => Typed::Keep,
        (Phase::Gamertag { .. }, Some(tag)) if live_proto::Gamertag::new(tag).is_err() => {
            Typed::Phase(Phase::Gamertag {
                error: Some(SetupError::BadGamertag),
            })
        }
        (Phase::Gamertag { .. }, Some(tag)) => Typed::Gamertag(tag.to_string()),
        (Phase::Invite { .. }, None) => Typed::Phase(Phase::Gamertag { error: None }),
        (Phase::Invite { .. }, Some("")) => Typed::Phase(Phase::Invite {
            error: Some(SetupError::NeedsInvite),
        }),
        (Phase::Invite { .. }, Some(invite)) => Typed::Invite(invite.to_string()),
        _ => return None,
    })
}

/// The page a typed code that is not six digits leaves (no request is
/// made), or `None` for one worth checking.
pub fn refused(code: &str) -> Option<Phase> {
    live_proto::recovery::parse_code(code)
        .is_none()
        .then_some(Phase::Code {
            error: Some(SetupError::WrongCode),
        })
}

/// The page after server call `call` for `tag` on `url` ended with
/// `result` (a new account's recovery secret, when it made one). Pure but
/// for the log line: [`Signin`] and the boot's sign-in page both follow it.
pub fn settled(
    call: Call,
    result: Result<Option<RecoveryInfo>, SetupError>,
    tag: &str,
    url: &str,
) -> Phase {
    match (result, call) {
        (Ok(info), _) => {
            eprintln!("live: {tag} verified on {url}; signing in");
            if call == Call::Code {
                Phase::Done
            } else {
                Phase::Created { info }
            }
        }
        (Err(SetupError::GamertagTaken), Call::Add { .. }) => {
            eprintln!("live: {tag} is on {url} already; asking for its code");
            Phase::Code { error: None }
        }
        // The first refusal for want of an invite is the question, not
        // an error.
        (Err(SetupError::NeedsInvite), Call::Add { invite: false }) => {
            Phase::Invite { error: None }
        }
        (Err(e @ (SetupError::NeedsInvite | SetupError::InviteInvalid)), Call::Add { .. }) => {
            eprintln!("live: making {tag}: {e}");
            Phase::Invite { error: Some(e) }
        }
        (Err(e), Call::Add { .. }) => {
            eprintln!("live: making {tag}: {e}");
            Phase::Gamertag { error: Some(e) }
        }
        (Err(SetupError::NoSuchGamertag), Call::Code) => {
            eprintln!("live: {tag} is not on {url}; offering to make it");
            Phase::Create { error: None }
        }
        (Err(e), Call::Create) => {
            eprintln!("live: making {tag}: {e}");
            Phase::Create { error: Some(e) }
        }
        (Err(e), Call::Code) => {
            eprintln!("live: the code for {tag}: {e}");
            Phase::Code { error: Some(e) }
        }
    }
}

/// Saves `entry` to the session's file (which the game reads) and to the
/// shared one beside it, so the next start has it too. Only the first must
/// succeed.
fn save_all(files: &[PathBuf], entry: &LocalEntry) -> Result<(), SetupError> {
    let (first, rest) = files.split_first().ok_or(SetupError::Save)?;
    account_setup::save(first, entry)?;
    for path in rest {
        if let Err(e) = account_setup::save(path, entry) {
            eprintln!(
                "live: {} not saved to {}: {e}",
                entry.gamertag,
                path.display()
            );
        }
    }
    Ok(())
}

/// Saves `gamertag` as waiting for its code ([`account_setup::save_waiting`])
/// to the session's file and the shared one; only the first must succeed.
fn save_waiting_all(files: &[PathBuf], gamertag: &str, server: &str) -> Result<(), SetupError> {
    let (first, rest) = files.split_first().ok_or(SetupError::Save)?;
    account_setup::save_waiting(first, gamertag, server)?;
    for path in rest {
        if let Err(e) = account_setup::save_waiting(path, gamertag, server) {
            eprintln!("live: {gamertag} not saved to {}: {e}", path.display());
        }
    }
    Ok(())
}

/// The files a sign-in saves to: `accounts` and, for `h2 play`'s session
/// file, the shared one beside it.
pub(crate) fn files(accounts: Option<&std::path::Path>) -> Vec<PathBuf> {
    accounts
        .map(|p| {
            std::iter::once(p.to_path_buf())
                .chain(account_setup::shared_accounts_path(p))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(error: Option<SetupError>) -> Phase {
        Phase::Code { error }
    }

    /// Typed text on each page, as a table: the page, the text, what it
    /// does.
    #[test]
    fn typed_text_on_each_page() {
        let gamertag = Phase::Gamertag { error: None };
        let invite = Phase::Invite { error: None };
        let cases: [(&Phase, Option<&str>, Option<Typed>); 7] = [
            (&gamertag, None, Some(Typed::Keep)),
            (
                &gamertag,
                Some("  Chief "),
                Some(Typed::Gamertag("Chief".into())),
            ),
            (
                &gamertag,
                Some("no/slashes"),
                Some(Typed::Phase(Phase::Gamertag {
                    error: Some(SetupError::BadGamertag),
                })),
            ),
            (&invite, None, Some(Typed::Phase(gamertag.clone()))),
            (
                &invite,
                Some(" "),
                Some(Typed::Phase(Phase::Invite {
                    error: Some(SetupError::NeedsInvite),
                })),
            ),
            (&invite, Some("ABC"), Some(Typed::Invite("ABC".into()))),
            (&Phase::Working, Some("x"), None),
        ];
        for (phase, text, want) in cases {
            assert_eq!(typed(phase, text), want, "{phase:?} {text:?}");
        }
        assert_eq!(typed(&code(None), Some("123456")), None);
    }

    #[test]
    fn a_code_not_six_digits_is_refused_without_a_request() {
        assert_eq!(refused("12345"), Some(code(Some(SetupError::WrongCode))));
        assert_eq!(refused("123456"), None);
    }

    /// Every way a server call ends, as a table.
    #[test]
    fn each_call_settles_to_its_page() {
        use SetupError as E;
        let add = Call::Add { invite: false };
        let add_inv = Call::Add { invite: true };
        let cases: [(Call, Result<Option<RecoveryInfo>, E>, Phase); 9] = [
            (Call::Code, Ok(None), Phase::Done),
            (Call::Create, Ok(None), Phase::Created { info: None }),
            (add, Err(E::GamertagTaken), code(None)),
            (add, Err(E::NeedsInvite), Phase::Invite { error: None }),
            (
                add_inv,
                Err(E::NeedsInvite),
                Phase::Invite {
                    error: Some(E::NeedsInvite),
                },
            ),
            (
                add,
                Err(E::Unreachable),
                Phase::Gamertag {
                    error: Some(E::Unreachable),
                },
            ),
            (
                Call::Code,
                Err(E::NoSuchGamertag),
                Phase::Create { error: None },
            ),
            (
                Call::Create,
                Err(E::Busy),
                Phase::Create {
                    error: Some(E::Busy),
                },
            ),
            (
                Call::Code,
                Err(E::WrongCodeLeft { left: 2 }),
                code(Some(E::WrongCodeLeft { left: 2 })),
            ),
        ];
        for (call, result, want) in cases {
            let r = format!("{result:?}");
            assert_eq!(
                settled(call, result, "Chief", "https://x"),
                want,
                "{call:?} {r}"
            );
        }
    }
}
