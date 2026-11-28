//! **Rebuild map** and the reload after **Build AI ground**
//! (docs/game/forge-design.md, "Rebuild map"; docs/game/forge-map-variants.md,
//! "AI ground").
//!
//! A play copy is built from the document off the game thread, written as
//! `<data>/cache/forge-play/<cache name>.map`, and the running map is
//! reloaded onto it through the one reload `map.reload` also runs
//! (`agent::sim::MapReloader`). The map's new activation would
//! end the session ([`super::Driver`] drops the editor when the scenario or
//! the checksum changes), so while a reload of ours is under way the
//! driver carries the editor across it ([`Carry`]) and, once the session
//! opens on the copy, the session goes on over it with its document, log,
//! undo, the monitor's place and the tool
//! ([`super::editing::Editor::reopen`], `forge::Session::reopen`).
//!
//! Two things come from outside `game`, which may not use `tools`: the
//! build of a copy ([`install_builder`]: `forge_build` over the disc's maps,
//! from the composition root) and the reload (`agent::sim::MapReloader`,
//! the agent's, which [`super::install`] takes when the game has an
//! agent). With either missing **Rebuild map** says so by name and the
//! AI ground's notice says the running map is not changed.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use forge::CopyKind;
use forge_doc::ForgeDoc;

use super::editing::Editor;

/// What **Rebuild map** says when this game cannot build a copy.
pub const NO_BUILDER: &str = "This game cannot rebuild the map.";
/// What **Rebuild map** says when this game cannot reload the map.
pub const NO_RELOAD: &str = "This game cannot reload the map.";
/// What **Rebuild map** says when the running map is the document already.
pub const NOTHING: &str = "The map already has every edit.";
/// The notices of a reload.
pub const REBUILDING: &str = "Rebuilding the map...";
pub const REBUILT: &str = "The map is rebuilt.";
pub const GROUND_LOADED: &str = "The map has the AI ground.";

/// The build of a play copy: the map's bytes for a document.
pub trait CopyBuilder: Send {
    /// Materialise `doc` (a live form, `forge::live_form`) over this
    /// machine's maps. An `Err` is a sentence for the HUD.
    fn build(&mut self, doc: &ForgeDoc) -> Result<Vec<u8>, String>;
}

static BUILDER: Mutex<Option<Box<dyn CopyBuilder>>> = Mutex::new(None);

/// The map the last AI ground build made, for the reload after it.
static GROUND_COPY: Mutex<Option<Vec<u8>>> = Mutex::new(None);

/// Whether the game can reload the map (a reloader is installed).
/// Tests that change it, or the builder, hold `ground::TEST_GUARD`.
static RELOADS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Supply the build of a play copy, replacing any.
pub fn install_builder(builder: Box<dyn CopyBuilder>) {
    *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(builder);
}

/// The build is gone: **Rebuild map** refuses again.
pub fn uninstall_builder() {
    *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Whether a copy can be built. A build in progress holds the lock and the
/// builder is then there, so this never waits for one.
pub fn can_build() -> bool {
    match BUILDER.try_lock() {
        Ok(guard) => guard.is_some(),
        Err(std::sync::TryLockError::WouldBlock) => true,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().is_some(),
    }
}

/// Build a copy with the installed builder.
pub(super) fn build(doc: &ForgeDoc) -> Result<Vec<u8>, String> {
    let mut guard = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_mut().ok_or(NO_BUILDER)?.build(doc)
}

/// Whether this game reloads the map with what it builds.
pub fn reloads() -> bool {
    RELOADS.load(std::sync::atomic::Ordering::Acquire)
}

pub(super) fn set_reloads(yes: bool) {
    RELOADS.store(yes, std::sync::atomic::Ordering::Release);
}

/// Keep the map an AI ground build made (the app's ground builder hands
/// it here, `boot::ground_builder::AppGround::after`).
pub fn keep_ground_copy(bytes: Vec<u8>) {
    *GROUND_COPY.lock().unwrap_or_else(|e| e.into_inner()) = Some(bytes);
}

/// The map the last ground build made, once.
pub(super) fn take_ground_copy() -> Option<Vec<u8>> {
    GROUND_COPY.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Where the copy of the running map `cache` is written: named for the
/// cache, as a reload's file must be.
pub(super) fn copy_path(data: &Path, cache: &str) -> PathBuf {
    data.join("cache").join("forge-play").join(format!("{cache}.map"))
}

/// Write a copy's bytes to `path`, whole or not at all.
pub(super) fn write_copy(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("The rebuilt map was not written ({e}).");
    let dir = path.parent().ok_or("The rebuilt map has no folder.")?;
    std::fs::create_dir_all(dir).map_err(fail)?;
    let part = path.with_extension("map.part");
    std::fs::write(&part, bytes).map_err(fail)?;
    std::fs::rename(&part, path).map_err(fail)
}

/// A play copy written and waiting for the reload.
#[derive(Clone, Debug)]
pub(super) struct PlayCopy {
    pub file: PathBuf,
    /// The form it was built from, which the session reopens over.
    pub built: ForgeDoc,
    pub kind: CopyKind,
}

impl PlayCopy {
    /// What the HUD says when the session has reopened over it.
    pub fn done(&self) -> &'static str {
        match self.kind {
            CopyKind::Rebuild => REBUILT,
            CopyKind::Ground => GROUND_LOADED,
        }
    }
}

/// A reload the game is carrying out: its checksum once known, or why not.
pub(super) trait Ticket: Send {
    fn poll(&mut self) -> Option<Result<u32, String>>;
}

/// The reload of the running map onto a copy.
pub(super) trait Reloader: Send {
    fn request(
        &mut self,
        m: &dyn guest_layout::GuestMemory,
        file: &Path,
    ) -> Result<Box<dyn Ticket>, String>;
}

/// The reload `map.reload` runs, through the agent's `sim` handle.
impl Reloader for agent::sim::MapReloader {
    fn request(
        &mut self,
        m: &dyn guest_layout::GuestMemory,
        file: &Path,
    ) -> Result<Box<dyn Ticket>, String> {
        let reload = self.reload(m, file)?;
        Ok(Box::new(reload))
    }
}

impl Ticket for agent::sim::MapReload {
    fn poll(&mut self) -> Option<Result<u32, String>> {
        agent::sim::MapReload::poll(self)
    }
}

/// A reload of ours under way, and the editor it carries across the map's
/// activation.
pub(super) struct Carry {
    pub copy: PlayCopy,
    pub ticket: Option<Box<dyn Ticket>>,
    /// The copy's checksum, once the game queued the reload onto it.
    pub checksum: Option<u32>,
    /// The editor, once the running map went away under it.
    pub editor: Option<Editor>,
}

/// What the driver does with a reload as the game's map changes.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Still waiting for the game to take it.
    Wait,
    /// The game refused it: the editor goes on over the map it has, and
    /// says why.
    Refused(String),
    /// Queued onto the copy with this checksum.
    Queued(u32),
}

impl Carry {
    pub fn new(copy: PlayCopy, ticket: Box<dyn Ticket>) -> Self {
        Self {
            copy,
            ticket: Some(ticket),
            checksum: None,
            editor: None,
        }
    }

    /// Ask the reload how it stands; once it has answered, it is not asked
    /// again.
    pub fn poll(&mut self) -> Step {
        let Some(ticket) = &mut self.ticket else {
            return Step::Wait;
        };
        match ticket.poll() {
            None => Step::Wait,
            Some(Ok(checksum)) => {
                self.ticket = None;
                self.checksum = Some(checksum);
                Step::Queued(checksum)
            }
            Some(Err(why)) => {
                self.ticket = None;
                Step::Refused(why)
            }
        }
    }

    /// Whether the session opening on a map of this checksum is the one the
    /// reload was for. A reload still under way, or one onto another copy,
    /// is not.
    pub fn opens(&self, checksum: u32) -> bool {
        self.editor.is_some() && self.checksum == Some(checksum)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    struct Answer(Option<Result<u32, String>>);

    impl Ticket for Answer {
        fn poll(&mut self) -> Option<Result<u32, String>> {
            self.0.take()
        }
    }

    pub(in crate::forge_glue) fn answer(result: Option<Result<u32, String>>) -> Box<dyn Ticket> {
        Box::new(Answer(result))
    }

    fn copy() -> PlayCopy {
        PlayCopy {
            file: PathBuf::from("/tmp/court.map"),
            built: forge_doc::ForgeDoc {
                format: forge_doc::FORMAT,
                builder: forge_doc::BuilderVersions::CURRENT,
                base: forge_doc::MapRef {
                    map: "court".into(),
                    sha256: forge_doc::Sha256::of(b"an invented court"),
                },
                sources: vec![],
                title: "Court".into(),
                author: "Builder".into(),
                description: String::new(),
                recipes: vec![],
                imports: vec![],
                ops: vec![],
                expect: None,
                waves: None,
            },
            kind: CopyKind::Rebuild,
        }
    }

    #[test]
    fn a_reload_answers_once_and_a_refusal_says_why() {
        let mut carry = Carry::new(copy(), answer(None));
        assert_eq!(carry.poll(), Step::Wait);
        assert!(!carry.opens(7), "nothing queued and no editor yet");
        carry.ticket = Some(answer(Some(Ok(7))));
        assert_eq!(carry.poll(), Step::Queued(7));
        assert_eq!(carry.poll(), Step::Wait, "answered once");
        assert!(!carry.opens(7), "the map has not gone from under the editor yet");

        let mut refused = Carry::new(copy(), answer(Some(Err("busy".into()))));
        assert_eq!(refused.poll(), Step::Refused("busy".into()));
        assert!(refused.checksum.is_none());
    }

    #[test]
    fn a_copy_is_named_for_the_running_cache_and_written_whole() {
        let dir = std::env::temp_dir().join(format!("forge-play-{}", std::process::id()));
        let path = copy_path(&dir, "beavercreek");
        assert!(path.ends_with("cache/forge-play/beavercreek.map"));
        write_copy(&path, b"map bytes").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"map bytes");
        assert!(!path.with_extension("map.part").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
