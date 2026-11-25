//! The Fish Audio key, from the macOS Keychain. It is read when a line is
//! asked of the service (never for a cached one), held in a [`Secret`]
//! that prints as `[key]`, sent as a bearer header, and dropped. It is
//! never logged, never put in an error message and never written to disk.

use std::fmt;
use std::process::{Command, Stdio};

/// The Keychain item's service name.
pub const SERVICE: &str = "fish-audio";

/// `security`'s exit status for "the item could not be found".
const NOT_FOUND: i32 = 44;

/// The one-time setup: the command to run in a terminal, with a
/// placeholder where the key goes.
pub const ADD_COMMAND: &str =
    "security add-generic-password -s fish-audio -a \"$USER\" -w '<your-fish-audio-key>'";

/// What Studio says when there is no key.
pub fn add_key_help() -> String {
    format!(
        "add your Fish Audio key: there is none in the Keychain (service \"{SERVICE}\"). \
         Make a key at https://fish.audio/app/api-keys/, then run this once in a terminal: \
         {ADD_COMMAND}   (leave the key off after -w and `security` asks for it, \
         which keeps it out of your shell history)"
    )
}

/// A key. Its `Debug` and `Display` say nothing of it.
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Secret {
        Secret(s.into())
    }

    /// The key itself: for the request header only.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// `text` with the key, if it is in it, taken out.
    pub fn scrub(&self, text: &str) -> String {
        if self.0.is_empty() {
            text.to_owned()
        } else {
            text.replace(&self.0, "[key]")
        }
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([key])")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[key]")
    }
}

/// Why the key was not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    Missing,
    /// Why not; never the key.
    Unreadable(String),
}

/// Where the key comes from: the Keychain, or a fake in the tests.
pub trait KeySource {
    /// The key.
    fn key(&self) -> Result<Secret, KeyError>;
    /// Whether there is one, without reading it.
    fn present(&self) -> Result<bool, KeyError>;
}

/// The macOS Keychain, through `security`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Keychain;

impl Keychain {
    fn find(secret: bool) -> Result<Vec<u8>, KeyError> {
        let mut c = Command::new("security");
        c.args(["find-generic-password", "-s", SERVICE]);
        if secret {
            c.arg("-w");
        }
        c.stdin(Stdio::null()).stderr(Stdio::null());
        c.stdout(if secret {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        match c.output() {
            Ok(o) if o.status.success() => Ok(o.stdout),
            Ok(o) if o.status.code() == Some(NOT_FOUND) => Err(KeyError::Missing),
            Ok(o) => Err(KeyError::Unreadable(format!(
                "`security find-generic-password -s {SERVICE}` exited with {}",
                o.status
                    .code()
                    .map_or_else(|| "a signal".to_owned(), |c| c.to_string())
            ))),
            Err(e) => Err(KeyError::Unreadable(format!(
                "could not run `security` ({:?}): the key is read from the macOS Keychain",
                e.kind()
            ))),
        }
    }
}

impl KeySource for Keychain {
    fn key(&self) -> Result<Secret, KeyError> {
        let out = Self::find(true)?;
        let text = String::from_utf8_lossy(&out);
        let key = text.trim();
        if key.is_empty() {
            return Err(KeyError::Missing);
        }
        Ok(Secret::new(key))
    }

    fn present(&self) -> Result<bool, KeyError> {
        match Self::find(false) {
            Ok(_) => Ok(true),
            Err(KeyError::Missing) => Ok(false),
            Err(e) => Err(e),
        }
    }
}
