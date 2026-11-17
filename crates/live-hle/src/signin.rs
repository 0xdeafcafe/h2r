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

/// Which call a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
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
        let text = text.map(str::trim);
        match (&self.prompt.phase, text) {
            (Phase::Gamertag { .. }, None) => {}
            (Phase::Gamertag { .. }, Some(tag)) => {
                if live_proto::Gamertag::new(tag).is_err() {
                    self.prompt.phase = Phase::Gamertag {
                        error: Some(SetupError::BadGamertag),
                    };
                } else {
                    self.prompt.gamertag = tag.to_string();
                    self.invite = None;
                    self.add();
                }
            }
            (Phase::Invite { .. }, None) => {
                self.prompt.phase = Phase::Gamertag { error: None };
            }
            (Phase::Invite { .. }, Some("")) => {
                self.prompt.phase = Phase::Invite {
                    error: Some(SetupError::NeedsInvite),
                };
            }
            (Phase::Invite { .. }, Some(invite)) => {
                self.invite = Some(invite.to_string());
                self.add();
            }
            _ => return false,
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
        if live_proto::recovery::parse_code(code).is_none() {
            self.prompt.phase = Phase::Code {
                error: Some(SetupError::WrongCode),
            };
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
        let tag = &self.prompt.gamertag;
        self.prompt.phase = match (result, call) {
            (Ok((entry, info)), _) => {
                eprintln!(
                    "live: {tag} verified on {}; signing in",
                    self.prompt.server_url
                );
                self.verified = Some(entry);
                if call == Call::Code {
                    Phase::Done
                } else {
                    Phase::Created { info }
                }
            }
            (Err(SetupError::GamertagTaken), Call::Add { .. }) => {
                eprintln!(
                    "live: {tag} is on {} already; asking for its code",
                    self.prompt.server_url
                );
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
                eprintln!(
                    "live: {tag} is not on {}; offering to make it",
                    self.prompt.server_url
                );
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
        };
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
