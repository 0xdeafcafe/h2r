//! Evidence from the owner's two discs. VM tests use an in-memory world
//! and exercise the generated HaloScript against the retail descriptors.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::BufReader,
    path::PathBuf,
};

use h1ai::{
    conversations::{Conversation, Participant, SpeakerScopes},
    h1::H1Ai,
};
use h1scripts::{
    Catalog, Tables,
    compile::{Compiled, Cutscenes, Missing, compile_with_conversations},
    translate::{Table, Target},
};
use hs::h1_conversation::{BUILTINS, Op};
use hs::vm::{Cell, Engine, EngineCall, EngineError, HsType, ScenarioGlobals, Vm, VmImage};

const BASE: u32 = 0x8000_0000;
const BRIDGE: u16 = BUILTINS[0].1;
const ACTOR: u32 = 0xe000_0001;
const ACTOR_NAME: i16 = 2;
const SECOND_ACTOR: u32 = 0xe000_0002;

fn disc(env: &str, name: &str) -> PathBuf {
    std::env::var_os(env).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../disc")
            .join(name)
    })
}

fn with_h1(mut f: impl FnMut(&Catalog, &str, &h1map::H1Map)) {
    let path = disc("H1_ISO", "halo1.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let xbe = iso.find("default.xbe").unwrap();
    let xbe_bytes = iso.read(&xbe).unwrap();
    let catalog = Catalog::from_xbe(&xbe::Xbe::parse(&xbe_bytes).unwrap()).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let entry = disc.map(level).unwrap();
        let file = iso.read(entry).unwrap();
        let map = h1map::H1Map::from_reader(&file[..]).unwrap();
        f(&catalog, level, &map);
    }
}

fn h2() -> (hs::Catalog, Target, VmImage) {
    let path = disc("H2_DISC", "halo2.iso");
    let open = || BufReader::new(File::open(&path).unwrap());
    let mut iso = xiso::Xiso::open(open()).unwrap();
    let entry = iso.find("default.xbe").unwrap();
    let xbe_bytes = iso.read(&entry).unwrap();
    let xbe = xbe::Xbe::parse(&xbe_bytes).unwrap();
    let mut catalog = hs::Catalog::from_xbe(&xbe).unwrap();
    let mut scenarios = Vec::new();
    for entry in iso
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let base = iso.partition_offset() + u64::from(entry.sector) * xiso::SECTOR;
        let mut map = map::Map::from_region(open(), base, entry.size.into()).unwrap();
        let scenario = hs::Scenario::read(&mut map).unwrap();
        catalog.observe(&scenario).unwrap();
        scenarios.push(scenario);
    }
    let mut target = Target::from_catalog(&catalog);
    for scenario in &scenarios {
        target.observe_externals(scenario);
    }
    assert!(
        catalog.function(BRIDGE).unwrap().name().is_none(),
        "slot 845 retail use"
    );
    let mut image = VmImage::from_xbe(&xbe, &catalog).unwrap();
    assert!(
        image.discarded().contains_key(&BRIDGE),
        "retail slot must be discarded"
    );
    image.use_engine_builtin(BRIDGE);
    (catalog, target, image)
}

fn compile(
    t: &Tables,
    h1: &Catalog,
    h2: &Target,
    conversations: &[Conversation],
    scopes: &SpeakerScopes,
) -> Compiled {
    compile_with_conversations(
        t,
        h1,
        &Table::build(h1, h2),
        h2,
        Missing::Stub,
        Cutscenes::Play,
        &|_, _| true,
        &[],
        conversations,
        scopes,
    )
    .unwrap()
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn every_campaign_conversation_compiles_with_its_source_sounds() {
    let (h2_catalog, target, _) = h2();
    assert_eq!(target.builtins["unit_set_emotion"].index, 218);
    assert_eq!(h2_catalog.function(218).unwrap().evaluate(), 0x002a3b70);
    assert_eq!(target.builtins["ai_dialogue_enable"].index, 281);
    assert_eq!(h2_catalog.function(281).unwrap().evaluate(), 0x002a4dc0);
    let mut totals = (0, 0);
    with_h1(|h1, level, map| {
        let ai = H1Ai::read(map).unwrap_or_else(|e| panic!("{level}: {e}"));
        let t = Tables::read(map).unwrap();
        let options = Default::default();
        let mut plan = h1ai::convert::convert(&ai, &options).unwrap();
        let mut controls = h1ai::control::prepare(&ai, &mut plan, &options, &[]).unwrap();
        controls.bind_script_indices(&plan);
        let scopes = SpeakerScopes::from_plan(&plan, &controls, &ai.conversations).unwrap();
        if level == "a30" {
            let squad = plan
                .ai
                .squads
                .iter()
                .position(|s| s.name == "first_marine/left_back")
                .unwrap();
            assert_eq!(scopes.squad_voices[squad].leader_rule, 3);
        }
        if matches!(level, "a10" | "a30") {
            assert!(
                scopes
                    .voice_definitions
                    .iter()
                    .any(|d| d.generic == [2, 4, 1, 5, 6])
            );
        }
        let code = compile(&t, h1, &target, &ai.conversations, &scopes);
        for missing in &code.missing {
            assert!(
                !missing.name.starts_with("ai_conversation")
                    && !matches!(
                        missing.name.as_str(),
                        "ai_dialogue_triggers" | "unit_set_emotion"
                    ),
                "{level}: missing {} in {}",
                missing.name,
                missing.definition,
            );
        }
        let source_sounds: BTreeSet<_> = ai
            .conversations
            .iter()
            .flat_map(|c| &c.lines)
            .flat_map(|line| line.sounds.iter().flatten().map(String::as_str))
            .collect();
        let names: BTreeSet<_> = code
            .references
            .iter()
            .filter(|(group, _)| *group == h1map::Group::new(b"snd!"))
            .map(|(_, name)| name.as_str())
            .collect();
        assert!(
            source_sounds.is_subset(&names),
            "{level}: dialogue sound references"
        );
        assert_eq!(
            code.hooks
                .iter()
                .filter(|h| h.h1_type == "conversation" && h.type_name == "sound")
                .filter(|h| map.find(h1map::Group::new(b"snd!"), &h.token).is_none())
                .count(),
            0,
            "{level}: missing sound tags"
        );

        use hs::h1_conversation::spawn::*;
        let word = |at: usize| u32::from_le_bytes(code.strings[at..at + 4].try_into().unwrap());
        if !scopes.voice_definitions.is_empty() {
            assert_eq!(word(4), SIGNATURE, "{level}: voice metadata signature");
            assert_eq!(word(12) as usize, scopes.voice_definitions.len());
            assert_eq!(word(16) as usize, scopes.squad_voices.len());
            let end = HEADER + BANK_BYTES * scopes.voice_definitions.len();
            for (bank, raw) in scopes
                .voice_definitions
                .iter()
                .zip(code.strings[HEADER..end].chunks_exact(BANK_BYTES))
            {
                assert_eq!(
                    i16::from_le_bytes(raw[2..4].try_into().unwrap()) as usize,
                    bank.generic.len()
                );
                for (i, v) in bank.generic.iter().enumerate() {
                    assert_eq!(
                        i16::from_le_bytes(raw[4 + 2 * i..6 + 2 * i].try_into().unwrap()),
                        *v
                    );
                }
            }
            let starts = end + SQUAD_BYTES * scopes.squad_voices.len();
            let half = |at: usize| u16::from_le_bytes(code.strings[at..at + 2].try_into().unwrap());
            for (i, squad) in scopes.squad_voices.iter().enumerate() {
                let at = end + i * SQUAD_BYTES;
                assert_eq!(half(at), squad.encounter);
                assert_eq!(half(at + 2), squad.leader_rule);
                assert_eq!(half(at + 4), squad.generic.unwrap_or(NO_BANK));
                assert_eq!(usize::from(half(at + 8)), squad.locations.len());
                let first = starts + 2 * usize::from(half(at + 6));
                for (j, bank) in squad.locations.iter().enumerate() {
                    assert_eq!(half(first + 2 * j), bank.unwrap_or(NO_BANK));
                }
            }
        }
        let bytes = code.scenario_bytes(BASE);
        let sc = hs::Scenario::decode(&bytes, BASE).unwrap();
        let generated = sc
            .reachable(
                sc.scripts()[t.scripts.len()..]
                    .iter()
                    .map(|s| s.root())
                    .chain(sc.globals()[t.globals.len()..].iter().map(|g| g.root())),
            )
            .unwrap();
        let bad: Vec<_> = sc
            .unrunnable(&h2_catalog)
            .unwrap()
            .into_iter()
            .filter(|u| generated.iter().any(|d| d.slot() == u.slot))
            .collect();
        assert!(bad.is_empty(), "{level}: {bad:#?}");
        let compiled_approach = sc.scripts()[t.scripts.len()..]
            .iter()
            .filter(|s| s.kind() == 5 && s.name().unwrap().starts_with("__h1cv0_approach_"))
            .count();
        assert_eq!(
            compiled_approach,
            ai.conversations
                .iter()
                .filter(|scene| {
                    scene.runs_to_player() && scene.participants.iter().any(|p| !p.radio())
                })
                .count(),
            "{level}: run-to-player command scripts"
        );
        let workers = sc.scripts()[t.scripts.len()..]
            .iter()
            .filter(|s| s.kind() == 2 && s.name().unwrap().starts_with("__h1cv"))
            .count();
        assert_eq!(workers, ai.conversations.len(), "{level}");
        assert!(sc.scripts().iter().filter(|s| s.kind() <= 2).count() < 0x140);
        totals.0 += ai.conversations.len();
        totals.1 += ai
            .conversations
            .iter()
            .map(|c| c.lines.len())
            .sum::<usize>();
    });
    assert_eq!(totals, (177, 361));
}

#[derive(Default)]
struct DialogueEngine {
    tick: i32,
    seed: u32,
    actor_alive: bool,
    player_distance: f32,
    playing: BTreeMap<u32, i32>,
    starts: Vec<(u32, u32)>,
    next_sound: u32,
    stops: Vec<u32>,
}

impl Engine for DialogueEngine {
    fn game_time(&self) -> i32 {
        self.tick
    }
    fn random_seed(&mut self) -> &mut u32 {
        &mut self.seed
    }
    fn object_type(&mut self, object: u32) -> Option<u8> {
        (matches!(object, ACTOR | SECOND_ACTOR) && self.actor_alive).then_some(0)
    }
    fn object_from_name(&mut self, name: i16) -> u32 {
        match name {
            ACTOR_NAME if self.actor_alive => ACTOR,
            n if n == ACTOR_NAME + 1 && self.actor_alive => SECOND_ACTOR,
            _ => u32::MAX,
        }
    }
    fn object_list_new(&mut self, _: &[u32]) -> u32 {
        u32::MAX
    }
    fn object_list_from_ai(&mut self, _: u32) -> u32 {
        u32::MAX
    }
    fn call(&mut self, c: EngineCall<'_>) -> Result<Cell, EngineError> {
        let subject = c.arguments.first().copied().unwrap_or(Cell::NONE).0;
        match c.index {
            28 => Ok(Cell(0xe100_0000)), // players
            69 => Ok(Cell::from_real(
                if matches!(subject, ACTOR | SECOND_ACTOR) && self.actor_alive {
                    1.0
                } else {
                    0.0
                },
            )),
            114 => Ok(Cell::from_real(self.player_distance)),
            _ => Err(EngineError::Failed(format!(
                "unexpected builtin {}",
                c.index
            ))),
        }
    }

    fn call_with_globals(
        &mut self,
        c: EngineCall<'_>,
        mut globals: ScenarioGlobals<'_>,
    ) -> Result<Cell, EngineError> {
        if c.index != BRIDGE {
            return self.call(c);
        }
        let [op, a, b, out] = c.arguments else {
            panic!("conversation builtin arguments")
        };
        let (a, b, out) = (a.0, b.0, out.0);
        match Op::decode(op.0).unwrap() {
            Op::Score => {
                let requested = globals.get(b, HsType::LONG).unwrap().0 as i16;
                assert!(globals.set(a + 1, HsType::REAL, Cell::from_real(1.0)));
                assert!(globals.set(a + 2, HsType::SHORT, Cell(u32::from(requested as u16))));
            }
            Op::RememberVoice => {}
            Op::StartSound => {
                let tag = globals
                    .get(a, HsType(hs::h1_conversation::SOUND))
                    .unwrap()
                    .0;
                let object = globals.get(b, HsType::OBJECT).unwrap_or(Cell::NONE).0;
                self.starts.push((tag, object));
                self.next_sound += 1;
                let duration = if object == SECOND_ACTOR { 10 } else { 5 };
                self.playing.insert(self.next_sound, self.tick + duration);
                assert!(globals.set(out, HsType::LONG, Cell(self.next_sound)));
            }
            Op::PollSound | Op::StopSound => {
                let handle = globals.get(a, HsType::LONG).unwrap().0;
                let stop = matches!(Op::decode(op.0), Some(Op::StopSound));
                if stop
                    || self
                        .playing
                        .get(&handle)
                        .is_none_or(|&end| end <= self.tick)
                {
                    if self.playing.remove(&handle).is_some() && stop {
                        self.stops.push(handle);
                    }
                    assert!(globals.set(a, HsType::LONG, Cell::NONE));
                }
            }
            _ => panic!("unexpected conversation operation {}", op.0),
        }
        Ok(Cell::ZERO)
    }
}

fn tick_until(
    vm: &mut Vm<'_>,
    engine: &mut DialogueEngine,
    mut condition: impl FnMut(&Vm<'_>, &DialogueEngine) -> bool,
) {
    for _ in 0..180 {
        if condition(vm, engine) {
            return;
        }
        engine.tick += 1;
        vm.update(engine);
        assert!(vm.faults().is_empty(), "{:#?}", vm.faults());
    }
    assert!(
        condition(vm, engine),
        "conversation did not reach the expected state"
    );
}

#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn conversation_vm_plays_holds_advances_and_restarts() {
    let (h2_catalog, target, image) = h2();
    let mut exercised = false;
    with_h1(|h1, _level, map| {
        if exercised {
            return;
        }
        let scenes = h1ai::conversations::read(map).unwrap();
        let Some(scene) = scenes.into_iter().find(|c| {
            c.lines.len() >= 2
                && c.participants.iter().all(|p| p.radio())
                && c.lines[..2]
                    .iter()
                    .all(|l| l.speaker >= 0 && l.sounds[0].is_some())
                && c.lines[0].sounds[0] != c.lines[1].sounds[0]
        }) else {
            return;
        };
        let mut scene = scene;
        scene.lines.truncate(2);
        // Keep the two disc voices, and give the first one a physical role
        // so the VM must bind a unit and apply the player's trigger distance.
        scene.lines[0].speaker = scene.participants.len() as i16;
        scene.participants.push(Participant {
            flags: 0,
            selection: 0,
            actor_type: 7,
            existing_name: Some("test_actor".into()),
            assigned_name: None,
            voices: [1, -1, -1, -1, -1, -1],
            encounter: None,
        });
        scene.flags = 0x40;
        scene.trigger_distance = 12.0;
        scene.lines[0].flags = 8;
        scene.lines[1].flags = 0;
        for line in &mut scene.lines {
            line.addressee_kind = 0;
            line.addressee = -1;
        }
        let paths = scene
            .lines
            .iter()
            .map(|l| l.sounds[0].clone().unwrap())
            .collect::<Vec<_>>();
        let t = Tables::read(map).unwrap();
        // Only the scene: the level's command lists would add their own
        // hooks (animations, recordings, object names) to resolve.
        let t = Tables {
            commands: Default::default(),
            scripts: Vec::new(),
            globals: Vec::new(),
            references: Vec::new(),
            ..t
        };
        let mut concurrent = scene.clone();
        concurrent.name = "concurrent".into();
        concurrent.lines.truncate(1);
        concurrent.lines[0].flags = 0;
        concurrent.lines[0].pause_after = 0.0;
        concurrent.participants.last_mut().unwrap().existing_name = Some("second_actor".into());
        let mut compiled = compile(
            &t,
            h1,
            &target,
            &[scene, concurrent],
            &SpeakerScopes::default(),
        );
        let sound = |i| 0x1234_0000 | i as u32;
        let left = compiled.resolve_hooks(|h| {
            if h.token == "test_actor" {
                Some(0xffff_0000 | ACTOR_NAME as u32)
            } else if h.token == "second_actor" {
                Some(0xffff_0000 | (ACTOR_NAME + 1) as u32)
            } else {
                paths.iter().position(|p| p == &h.token).map(sound)
            }
        });
        assert!(left.is_empty());
        let bytes = compiled.scenario_bytes(BASE);
        let sc = hs::Scenario::decode(&bytes, BASE).unwrap();
        sc.validate(&h2_catalog).unwrap();
        let script_at = |purpose: &str, scene: usize| {
            sc.scripts()
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.name()
                        .unwrap()
                        .starts_with(&format!("__h1cv0_{purpose}_"))
                })
                .nth(scene)
                .unwrap()
                .0 as u16
        };
        let script = |purpose| script_at(purpose, 0);
        let global_at = |purpose: &str, scene: usize| {
            sc.globals()
                .iter()
                .enumerate()
                .filter(|(_, g)| {
                    g.name()
                        .unwrap()
                        .starts_with(&format!("__h1cv0_{purpose}_"))
                })
                .nth(scene)
                .unwrap()
                .0
        };
        let global = |purpose| global_at(purpose, 0);
        let (start, stop, advance) = (script("start"), script("stop"), script("advance"));
        let status = |vm: &Vm<'_>| vm.global(global("status")).unwrap().short();
        let line = |vm: &Vm<'_>| vm.global(global("line")).unwrap().short();

        let mut vm = Vm::new(&sc, &h2_catalog, &image);
        let mut engine = DialogueEngine::default();
        vm.initialize(&mut engine).unwrap();
        assert_eq!((status(&vm), line(&vm)), (0, 999));
        assert!(
            !vm.call_script(&mut engine, start)
                .unwrap()
                .unwrap()
                .boolean()
        );
        assert_eq!((status(&vm), line(&vm)), (5, 999));

        engine.actor_alive = true;
        engine.player_distance = 32.0;
        assert!(
            vm.call_script(&mut engine, start)
                .unwrap()
                .unwrap()
                .boolean()
        );
        assert_eq!((status(&vm), line(&vm)), (1, -1));
        tick_until(&mut vm, &mut engine, |_, e| e.tick == 35);
        assert!(engine.starts.is_empty());
        engine.player_distance = 6.0;
        tick_until(&mut vm, &mut engine, |_, e| e.starts.len() == 1);
        assert_eq!((status(&vm), line(&vm)), (3, 0));
        vm.call_script(&mut engine, stop).unwrap().unwrap();
        assert_eq!((status(&vm), line(&vm)), (7, 999));
        assert_eq!(engine.stops, [1]);

        // Starting again on the same tick must not resume the cancelled worker.
        assert!(
            vm.call_script(&mut engine, start)
                .unwrap()
                .unwrap()
                .boolean()
        );
        tick_until(&mut vm, &mut engine, |vm, _| status(vm) == 4);
        assert_eq!(line(&vm), 0);
        assert_eq!(engine.starts, [(sound(0), ACTOR); 2]);
        vm.call_script(&mut engine, advance).unwrap().unwrap();
        tick_until(&mut vm, &mut engine, |vm, _| status(vm) == 6);
        assert_eq!(
            engine.starts,
            [(sound(0), ACTOR), (sound(0), ACTOR), (sound(1), u32::MAX)]
        );
        assert_eq!(line(&vm), 999);

        // Two physical speakers may overlap one sound tag. Aborting the first
        // must leave the second instance live until its own end tick.
        vm.call_script(&mut engine, start).unwrap().unwrap();
        vm.call_script(&mut engine, script_at("start", 1))
            .unwrap()
            .unwrap();
        tick_until(&mut vm, &mut engine, |_, e| e.starts.len() == 5);
        // Both workers were started before the same tick. Which one the VM
        // runs first within a tick is its thread order, which no listener
        // can hear: compare the two starts as a set, then follow each
        // speaker's own handle (the stub numbers them from 1 in start order).
        let mut both = engine.starts[3..].to_vec();
        both.sort_unstable();
        let mut expected = vec![(sound(0), ACTOR), (sound(0), SECOND_ACTOR)];
        expected.sort_unstable();
        assert_eq!(both, expected);
        let handle = |object| {
            4 + engine.starts[3..].iter().position(|&(_, o)| o == object).unwrap() as u32
        };
        let (first, overlap) = (handle(ACTOR), handle(SECOND_ACTOR));
        let second = |vm: &Vm<'_>| vm.global(global_at("status", 1)).unwrap().short();
        assert_eq!(second(&vm), 3);
        vm.call_script(&mut engine, stop).unwrap().unwrap();
        assert_eq!(engine.stops, [1, first]);
        assert!(engine.playing.contains_key(&overlap));
        let end = engine.playing[&overlap];
        tick_until(&mut vm, &mut engine, |vm, _| second(vm) == 6);
        assert_eq!(engine.tick, end);
        assert_eq!(engine.stops, [1, first]);
        exercised = true;
    });
    assert!(
        exercised,
        "missing a two-line radio conversation on Halo 1 disc"
    );
}
