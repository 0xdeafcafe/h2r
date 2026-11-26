//! Extension records (docs/game/film-format.md, "The film file"): input a
//! feature of ours gives the simulation from outside the update, written
//! into the film on the tick the feature applied it and handed back to the
//! feature before that tick in playback. The film file keeps each as an
//! extension block (`film::file::FilmWriter::extension`) with the feature's
//! tag.
//!
//! - **Recording**: a feature calls [`Session::record_extension`] (or, for
//!   the Forge session, leaves its records for [`outside`] to take) as it
//!   applies the input. The recorder writes the records of a game tick
//!   before the tick's first film tick (`record.rs`, `post_tick`).
//! - **Playback**: the player reads the records of each film tick when the
//!   tick before it ends ([`super::film_play::tick_done`]), or at the game's
//!   first update for the first tick, and [`deliver`] routes each to its
//!   feature, which applies it in the game tick that follows.
//! - **A change with no record** ends the film at the tick before it
//!   ([`Session::outside_change`]): an editor write or an agent API write,
//!   and a record that cannot be written.
//!
//! A film with a tag this build has no feature for does not play: the
//! player names the feature ([`Unknown`]).
use super::Session;
use film::Tag;

/// A shared Forge session: the welcome that opens it, or one stream item
/// (`forge::net::film`).
pub const FORGE: Tag = *b"FRGE";

/// The feature a tag is for, if this build has it.
pub fn feature(tag: Tag) -> Option<&'static str> {
    (tag == FORGE).then_some("a shared Forge session")
}

/// A film that holds records for a feature this build does not have.
#[derive(Debug, PartialEq, Eq)]
pub struct Unknown(pub Tag);

impl std::fmt::Display for Unknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The film holds records of a feature this build does not have ({})",
            String::from_utf8_lossy(&self.0)
        )
    }
}

impl std::error::Error for Unknown {}

/// Whether this build can play every feature the film's records name.
pub fn check(tags: &[Tag]) -> Result<(), Unknown> {
    match tags.iter().find(|tag| feature(**tag).is_none()) {
        Some(&tag) => Err(Unknown(tag)),
        None => Ok(()),
    }
}

/// The records features have left for the film since the last call, in the
/// order applied. The Forge session leaves its edits with its own glue
/// (`forge_glue::net::take_edits`).
pub(super) fn outside() -> Vec<(Tag, Vec<u8>)> {
    crate::forge_glue::net::take_edits()
        .into_iter()
        .map(|payload| (FORGE, payload))
        .collect()
}

/// Hand a film's record to its feature, which applies it in the game tick
/// that follows. The feature queues it; nothing is applied here.
pub(super) fn deliver(tag: Tag, bytes: Vec<u8>) -> Result<(), Unknown> {
    if tag == FORGE {
        crate::forge_glue::net::deliver_played(bytes);
        return Ok(());
    }
    Err(Unknown(tag))
}

/// What a film's start or a restart leaves behind: records handed over and
/// not applied.
pub(super) fn clear() {
    crate::forge_glue::net::take_played();
}

impl Session {
    /// A feature applied outside input to the game in this game tick: the
    /// film keeps `bytes` as the feature's record (`tag`) at the tick's
    /// first update, and playback hands it back before that tick. A record
    /// that cannot be kept (it is too large, or the film holds too many)
    /// ends the film at the tick before. Nothing is kept for a film being
    /// played or when no game is in play.
    pub fn record_extension(&self, tag: Tag, bytes: Vec<u8>) {
        let mut s = self.lock();
        if s.complete || s.failure.is_some() {
            return;
        }
        if let super::Mode::Record(r) = &mut s.mode {
            r.frame.extensions.push((tag, bytes));
        }
    }
}
