//! The lobby's start gate (docs/game/rules-start-gate.md, "Where it runs"
//! and "What a blocked start shows"): the map the lobby has chosen, checked
//! against the labels of the script its variant names before Start Game.
//!
//! The party lobby's `before` on `LOBBY_VARIANT_PANEL`
//! (`crate::lobby_mode`) reads the lobby's [`Key`] ([`read`], with guest
//! memory held) and then calls [`refresh`] (without it: the census reads
//! the map file through the VFS). A changed key runs the check once and
//! keeps the verdict; the same key keeps it. The key is the lobby's map
//! (`h1_options_row::LOBBY_MAP`), the Forge map in Forge mode
//! (`lobby_start::forge_map`), and the variant record's script prefix,
//! engine and team play: the record's own block, or the store bundle's
//! that `game_variants::inject::apply_block` writes at activation.
//!
//! The census is `Census::of_file`: the map file's netgame flags and
//! starting locations, with the saved Forge map's document in Forge mode.
//! The teams are 0 and 1 in a team game (the lobby does not know which
//! teams its players will be on) and none otherwise.
//!
//! A [`Verdict::Blocked`] holds Start Game ([`blocked`], read by
//! `lobby_mode::start_refused` and the status line in `lobby_status`,
//! whose text is `lobby_mode::gate_text`)
//! except in Forge mode, where the map is what is being built. The lobby's
//! check is a courtesy: what it cannot decide (a script not on this
//! machine, a map file it cannot read, a script the host would refuse for
//! another reason) leaves the verdict clear, and the host's gate at the
//! start (`runtime::Runtime::start`) decides. A host block becomes the
//! lobby's verdict (`feature::blocked`), kept until the key changes. A
//! guest checks nothing: it runs no script, and its status line shows
//! retail's text. Without the `rules` feature no script runs, so nothing
//! is checked.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use ::rules::labels::Census;
use agent_proto::rules as wire;
use guest_layout::GuestMemory;
use forge_doc::CensusOf;
use variants::ext::{self, Ext, Mode};

use crate::lobby_start;

/// The teams a team game's lobby checks `per team` labels for.
pub const TEAMS: &[u8] = &[0, 1];
/// In a variant record: the name field (`+4`, 0x40 bytes), the engine
/// index (`+0x44`) and the flags (`+0x48`, bit 0 team play).
pub const RECORD_NAME: usize = 4;
pub const RECORD_NAME_LEN: usize = 0x40;
pub const RECORD_ENGINE: usize = 0x44;
pub const RECORD_FLAGS: usize = 0x48;
/// The record's bytes the key reads: up to the end of its block.
pub const RECORD_LEN: usize = ext::AT + ext::LEN;

/// What the lobby's check depends on: a change to any runs it again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Key {
    /// The lobby's map id (`h1_options_row::LOBBY_MAP`).
    pub map: u32,
    /// The saved Forge map applied to the census: Forge mode's choice,
    /// none outside Forge mode and for a new map.
    pub forge_map: Option<String>,
    /// The script prefix the game would run; none when the record names
    /// no script.
    pub script: Option<[u8; 8]>,
    /// The record's engine index.
    pub engine: i32,
    pub team_play: bool,
}

/// The lobby as the gate reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    /// This machine starts the game: not networked (Split Screen), or the
    /// party's leader.
    pub leads: bool,
    pub key: Key,
}

/// What the lobby's check decided.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Verdict {
    #[default]
    Clear,
    /// Every problem the check found, in its order.
    Blocked(Vec<String>),
}

/// Where the gate finds a census and a blob. In the game, [`InGame`].
pub trait Sources: Send + Sync {
    /// The census of the lobby's map `map` with the saved Forge map
    /// `forge_map`'s document applied, else the built-in layout of `script`
    /// (`census::layout_script`) on the map, if it has one.
    fn census(&self, map: u32, forge_map: Option<&str>, script: Option<&str>) -> Result<Census, String>;
    /// The blob on this machine whose hash starts with `prefix`.
    fn blob(&self, prefix: [u8; 8]) -> Option<Vec<u8>>;
}

/// The lobby's gate: the last key and its verdict.
#[derive(Debug, Default)]
pub struct Gate {
    key: Option<Key>,
    verdict: Verdict,
    /// The host block last adopted, so it is adopted once.
    host_seen: Option<Vec<String>>,
}

impl Gate {
    pub fn verdict(&self) -> &Verdict {
        &self.verdict
    }

    /// The problems while the verdict blocks.
    pub fn problems(&self) -> Option<&[String]> {
        match &self.verdict {
            Verdict::Blocked(p) => Some(p),
            Verdict::Clear => None,
        }
    }

    /// The problems that hold Start Game in the lobby `state`: none in
    /// Forge mode, and none outside a lobby (the level boot's game).
    pub fn held(&self, state: &lobby_start::State) -> Option<&[String]> {
        self.problems()
            .filter(|_| state.lobby && state.mode != lobby_start::Mode::Forge)
    }

    /// Forgets the key and the verdict: the next refresh checks again.
    pub fn clear(&mut self) {
        *self = Gate::default();
    }

    /// The lobby now (`None`: it cannot be read), the host's block
    /// (`feature::blocked`) and the sources. A guest's lobby clears the
    /// verdict. A host block not yet seen becomes the verdict, kept for
    /// this key. Otherwise a changed key runs the check once.
    pub fn refresh(&mut self, input: Option<Input>, host: Option<Vec<String>>, sources: &dyn Sources) {
        let Some(input) = input.filter(|i| i.leads) else {
            self.clear();
            return;
        };
        if host != self.host_seen {
            self.host_seen = host.clone();
            if let Some(problems) = host {
                self.key = Some(input.key);
                self.verdict = Verdict::Blocked(problems);
                return;
            }
        }
        if self.key.as_ref() == Some(&input.key) {
            return;
        }
        self.verdict = check(&input.key, sources);
        if let Verdict::Blocked(p) = &self.verdict {
            eprintln!(
                "rules: the lobby's map does not have what the script needs, Start Game is held: {}",
                p.join("; ")
            );
        }
        self.key = Some(input.key);
    }
}

/// The check for `key`. Clear when the key names no script, or what the
/// lobby cannot decide: a blob not on this machine or that does not
/// verify, a script for another engine (the host refuses it and the
/// carrier's rules play), a script with no labels or tags, or a census the
/// lobby cannot read.
pub fn check(key: &Key, sources: &dyn Sources) -> Verdict {
    let Some(prefix) = key.script else {
        return Verdict::Clear;
    };
    let Some(blob) = sources.blob(prefix) else {
        return Verdict::Clear;
    };
    let Ok((ir, _)) = ::rules::verify::load(&blob) else {
        return Verdict::Clear;
    };
    let matches_engine = key.engine == variants::record::SCRIPT_INDEX
        || key.engine == i32::from(ir.header.engine());
    if !matches_engine || (ir.labels.is_empty() && ir.tags.is_empty()) {
        return Verdict::Clear;
    }
    let siege = is_siege(prefix);
    let census = match sources.census(key.map, key.forge_map.as_deref(), super::census::layout_script(&ir)) {
        Ok(c) => c,
        Err(e) if siege => return Verdict::Blocked(vec![format!("Siege cannot read this map's objectives: {e}")]),
        Err(e) => {
            eprintln!("rules: the lobby's map census could not be read, the host checks at the start: {e}");
            return Verdict::Clear;
        }
    };
    let teams: &[u8] = if key.team_play { TEAMS } else { &[] };
    let problems = map_problems(&ir, &census, teams, siege);
    if problems.is_empty() {
        Verdict::Clear
    } else {
        Verdict::Blocked(problems)
    }
}

// Siege's script accepts a site OR a final stand. Its individual labels
// are optional because a map does not need both. Without either, its
// round_start ends the round immediately.
fn is_siege(prefix: [u8; 8]) -> bool {
    crate::community_variants::listed_named("Siege").is_some_and(|l| l.ext().script.prefix == prefix)
}

fn map_problems(ir: &::rules::Ir, census: &Census, teams: &[u8], siege: bool) -> Vec<String> {
    let mut problems: Vec<_> = ::rules::labels::check(ir, census, teams)
        .into_iter().map(|problem| problem.text).collect();
    if siege && !siege_objectives(census) {
        problems.push("Siege needs a territory, a Siege site or a defending flag stand.".into());
    }
    problems
}

/// Whether the map's census has what Siege plays: a Siege site, a
/// territory, a Siege goal (the final stand) or the defending team's flag
/// stand. A map with none ends Siege's first round at once
/// (`content/modes/rules/siege.rules`, `round_start`).
pub fn siege_objectives(census: &Census) -> bool {
    let zones = |label: &str, team: Option<u8>| {
        census.labels.get(label).is_some_and(|members| members.iter().any(|m|
            m.class.satisfies(::rules::ir::LabelKind::Zone) && team.is_none_or(|t| m.team == Some(t))
        ))
    };
    zones("siege_site", None) || zones("territory", None)
        || zones("siege_goal", None) || zones("ctf_flag_spawn", Some(1))
}

static SIEGE_MAPS: Mutex<std::collections::BTreeMap<u32, bool>> = Mutex::new(std::collections::BTreeMap::new());
/// By map, variant file and the revision its caller names (the document's
/// id): a saved variant changes under its file name.
static SIEGE_VARIANTS: Mutex<std::collections::BTreeMap<(u32, String, String), bool>> =
    Mutex::new(std::collections::BTreeMap::new());

/// Forgets which maps have Siege's objectives: the catalogue changed.
pub fn forget_siege_maps() {
    SIEGE_MAPS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    SIEGE_VARIANTS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Whether the map `map` with the edits of its saved variant `file` has
/// Siege's objectives. `None` when it cannot be told.
fn siege_census(map: u32, file: Option<&str>) -> Option<bool> {
    let sources = SOURCES.get()?.as_ref();
    let prefix = crate::community_variants::listed_named("Siege")?.ext().script.prefix;
    let (ir, _) = ::rules::verify::load(&sources.blob(prefix)?).ok()?;
    let census = sources.census(map, file, super::census::layout_script(&ir)).ok()?;
    Some(siege_objectives(&census))
}

/// Whether the map `map` (a list id) played as its saved variant `file`
/// has Siege's objectives ([`siege_objectives`] over the census of the map
/// with the variant's edits). `None` when it cannot be told. The answer is
/// kept per map, file and `revision` until [`forget_siege_maps`].
pub fn siege_supports_variant(map: u32, file: &str, revision: &str) -> Option<bool> {
    let key = (map, file.to_owned(), revision.to_owned());
    if let Some(known) = SIEGE_VARIANTS.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some(*known);
    }
    let has = siege_census(map, Some(file))?;
    SIEGE_VARIANTS.lock().unwrap_or_else(|e| e.into_inner()).insert(key, has);
    Some(has)
}

/// Whether the map `map` (a list id) has Siege's objectives
/// ([`siege_objectives`], over the census with Siege's built-in layout).
/// `None` when it cannot be told: Siege's script is not on this machine or
/// the map file cannot be read. The answer is kept per map until
/// [`forget_siege_maps`].
pub fn siege_supports(map: u32) -> Option<bool> {
    if let Some(known) = SIEGE_MAPS.lock().unwrap_or_else(|e| e.into_inner()).get(&map) {
        return Some(*known);
    }
    let has = siege_census(map, None)?;
    SIEGE_MAPS.lock().unwrap_or_else(|e| e.into_inner()).insert(map, has);
    Some(has)
}

/// Whether `record` is the Siege game type.
pub fn is_siege_record(record: &variants::GameVariant) -> bool {
    script_of(record.ext_block(), record.game_type().engine()).is_some_and(is_siege)
}

/// Maps offered for an explicit script use the same label and team requirements
/// as Start Game. Read the blob once when opening the picker.
pub fn map_filter(record: &variants::GameVariant) -> Option<impl Fn(u32) -> bool> {
    map_filter_using(record, SOURCES.get()?.as_ref())
}

pub(crate) fn map_filter_using<'a>(
    record: &variants::GameVariant,
    sources: &'a dyn Sources,
) -> Option<impl Fn(u32) -> bool + use<'a>> {
    let prefix = script_of(record.ext_block(), record.game_type().engine())?;
    let (ir, _) = ::rules::verify::load(&sources.blob(prefix)?).ok()?;
    if ir.labels.is_empty() && ir.tags.is_empty() {
        return None;
    }
    let teams = if record.team_play() { TEAMS } else { &[] };
    let siege = is_siege(prefix);
    let layout = super::census::layout_script(&ir);
    Some(move |id| sources.census(id, None, layout).is_ok_and(|census|
        map_problems(&ir, &census, teams, siege).is_empty()
    ))
}

/// The script prefix a record's block names: a `Rules` block's prefix, or
/// for a record with no mode on a ported engine (`runtime::PORTED`), its
/// built-in's.
pub fn script_of(block: &[u8; ext::LEN], engine: i32) -> Option<[u8; 8]> {
    let script = Ext::in_effect(block).script;
    match script.mode {
        Mode::Rules => Some(script.prefix),
        Mode::None => {
            let e = u8::try_from(engine).ok().filter(|e| super::runtime::PORTED.contains(e))?;
            let b = super::runtime::builtins().iter().find(|b| b.engine == e)?;
            let mut p = [0; 8];
            p.copy_from_slice(&b.id.0[..8]);
            Some(p)
        }
        _ => None,
    }
}

/// The key of a variant record's bytes (`record`, [`RECORD_LEN`] long) on
/// map `map`, with `forge_map`. The block is the store bundle's when the
/// record is built from one of our entries
/// (`game_variants::inject::bundle_block`), else its own.
pub fn key_of(map: u32, forge_map: Option<String>, record: Option<&[u8; RECORD_LEN]>) -> Key {
    let Some(r) = record else {
        return Key {
            map,
            forge_map,
            ..Key::default()
        };
    };
    let word = |at: usize| u32::from_le_bytes(r[at..at + 4].try_into().unwrap());
    let engine = word(RECORD_ENGINE) as i32;
    let name = &r[RECORD_NAME..RECORD_NAME + RECORD_NAME_LEN];
    let own: [u8; ext::LEN] = r[ext::AT..ext::AT + ext::LEN].try_into().unwrap();
    let block = crate::game_variants::inject::bundle_block(name, engine).unwrap_or(own);
    Key {
        map,
        forge_map,
        script: script_of(&block, engine),
        engine,
        team_play: word(RECORD_FLAGS) & super::guest::TEAM_PLAY != 0,
    }
}

/// Whether this machine starts the lobby's game: not networked, or the
/// party's leader. `None` while the lobby cannot be read.
pub fn leads(m: &dyn GuestMemory) -> Option<bool> {
    use bots::lobby;
    if m.u8(lobby::NETWORKING).ok()? == 0 {
        return Some(true);
    }
    Some(lobby::facts(m).ok()?.leader)
}

/// The lobby now: its leadership, its map, the lobby's Forge map in Forge
/// mode and the variant record at `variant` (the panel update's argument;
/// 0 is none). `None` while it cannot be read.
pub fn read(m: &dyn GuestMemory, variant: u32) -> Option<Input> {
    let leads = leads(m)?;
    let map = m.u32(crate::h1_options_row::LOBBY_MAP).ok()?;
    let state = lobby_start::state();
    let forge_map = (state.mode == lobby_start::Mode::Forge)
        .then_some(state.forge_map)
        .flatten();
    let record = if variant == 0 {
        None
    } else {
        let mut r = [0u8; RECORD_LEN];
        m.read(variant, &mut r).ok()?;
        Some(r)
    };
    Some(Input {
        leads,
        key: key_of(map, forge_map, record.as_ref()),
    })
}

/// `rules.state` in the lobby: when no script runs and the runtime has no
/// block of its own, the lobby's problems as `blocked`, and why.
pub fn annotate(s: &mut wire::State) {
    if s.running || !s.blocked.is_empty() {
        return;
    }
    let Some(problems) = with_lobby(|g| g.problems().map(<[String]>::to_vec)) else {
        return;
    };
    s.refused = Some(if lobby_start::mode() == lobby_start::Mode::Forge {
        "the lobby's map does not have what the script needs; in a Forge session the script does not run until it does".into()
    } else {
        "the lobby's map does not have what the script needs; Start Game is held".into()
    });
    s.blocked = problems;
}

// ---- the lobby's gate ----

#[cfg(not(test))]
static GATE: Mutex<Gate> = Mutex::new(Gate {
    key: None,
    verdict: Verdict::Clear,
    host_seen: None,
});

#[cfg(not(test))]
/// The lobby's gate.
pub fn with_lobby<T>(f: impl FnOnce(&mut Gate) -> T) -> T {
    f(&mut GATE.lock().unwrap_or_else(|e| e.into_inner()))
}

// Tests run side by side, each on its own thread: each has its own lobby,
// as `lobby_start`'s.
#[cfg(test)]
thread_local! {
    static GATE: std::cell::RefCell<Gate> = std::cell::RefCell::new(Gate::default());
}

#[cfg(test)]
/// The lobby's gate (this test thread's).
pub fn with_lobby<T>(f: impl FnOnce(&mut Gate) -> T) -> T {
    GATE.with(|g| f(&mut g.borrow_mut()))
}

static SOURCES: OnceLock<Box<dyn Sources>> = OnceLock::new();

/// Sets the game's sources once; later calls keep the first.
pub fn install(sources: Box<dyn Sources>) {
    let _ = SOURCES.set(sources);
}

/// Refreshes the lobby's verdict for `input` ([`read`]). Call it with no
/// guest memory held: the census reads the map file through the VFS.
pub fn refresh(input: Option<Input>) {
    let Some(sources) = SOURCES.get() else {
        return;
    };
    if !super::feature::installed() {
        clear();
        return;
    }
    let host = super::feature::blocked();
    with_lobby(|g| g.refresh(input, host, &**sources));
}

/// The problems that hold the lobby's Start Game now ([`Gate::held`]).
pub fn blocked() -> Option<Vec<String>> {
    let state = lobby_start::state();
    with_lobby(|g| g.held(&state).map(<[String]>::to_vec))
}

/// The lobby was left, or a snapshot loaded: the next refresh checks again.
pub fn clear() {
    with_lobby(Gate::clear);
}

/// The game's sources: the map file through the VFS, the saved Forge maps
/// under the data directory, and the blobs compiled in or in the
/// game-variant store (`feature::local_blob`).
pub struct InGame {
    pub vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    pub data: PathBuf,
}

impl Sources for InGame {
    fn census(&self, map: u32, forge_map: Option<&str>, script: Option<&str>) -> Result<Census, String> {
        let path = crate::map_select::selected_path(map)
            .ok_or_else(|| format!("map {map} is not in the catalogue"))?;
        let vfs = self.vfs.as_ref().ok_or("no map files are mounted")?;
        let doc = forge_map
            .map(|file| {
                let base = crate::map_select::short_name_of(map)
                    .ok_or_else(|| format!("map {map} is not in the catalogue"))?;
                let installed = crate::map_variant::installed(&base);
                forge::files::variants::load(&self.data, &base, file, installed.as_ref())
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        let file = crate::bots_nav_auto::VfsFile::open(Arc::clone(vfs), &path)
            .map_err(|e| format!("{path}: {e}"))?;
        let map_id = map;
        let mut map = map::Map::from_reader(std::io::BufReader::new(file))
            .map_err(|e| format!("{path}: {e}"))?;
        // A Forge map's own document is the author's; without one, the
        // script's built-in layout is what the match reads
        // (`census::things_for`).
        let rows = forge_doc::rows_of_file(&mut map).map_err(|e| format!("{path}: {e}"))?;
        let layout = match doc {
            Some(_) => None,
            None => crate::map_select::short_name_of(map_id)
                .and_then(|name| super::census::layout_doc(script, &name, &rows.counts)),
        };
        let mut census = Census::of_game(&rows, doc.as_ref().or(layout.as_ref())).map_err(|e| format!("{path}: {e}"))?;
        census.sounds = rows.sounds.clone();
        Ok(census)
    }

    fn blob(&self, prefix: [u8; 8]) -> Option<Vec<u8>> {
        super::feature::local_blob(prefix)
    }
}
