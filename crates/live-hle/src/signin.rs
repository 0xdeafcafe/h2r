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

use crate::account_setup::{self, Job, LocalEntry, SetupError};
use live_proto::recovery::RecoveryInfo;
use std::path::PathBuf;

/// What the prompt shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
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

/// One sign-in waiting for a code. Owned by the HLE state; the logon task
/// `handle` is pending while it lives.
pub(crate) struct Signin {
    pub handle: u32,
    pub slot: usize,
    prompt: Prompt,
    /// The session's accounts file first, then the shared one.
    files: Vec<PathBuf>,
    job: Option<(Job<Outcome>, bool)>,
    verified: Option<LocalEntry>,
    opened: bool,
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
        }
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
            false,
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
            true,
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
        let (_, creating) = self.job.take().expect("polled");
        if self.prompt.phase == Phase::Cancelled {
            return;
        }
        let tag = &self.prompt.gamertag;
        self.prompt.phase = match (result, creating) {
            (Ok((entry, info)), _) => {
                eprintln!(
                    "live: {tag} verified on {}; signing in",
                    self.prompt.server_url
                );
                self.verified = Some(entry);
                if creating {
                    Phase::Created { info }
                } else {
                    Phase::Done
                }
            }
            (Err(SetupError::NoSuchGamertag), false) => {
                eprintln!(
                    "live: {tag} is not on {}; offering to make it",
                    self.prompt.server_url
                );
                Phase::Create { error: None }
            }
            (Err(e), true) => {
                eprintln!("live: making {tag}: {e}");
                Phase::Create { error: Some(e) }
            }
            (Err(e), false) => {
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
