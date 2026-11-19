//! Conversations expressed with retail Halo 2 scripts and per-voice playback.
//! The binder runs synchronously. Only the conversation's worker sleeps.

use super::*;
use h1ai::conversations::{Conversation, Participant, SpeakerScopes};
use hs::h1_conversation::Op;
use std::collections::HashSet;

mod emit;
mod playback;
mod staging;
mod voices;

use emit::*;

const OBJECT_LIST: u16 = 31;
const SOUND: u16 = 32;
const AI: u16 = 19;
const OBJECT: u16 = 50;
const OBJECT_NAME: u16 = 56;

const TRYING: i32 = 1;
const READY: i32 = 2;
const PLAYING: i32 = 3;
const HOLD: i32 = 4;
const FAILED: i32 = 5;
const DONE: i32 = 6;
const ABORTED: i32 = 7;
const NO_LINE: i32 = 999;

// The actor pool has 0x100 slots (Halo 2 0x001dfb09). Batches and chunks
// scan them without an unbounded recursive HaloScript stack.
const SCAN_BATCH: usize = 8;
const SCAN_CHUNKS: usize = 4;

#[derive(Clone)]
struct Alias {
    unit: Global,
}

#[derive(Clone)]
struct Role {
    unit: Global,
    voice: Global,
    voices: u16,
}

struct CastScratch {
    actors: Global,
    candidate: Global,
    rank: Global,
    voice: Global,
    profile_base: u32,
    best: Global,
    count: Global,
    cursor: Global,
    scope_cursor: Global,
}

#[derive(Clone)]
struct Entry {
    start: Script,
    stop: Script,
    status: Global,
    line: Global,
    advance: Script,
}

#[derive(Default)]
pub(super) struct Lowering {
    entries: Vec<Entry>,
    names: Vec<String>,
    aliases: HashMap<String, Alias>,
}

impl Lowering {
    pub(super) fn has_alias(&self, name: &str) -> bool {
        self.aliases.contains_key(&name.to_ascii_lowercase())
    }
}

pub(super) fn build(
    c: &mut Compiler<'_>,
    scenes: &[Conversation],
    scopes: &SpeakerScopes,
) -> Result<Lowering> {
    let persistent = c.t.scripts.iter().filter(|s| s.kind <= 2).count();
    if persistent + scenes.len() >= 0x140 {
        return Err(invalid(
            "conversation workers exceed the Halo 2 thread pool",
        ));
    }
    let has_cast = scenes
        .iter()
        .flat_map(|s| &s.participants)
        .any(|p| !p.radio());
    let voice_metadata = has_cast || !scopes.voice_definitions.is_empty();
    if voice_metadata {
        c.out.strings.resize(voices::prefix_len(scopes), 0);
    }
    let mut program = Program::new(c);
    let profile_base = if voice_metadata {
        voices::build(&mut program, scopes)?
    } else {
        0
    };
    if scenes.is_empty() {
        return Ok(Lowering::default());
    }
    for participant in scenes.iter().flat_map(|scene| &scene.participants) {
        if !participant.radio()
            && let Some(name) = &participant.assigned_name
        {
            program.add_alias(name)?;
        }
    }
    let scratch = CastScratch {
        actors: program.global("actors", Expr::None(OBJECT_LIST))?,
        candidate: program.global("candidate", Expr::None(OBJECT))?,
        rank: program.global("rank", Expr::Real(f32::MIN))?,
        voice: program.global("candidate_voice", short(-1))?,
        profile_base,
        best: {
            program.global("voice_table", long(profile_base))?;
            program.global("best", Expr::Real(f32::MIN))?
        },
        count: program.global("count", short(0))?,
        cursor: program.global("cursor", short(0))?,
        scope_cursor: program.global("scope", short(0))?,
    };
    let mut entries = Vec::with_capacity(scenes.len());
    for scene in scenes {
        entries.push(Scene::new(&mut program, scene, scopes, &scratch)?.build()?);
    }
    Ok(Lowering {
        entries,
        names: scenes.iter().map(|s| s.name.clone()).collect(),
        aliases: program.aliases,
    })
}

/// Keep every source voice in the script tag references, including
/// alternate recordings with no authored cast using that slot.
pub(super) fn append_sound_references(out: &mut Compiled, scenes: &[Conversation]) {
    let sound_group = h1map::Group::new(b"snd!");
    let mut seen: HashSet<_> = out
        .references
        .iter()
        .filter(|(group, _)| *group == sound_group)
        .map(|(_, name)| name.to_ascii_lowercase())
        .collect();
    for line in scenes.iter().flat_map(|scene| &scene.lines) {
        for name in line.sounds.iter().flatten() {
            if seen.insert(name.to_ascii_lowercase()) {
                out.references.push((sound_group, name.clone()));
            }
        }
    }
}

/// Select the real conversation lowering before the generic missing-builtin
/// policy. Atoms on Xbox name the conversation by its scenario-block index.
pub(super) fn translate(
    c: &mut Compiler<'_>,
    builtin: &str,
    args: &[Datum],
    expected: u16,
) -> Result<Option<Datum>> {
    if !matches!(
        builtin,
        "ai_conversation"
            | "ai_conversation_stop"
            | "ai_conversation_status"
            | "ai_conversation_line"
            | "ai_conversation_advance"
    ) || c.conversations.is_none()
    {
        return Ok(None);
    }
    let lowering = c.conversations.as_ref().expect("checked above");
    let &[argument] = args else {
        return Err(invalid(format!(
            "{builtin}: expected one conversation argument"
        )));
    };
    let arg = c.t.node(argument)?;
    if !arg.atom() || arg.global() || c.h1.type_name(arg.opcode())? != "conversation" {
        return Err(invalid(format!(
            "{builtin}: expected a scenario conversation literal"
        )));
    }
    let index = arg.data() as u16;
    let expr = if index == u16::MAX {
        // Halo 1 rejects starting none. Its inactive queries return 0/999.
        match builtin {
            "ai_conversation" => boolean(false),
            "ai_conversation_status" => short(0),
            "ai_conversation_line" => short(NO_LINE),
            _ => Expr::Number(h2t::VOID, 0),
        }
    } else {
        let scene = lowering
            .entries
            .get(index as usize)
            .ok_or_else(|| invalid(format!("{builtin}: bad conversation index {index}")))?;
        let name = c.t.string(arg.string_offset())?;
        if !lowering.names[index as usize].eq_ignore_ascii_case(name) {
            return Err(invalid(format!(
                "{builtin}: conversation {index} is named {name} in the script"
            )));
        }
        match builtin {
            "ai_conversation" => scene.start.call(),
            "ai_conversation_stop" => scene.stop.call(),
            "ai_conversation_status" => scene.status.get(),
            "ai_conversation_line" => scene.line.get(),
            _ => scene.advance.call(),
        }
    };
    Ok(Some(expr.emit(c, expected)?))
}

/// Halo 1 writes captured actors to its object name table. Converted script
/// reads also check the captured handle. The original name remains usable
/// by placement operations such as `object_create`.
pub(super) fn named_atom(
    c: &mut Compiler<'_>,
    ty: u16,
    source: u16,
    token: &str,
    original: Datum,
) -> Result<Datum> {
    if !(50..=61).contains(&source) {
        return Ok(original);
    }
    if !((50..=55).contains(&ty) || ty == OBJECT_LIST) {
        return Ok(original);
    }
    let Some(alias) = c
        .conversations
        .as_ref()
        .and_then(|l| l.aliases.get(&token.to_ascii_lowercase()))
        .cloned()
    else {
        return Ok(original);
    };
    c.set_type(original, ty);
    let condition = not(eq(alias.unit.get(), Expr::None(OBJECT))).emit(c, h2t::BOOLEAN)?;
    let unit = alias.unit.get().emit(c, ty)?;
    let iff =
        c.h2.builtins
            .get("if")
            .ok_or_else(|| invalid("conversation requires Halo 2 if"))?;
    Ok(c.call(iff.index, CALL, ty, b"if", &[condition, unit, original]))
}

struct Program<'c, 's> {
    compiler: &'c mut Compiler<'s>,
    prefix: String,
    aliases: HashMap<String, Alias>,
    scope_selectors: HashMap<u16, Script>,
}

impl<'c, 's> Program<'c, 's> {
    fn new(compiler: &'c mut Compiler<'s>) -> Self {
        let prefix = (0..)
            .map(|n| format!("__h1cv{n}_"))
            .find(|prefix| {
                let taken = |name: &str| name.to_ascii_lowercase().starts_with(prefix);
                !compiler.t.scripts.iter().any(|s| taken(&s.name))
                    && !compiler.t.globals.iter().any(|g| taken(&g.name))
            })
            .expect("finite script and global names");
        Self {
            compiler,
            prefix,
            aliases: HashMap::new(),
            scope_selectors: HashMap::new(),
        }
    }

    fn add_alias(&mut self, name: &str) -> Result<()> {
        let name = name.to_ascii_lowercase();
        if !self.aliases.contains_key(&name) {
            let alias = Alias {
                unit: self.global("alias", Expr::None(OBJECT))?,
            };
            self.aliases.insert(name, alias);
        }
        Ok(())
    }

    fn alias(&self, name: Option<&str>) -> Option<&Alias> {
        self.aliases.get(&name?.to_ascii_lowercase())
    }

    /// A shared balanced selector keeps unscoped roles from each embedding
    /// a copy of the same squad or encounter literals in the node pool.
    fn scope_selector(&mut self, kind: u16, scopes: &[String], cursor: &Global) -> Result<Script> {
        if let Some(script) = self.scope_selectors.get(&kind) {
            return Ok(script.clone());
        }
        fn select(scopes: &[String], first: usize, cursor: &Global) -> Expr {
            if scopes.len() == 1 {
                return Expr::Hook(AI, scopes[0].clone());
            }
            let mid = scopes.len() / 2;
            either(
                pred("<", [cursor.get(), short((first + mid) as i32)]),
                select(&scopes[..mid], first, cursor),
                select(&scopes[mid..], first + mid, cursor),
            )
        }
        let script = self.script("scope", 3, select(scopes, 0, cursor))?;
        self.scope_selectors.insert(kind, script.clone());
        Ok(script)
    }

    fn global(&mut self, purpose: &str, init: Expr) -> Result<Global> {
        let c = &mut self.compiler;
        let index = c.out.globals.len();
        if index >= hs::MAX_DEFINITIONS {
            return Err(invalid("too many scenario global definitions"));
        }
        let name = format!("{}{purpose}_{index:x}", self.prefix);
        let ty = init.ty();
        let root = init.emit(c, ty)?;
        c.out.globals.push(definition(&name, ty, 0, root, index)?);
        Ok(Global {
            index: index as u16,
            name,
            ty,
        })
    }

    fn script(&mut self, purpose: &str, kind: u16, body: Expr) -> Result<Script> {
        let c = &mut self.compiler;
        let index = c.out.scripts.len();
        let name = format!("{}{purpose}_{index:x}", self.prefix);
        let ty = body.ty();
        let root = body.emit(c, ty)?;
        c.out
            .scripts
            .push(definition(&name, kind, ty, root, index)?);
        Ok(Script {
            index: index as u16,
            name,
            ty,
        })
    }
}

struct Scene<'p, 'c, 's, 'a> {
    program: &'p mut Program<'c, 's>,
    source: &'a Conversation,
    scopes: &'a SpeakerScopes,
    scratch: &'a CastScratch,
    roles: Vec<Option<Role>>,
    status: Global,
    line: Global,
    sound: Global,
    instance: Global,
    request: Global,
    working: Global,
    advanced: Global,
}

impl<'p, 'c, 's, 'a> Scene<'p, 'c, 's, 'a> {
    fn new(
        program: &'p mut Program<'c, 's>,
        source: &'a Conversation,
        scopes: &'a SpeakerScopes,
        scratch: &'a CastScratch,
    ) -> Result<Self> {
        let mut roles = Vec::with_capacity(source.participants.len());
        for participant in &source.participants {
            roles.push(if participant.radio() {
                None
            } else {
                let unit = program.global("actor", Expr::None(OBJECT))?;
                let voice = program.global("profile", short(-1))?;
                let mut words = participant
                    .voices
                    .chunks_exact(2)
                    .map(|v| long(u32::from(v[0] as u16) | (u32::from(v[1] as u16) << 16)));
                // Native scoring reads these five consecutive globals for each role.
                let voices = program.global("voices", words.next().unwrap())?.index;
                for word in words {
                    program.global("voices", word)?;
                }
                Some(Role {
                    unit,
                    voice,
                    voices,
                })
            });
        }
        Ok(Self {
            roles,
            status: program.global("status", short(0))?,
            line: program.global("line", short(NO_LINE))?,
            sound: program.global("sound", Expr::None(SOUND))?,
            instance: program.global("instance", long(u32::MAX))?,
            request: program.global("request", Expr::Number(h2t::LONG, 0))?,
            working: program.global("working", Expr::Number(h2t::LONG, 0))?,
            advanced: program.global("advanced", boolean(false))?,
            program,
            source,
            scopes,
            scratch,
        })
    }

    fn cast(&mut self) -> Result<Script> {
        let mut body: Vec<_> = self
            .roles
            .iter()
            .flatten()
            .map(|r| r.unit.set(Expr::None(OBJECT)))
            .collect();
        let source = self.source;
        // Exact names reserve their actor before searching the squad rosters.
        for named in [true, false] {
            for (i, participant) in source.participants.iter().enumerate() {
                if let Some(role) = self.roles[i].clone()
                    && participant.existing_name.is_some() == named
                {
                    body.extend(self.select_role(i, participant, &role)?);
                }
            }
        }
        self.program.script("bind", 3, begin(h2t::VOID, body))
    }

    fn required_alive(&self) -> Expr {
        let mut required = Vec::new();
        let mut alternates = Vec::new();
        for (participant, role) in self.source.participants.iter().zip(&self.roles) {
            if let Some(role) = role {
                if participant.flags & 6 != 0 {
                    alternates.push(alive(role.unit.get()));
                } else if !participant.optional() {
                    required.push(alive(role.unit.get()));
                }
            }
        }
        if !alternates.is_empty() {
            required.push(or(alternates));
        }
        and(required)
    }

    fn near_player(&self) -> Expr {
        if !self.source.delays_start() || self.source.trigger_distance <= 0.0 {
            return boolean(true);
        }
        let mut nearby = self.roles.iter().flatten().peekable();
        if nearby.peek().is_none() {
            return boolean(true);
        }
        or(nearby.map(|role| {
            let distance = call(
                "objects_distance_to_object",
                h2t::REAL,
                [call("players", OBJECT_LIST, []), role.unit.get()],
            );
            and([
                alive(role.unit.get()),
                pred("<=", [distance, Expr::Real(self.source.trigger_distance)]),
            ])
        }))
    }

    fn select_role(
        &mut self,
        i: usize,
        participant: &Participant,
        role: &Role,
    ) -> Result<Vec<Expr>> {
        let scratch = self.scratch;
        let candidate = scratch.candidate.get();
        let mut tests = vec![alive(candidate.clone())];
        for (other, taken) in self.roles.iter().enumerate() {
            if other != i
                && let Some(taken) = taken
            {
                tests.push(not(eq(taken.unit.get(), candidate.clone())));
            }
        }
        let first = self.roles.iter().flatten().next().unwrap().unit.index;
        let count = self.roles.iter().flatten().count();
        if count > u8::MAX as usize {
            return Err(invalid(
                "conversation cast has more than 255 physical roles",
            ));
        }
        let options =
            (u32::from(first) << 16) | (count as u32) << 8 | u32::from(participant.selection);
        let accept = when(
            and(tests),
            begin(
                h2t::VOID,
                [
                    bridge(
                        Op::Score,
                        [scratch.candidate.operand(), u32::from(role.voices), options],
                    ),
                    when(
                        pred(">", [scratch.rank.get(), scratch.best.get()]),
                        begin(
                            h2t::VOID,
                            [
                                scratch.best.set(scratch.rank.get()),
                                role.unit.set(candidate),
                                role.voice.set(scratch.voice.get()),
                            ],
                        ),
                    ),
                ],
            ),
        );
        let reset = scratch.best.set(Expr::Real(f32::MIN));
        if let Some(name) = &participant.existing_name {
            let named = Expr::As(OBJECT, Box::new(Expr::Hook(OBJECT_NAME, name.clone())));
            let speaker = self
                .program
                .alias(Some(name))
                .map_or(named.clone(), |alias| {
                    either(alive(alias.unit.get()), alias.unit.get(), named)
                });
            return Ok(vec![reset, scratch.candidate.set(speaker), accept]);
        }
        let (scope, count) = if let Some(scope) = &participant.encounter {
            (Expr::Hook(AI, scope.clone()), 1)
        } else {
            let scopes = self.scopes.for_actor_type(participant.actor_type);
            if scopes.is_empty() {
                return Ok(Vec::new());
            }
            if let [scope] = scopes {
                (Expr::Hook(AI, scope.clone()), 1)
            } else {
                let selector = self.program.scope_selector(
                    participant.actor_type,
                    scopes,
                    &scratch.scope_cursor,
                )?;
                (selector.call(), scopes.len())
            }
        };
        let more = || pred("<", [scratch.cursor.get(), scratch.count.get()]);
        let step = when(
            more(),
            begin(
                h2t::VOID,
                [
                    scratch.candidate.set(call(
                        "list_get",
                        OBJECT,
                        [scratch.actors.get(), scratch.cursor.get()],
                    )),
                    scratch
                        .cursor
                        .set(add_one(h2t::SHORT, scratch.cursor.get())),
                    accept,
                ],
            ),
        );
        let step = self.program.script("pick_actor", 3, step)?;
        let batch = self.program.script(
            "scan_batch",
            3,
            when(
                more(),
                begin(h2t::VOID, (0..SCAN_BATCH).map(|_| step.call())),
            ),
        )?;
        let chunk = self.program.script(
            "scan_chunk",
            3,
            when(
                more(),
                begin(h2t::VOID, (0..SCAN_BATCH).map(|_| batch.call())),
            ),
        )?;
        let actors = call("ai_actors", OBJECT_LIST, [scope]);
        let mut select = vec![
            scratch.actors.set(actors),
            scratch
                .count
                .set(call("list_count", h2t::SHORT, [scratch.actors.get()])),
            scratch.cursor.set(short(0)),
        ];
        select.extend((0..SCAN_CHUNKS).map(|_| chunk.call()));
        select.push(scratch.actors.set(Expr::None(OBJECT_LIST)));
        if count == 1 {
            select.insert(0, reset);
            return Ok(select);
        }
        select.push(
            scratch
                .scope_cursor
                .set(add_one(h2t::SHORT, scratch.scope_cursor.get())),
        );
        let scan = self.program.script("scan", 3, begin(h2t::VOID, select))?;
        let mut attempts = vec![reset, scratch.scope_cursor.set(short(0))];
        attempts.extend((0..count).map(|_| scan.call()));
        Ok(attempts)
    }
}
