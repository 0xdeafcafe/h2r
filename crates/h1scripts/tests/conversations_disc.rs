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
use hs::vm::{Cell, Engine, EngineCall, EngineError, Vm, VmImage};

const BASE: u32 = 0x8000_0000;
const START: u16 = 597;
const TIME: u16 = 602;
const STOP: u16 = 603;
const ACTOR: u32 = 0xe000_0001;
const ACTOR_NAME: i16 = 2;

fn disc(env: &str, name: &str) -> PathBuf {
    std::env::var_os(env).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../disc")
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
    let image = VmImage::from_xbe(&xbe, &catalog).unwrap();
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
        let plan = h1ai::convert::convert(&ai, &Default::default());
        let scopes = SpeakerScopes::from_plan(&plan);
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
        let workers = sc.scripts()[t.scripts.len()..]
            .iter()
            .filter(|s| s.kind() == 2)
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
        (object == ACTOR && self.actor_alive).then_some(0)
    }
    fn object_from_name(&mut self, name: i16) -> u32 {
        if name == ACTOR_NAME && self.actor_alive {
            ACTOR
        } else {
            u32::MAX
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
            28 => return Ok(Cell(0xe100_0000)), // players
            69 => {
                return Ok(Cell::from_real(if subject == ACTOR && self.actor_alive {
                    1.0
                } else {
                    0.0
                }));
            }
            114 => return Ok(Cell::from_real(self.player_distance)),
            START => {
                self.starts.push((subject, c.arguments[1].0));
                self.playing.insert(subject, self.tick + 5);
            }
            TIME => {
                let end = self.playing.get(&subject).copied().unwrap_or(self.tick);
                return Ok(Cell::from_long((end - self.tick).max(0)));
            }
            STOP => {
                if self
                    .playing
                    .remove(&subject)
                    .is_some_and(|end| end > self.tick)
                {
                    self.stops.push(subject);
                }
            }
            _ => {
                return Err(EngineError::Failed(format!(
                    "unexpected builtin {}",
                    c.index
                )));
            }
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
        scene.flags |= 0x40;
        scene.trigger_distance = 12.0;
        scene.lines[0].flags |= 8;
        scene.lines[1].flags &= !8;
        let paths = scene
            .lines
            .iter()
            .map(|l| l.sounds[0].clone().unwrap())
            .collect::<Vec<_>>();
        let t = Tables::read(map).unwrap();
        let t = Tables {
            scripts: Vec::new(),
            globals: Vec::new(),
            references: Vec::new(),
            ..t
        };
        let mut compiled = compile(&t, h1, &target, &[scene], &SpeakerScopes::default());
        let sound = |i| 0x1234_0000 | i as u32;
        let left = compiled.resolve_hooks(|h| {
            if h.token == "test_actor" {
                Some(0xffff_0000 | ACTOR_NAME as u32)
            } else {
                paths.iter().position(|p| p == &h.token).map(sound)
            }
        });
        assert!(left.is_empty());
        let bytes = compiled.scenario_bytes(BASE);
        let sc = hs::Scenario::decode(&bytes, BASE).unwrap();
        sc.validate(&h2_catalog).unwrap();
        let script = |purpose: &str| {
            sc.scripts()
                .iter()
                .position(|s| {
                    s.name()
                        .unwrap()
                        .starts_with(&format!("__h1cv0_{purpose}_"))
                })
                .unwrap() as u16
        };
        let global = |purpose: &str| {
            sc.globals()
                .iter()
                .position(|g| {
                    g.name()
                        .unwrap()
                        .starts_with(&format!("__h1cv0_{purpose}_"))
                })
                .unwrap()
        };
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
        assert_eq!(engine.stops, [sound(0)]);

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
        exercised = true;
    });
    assert!(
        exercised,
        "missing a two-line radio conversation on Halo 1 disc"
    );
}
