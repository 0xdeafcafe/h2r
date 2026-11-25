//! The parity arm of entry I: a [`RayWalk`] compared with what the guest
//! did, while the guest's own query stays the answer.
//!
//! The guest is observed at each call of the interpreter (its frame, pc
//! and segment) and, where the terminal callbacks can be seen, at each key
//! it accepts. Each observation pulls the walk's next event with the
//! fraction the VM holds at that moment, and the two must be equal.
//!
//! The fraction only changes inside a terminal's child callback. With keys
//! observed, every such change happens between two observations, so the
//! fraction read at the next one is the one every return site in between
//! read, and the check follows queries that hit. Without keys a terminal
//! can pass unseen: the check then follows the query only while the
//! fraction stays where it was (collectors only lower it), and calls the
//! rest [`Unverified::Fraction`].
use super::{Event, RayWalk};
use crate::Bail;
use std::fmt;

/// Why a query was not compared to the end. Nothing is wrong with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unverified {
    /// The walk could not reproduce the query exactly (or the setup's
    /// environment is one it does not model).
    Bail(Bail),
    /// The fraction moved where no key was observed.
    Fraction,
}

/// How one query compared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every call (and every key, where observed) matched, and both ended
    /// together.
    Matched,
    Unverified(Unverified),
    /// The first difference: the observation's index, what the guest did
    /// (`None`: it had returned) and what the walk did (`None`: it had
    /// ended).
    Mismatch {
        at: u32,
        guest: Option<Event>,
        native: Option<Event>,
        what: &'static str,
        setup_byte: Option<(usize, u8, u8)>,
    },
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let side = |e: &Option<Event>| e.map_or("returned".to_owned(), |e| e.to_string());
        match self {
            Outcome::Matched => f.write_str("matched"),
            Outcome::Unverified(Unverified::Bail(why)) => write!(f, "unverified: {}", why.name()),
            Outcome::Unverified(Unverified::Fraction) => f.write_str("unverified: fraction"),
            Outcome::Mismatch {
                what,
                setup_byte: Some((offset, expected, actual)),
                ..
            } => write!(
                f,
                "{what} at VM+{offset:#x}: expected {expected:#04x}, guest {actual:#04x}"
            ),
            Outcome::Mismatch {
                at,
                guest,
                native,
                what,
                ..
            } => write!(
                f,
                "{what} at observation {at}: guest {}; native {}",
                side(guest),
                side(native)
            ),
        }
    }
}

/// One query under comparison.
#[derive(Clone, Debug)]
pub struct RayCheck<B> {
    walk: RayWalk<B>,
    keys: bool,
    fraction: u32,
    seen: u32,
    done: Option<Outcome>,
}

impl<B: AsRef<[u8]>> RayCheck<B> {
    /// `keys`: the guest's accepted keys are observed too. `fraction`: the
    /// VM's fraction after the setup.
    pub fn new(walk: RayWalk<B>, keys: bool, fraction: u32) -> Self {
        Self {
            walk,
            keys,
            fraction,
            seen: 0,
            done: None,
        }
    }

    /// The check has its outcome; later observations change nothing.
    pub fn decided(&self) -> bool {
        self.done.is_some()
    }

    /// A guest word the setup wrote, against the walk's value.
    pub fn expect(&mut self, what: &'static str, native: u32, guest: u32) {
        if self.done.is_none() && native != guest {
            self.done = Some(Outcome::Mismatch {
                at: self.seen,
                guest: None,
                native: None,
                what,
                setup_byte: None,
            });
        }
    }

    /// First differing byte of the VM as seen by the root interpreter.
    pub fn expect_byte(&mut self, offset: usize, expected: u8, actual: u8) {
        if self.done.is_none() && expected != actual {
            self.done = Some(Outcome::Mismatch {
                at: self.seen,
                guest: None,
                native: None,
                what: "ray VM setup",
                setup_byte: Some((offset, expected, actual)),
            });
        }
    }

    /// The guest's side of an observation could not be read: the query
    /// stays unverified.
    pub fn decline(&mut self, why: Bail) {
        if self.done.is_none() {
            self.done = Some(Outcome::Unverified(Unverified::Bail(why)));
        }
    }

    /// The guest entered the interpreter or accepted a key, with `fraction`
    /// in the VM.
    pub fn observe(&mut self, fraction: u32, guest: Event) {
        if self.done.is_some() {
            return;
        }
        if !self.keys && fraction != self.fraction {
            self.done = Some(Outcome::Unverified(Unverified::Fraction));
            return;
        }
        self.fraction = fraction;
        match self.native(fraction) {
            Err(why) => self.done = Some(Outcome::Unverified(Unverified::Bail(why))),
            Ok(Some(native)) if native == guest => self.seen += 1,
            Ok(native) => {
                self.done = Some(Outcome::Mismatch {
                    at: self.seen,
                    guest: Some(guest),
                    native,
                    what: "different step",
                    setup_byte: None,
                })
            }
        }
    }

    /// The guest query returned with `fraction` in the VM.
    pub fn finish(mut self, fraction: u32) -> Outcome {
        if let Some(done) = self.done.take() {
            return done;
        }
        if !self.keys && fraction != self.fraction {
            return Outcome::Unverified(Unverified::Fraction);
        }
        match self.native(fraction) {
            Err(why) => Outcome::Unverified(Unverified::Bail(why)),
            Ok(None) => Outcome::Matched,
            Ok(native) => Outcome::Mismatch {
                at: self.seen,
                guest: None,
                native,
                what: "native walk goes on",
                setup_byte: None,
            },
        }
    }

    /// The walk's next event the guest is watched for.
    fn native(&mut self, fraction: u32) -> Result<Option<Event>, Bail> {
        loop {
            match self.walk.next(fraction)? {
                Some(Event::Key(_)) if !self.keys => {}
                other => return Ok(other),
            }
        }
    }
}
