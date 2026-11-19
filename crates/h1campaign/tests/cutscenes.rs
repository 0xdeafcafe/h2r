//! Halo 1's cutscenes in the converted levels, from the owner's discs
//! (`$H1_ISO`, `$H2_DISC`). Surveys print counts, offsets and tag names
//! only; nothing is written to disk.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1scripts::translate::Kind;

const LEVELS: [&str; 10] = [
    "a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40",
];

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn levels() -> Vec<String> {
    std::env::var("H1_LEVELS")
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_else(|_| LEVELS.iter().map(|s| (*s).to_owned()).collect())
}

fn h1_iso() -> xiso::Xiso<BufReader<File>> {
    xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap()
}

fn h1_level(name: &str) -> h1map::H1Map {
    let mut iso = h1_iso();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap().clone();
    h1map::H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h2_iso() -> xiso::Xiso<BufReader<File>> {
    xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap()
}

fn h2_scenarios() -> (Vec<u8>, Vec<hs::Scenario>) {
    let mut iso = h2_iso();
    let e = iso.find("default.xbe").unwrap();
    let xbe = iso.read(&e).unwrap();
    let mut out = Vec::new();
    for e in iso
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        out.push(hs::Scenario::read(&mut m).unwrap());
        if std::env::var("DUMP_H2").is_err() {
            continue;
        }
        // The block at scnr +0x100 (stride 0x34): Halo 1's recorded
        // animation element (0x40) with its data reference in Halo 2's
        // 8-byte form, if that is what it is.
        let tag = m.scenario().unwrap().clone();
        let addr = tag.location.unwrap().address;
        let bytes = m.read_tag_bytes(tag.id).unwrap();
        let w = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        // Root blocks whose first element starts with a printable name of
        // 4..32 bytes: the candidates for the recorded animations block.
        let layouts = std::fs::read_to_string(format!(
            "{}/../tags/src/schema/layouts.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        for l in layouts
            .split("\ngroup ")
            .find(|g| g.starts_with("scnr "))
            .unwrap()
            .lines()
            .filter(|l| l.starts_with("  0x") && !l.starts_with("   "))
        {
            let f: Vec<_> = l.split_whitespace().collect();
            if f.get(1) != Some(&"block") {
                continue;
            }
            let field = usize::from_str_radix(&f[0][2..], 16).unwrap();
            let stride = usize::from_str_radix(&f[2][2..], 16).unwrap();
            let (n, a) = (w(field), w(field + 4));
            if n == 0 || a < addr || (a - addr) as usize + stride > bytes.len() || stride < 0x28 {
                continue;
            }
            let e0 = &bytes[(a - addr) as usize..(a - addr) as usize + stride];
            let len = e0.iter().position(|&c| c == 0).unwrap_or(0);
            if (4..32).contains(&len) && e0[..len].iter().all(|c| c.is_ascii_graphic()) {
                println!(
                    "{}: named block {field:#x} stride {stride:#x} count {n} [0] {:?}",
                    e.path,
                    String::from_utf8_lossy(&e0[..len])
                );
            }
        }
        let (count, at) = (w(0x100), w(0x104));
        if count > 0 && at >= addr && ((at - addr) as usize) < bytes.len() {
            let e0 = (at - addr) as usize;
            let el = &bytes[e0..e0 + 0x34];
            let name_end = el.iter().position(|&c| c == 0).unwrap_or(32).min(32);
            println!(
                "{}: scnr+0x100 {count}, [0] name {:?} bytes 20..34 {:02x?}",
                e.path,
                String::from_utf8_lossy(&el[..name_end]),
                &el[0x20..0x34]
            );
        } else {
            println!("{}: scnr+0x100 {count}", e.path);
        }
    }
    (xbe, out)
}

/// Halo 1 builtins a cutscene uses.
const CUTSCENE_BUILTINS: &[&str] = &[
    "custom_animation",
    "custom_animation_list",
    "unit_custom_animation_at_frame",
    "object_set_custom_animation",
    "scenery_animation_start",
    "scenery_animation_start_at_frame",
    "unit_stop_custom_animation",
    "unit_is_playing_custom_animation",
    "unit_get_custom_animation_time",
    "object_animation_start",
    "recording_play",
    "recording_play_and_delete",
    "recording_play_and_hover",
    "recording_kill",
    "recording_time",
    "camera_set",
    "camera_set_relative",
    "camera_set_animation",
    "camera_set_first_person",
    "camera_set_dead",
    "camera_control",
    "camera_time",
    "sound_impulse_start",
    "sound_impulse_time",
    "sound_impulse_stop",
    "sound_looping_start",
    "sound_looping_stop",
    "cinematic_start",
    "cinematic_stop",
    "cinematic_set_title",
    "cinematic_set_title_delayed",
    "cinematic_show_letterbox",
    "cinematic_screen_effect_start",
    "cinematic_screen_effect_stop",
    "cinematic_screen_effect_set_convolution",
    "cinematic_screen_effect_set_filter",
    "cinematic_screen_effect_set_video",
    "cinematic_suppress_bsp_object_creation",
    "fade_in",
    "fade_out",
    "show_hud",
    "player_enable_input",
    "player_camera_control",
    "switch_bsp",
    "object_create",
    "object_create_anew",
    "object_destroy",
    "object_teleport",
    "object_beautify",
    "object_pvs_activate",
    "object_pvs_set_camera",
    "unit_set_emotion",
    "unit_set_emotional_state",
    "objects_attach",
    "objects_detach",
    "unit_enter_vehicle",
    "unit_exit_vehicle",
    "unit_suspended",
    "object_hide",
    "unit_close",
    "unit_open",
    "vehicle_hover",
    "vehicle_load_magic",
    "vehicle_unload",
    "object_set_facing",
    "unit_set_desired_flashlight_state",
    "sound_class_set_gain",
];

/// Every builtin call in a level's scripts: (builtin index, argument
/// datums).
fn calls(t: &h1scripts::Tables) -> Vec<(u16, Vec<hs::Node>)> {
    let mut out = Vec::new();
    for n in t.nodes.iter().filter(|n| n.live()) {
        if n.atom() || n.script_call() {
            continue;
        }
        let Ok(head) = t.node(hs::Datum(n.data())) else {
            continue;
        };
        let mut args = Vec::new();
        let mut a = head.next();
        while a != hs::Datum::NULL {
            let Ok(x) = t.node(a) else { break };
            args.push(*x);
            a = x.next();
        }
        out.push((n.opcode(), args));
    }
    out
}

fn token(t: &h1scripts::Tables, n: &hs::Node) -> String {
    t.string(n.string_offset()).unwrap_or("?").to_owned()
}

/// What each level's cutscenes call, how each builtin carries over to
/// Halo 2, which animation graphs and animations they name, and the
/// recorded-animation block. Prints names of tags and builtins only.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn cutscene_survey() {
    let mut iso = h1_iso();
    let e = iso.find("/default.xbe").unwrap();
    let h1_xbe = iso.read(&e).unwrap();
    let catalog = h1scripts::Catalog::from_xbe(&xbe::Xbe::parse(&h1_xbe).unwrap()).unwrap();
    let (h2_xbe, scenarios) = h2_scenarios();
    let mut c = hs::Catalog::from_xbe(&xbe::Xbe::parse(&h2_xbe).unwrap()).unwrap();
    for s in &scenarios {
        c.observe(s).unwrap();
    }
    let target = h1scripts::translate::Target::from_catalog(&c);
    let table = h1scripts::translate::Table::build(&catalog, &target);

    println!("== builtins");
    for name in CUTSCENE_BUILTINS {
        let Some(i) = catalog.index_of(name) else {
            println!("{name}: not in Halo 1");
            continue;
        };
        let f = catalog.function(i).unwrap();
        let tr = table.builtin(i).unwrap();
        let sig: Vec<_> = f
            .parameters
            .iter()
            .map(|&p| catalog.type_name(p).unwrap_or("?"))
            .collect();
        let h2 = tr.target.as_ref().map(|(n, b)| {
            let p: Vec<_> = b
                .parameters
                .iter()
                .map(|&p| c.type_name(p).unwrap_or("?"))
                .collect();
            format!(
                "{n}#{} ({}) -> {}",
                b.index,
                p.join(" "),
                c.type_name(b.return_type).unwrap_or("?")
            )
        });
        println!(
            "{name} ({}) -> {}: {:?} {:?} {:?}",
            sig.join(" "),
            catalog.type_name(f.return_type).unwrap_or("?"),
            tr.kind,
            h2,
            tr.reason
        );
    }
    if std::env::var("DUMP_H2").is_ok() {
        for (j, f) in c.functions().iter().enumerate() {
            let p: Vec<_> = f
                .parameters()
                .iter()
                .map(|&p| c.type_name(p).unwrap_or("?"))
                .collect();
            println!(
                "h2fn #{j} {} ({}) -> {} flags {}",
                f.name().unwrap_or("-"),
                p.join(" "),
                c.type_name(f.return_type()).unwrap_or("?"),
                f.flags()
            );
        }
        for (j, f) in catalog.functions().iter().enumerate() {
            let p: Vec<_> = f
                .parameters
                .iter()
                .map(|&p| catalog.type_name(p).unwrap_or("?"))
                .collect();
            println!(
                "h1fn #{j} {} ({}) -> {}",
                f.name,
                p.join(" "),
                catalog.type_name(f.return_type).unwrap_or("?")
            );
        }
        return;
    }
    println!("== Halo 2 types with cutscene/recording/animation in the name");
    for (i, t) in c.types().iter().enumerate() {
        if t.contains("record") || t.contains("cutscene") || t.contains("animation") {
            println!("  type {i} {t}");
            for (j, f) in c.functions().iter().enumerate() {
                if f.parameters().contains(&(i as u16)) || f.return_type() == i as u16 {
                    let p: Vec<_> = f
                        .parameters()
                        .iter()
                        .map(|&p| c.type_name(p).unwrap_or("?"))
                        .collect();
                    println!(
                        "    #{j} {:?} ({}) -> {} usage {:?}",
                        f.name(),
                        p.join(" "),
                        c.type_name(f.return_type()).unwrap_or("?"),
                        f.usage()
                    );
                }
            }
        }
    }

    // How retail Halo 2 names a custom animation: a few string ID atoms
    // under calls of the animation builtins (their shape only matters).
    let named = c.named_functions();
    let anim_h2: BTreeSet<u16> = named
        .iter()
        .filter(|(_, n)| n.contains("custom_animation") || n.contains("animation_start"))
        .map(|(i, _)| *i)
        .collect();
    let mut shown = 0;
    for s in &scenarios {
        for n in s.nodes().iter().filter(|n| n.live() && !n.atom()) {
            if !anim_h2.contains(&n.opcode()) || n.script_call() || shown >= 6 {
                continue;
            }
            let mut a = s
                .node(hs::Datum(n.data()))
                .map(|h| h.next())
                .unwrap_or(hs::Datum::NULL);
            let mut toks = Vec::new();
            while a != hs::Datum::NULL {
                let Ok(x) = s.node(a) else { break };
                toks.push(s.string(x.string_offset()).unwrap_or("?").to_owned());
                a = x.next();
            }
            println!("  h2 {}: {toks:?}", named[&n.opcode()]);
            shown += 1;
        }
    }

    let anim_builtins = [
        "custom_animation",
        "unit_custom_animation_at_frame",
        "object_set_custom_animation",
        "scenery_animation_start",
        "scenery_animation_start_at_frame",
        "camera_set_animation",
        "custom_animation_list",
    ];
    for level in levels() {
        let h1 = h1_level(&level);
        let t = h1scripts::Tables::read(&h1).unwrap();
        let all = calls(&t);
        let mut counts: BTreeMap<String, (usize, Kind)> = BTreeMap::new();
        let mut graphs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut objects_by_graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut impulse = 0;
        let mut impulse_sounds = BTreeSet::new();
        for (i, args) in &all {
            let Ok(f) = catalog.function(*i) else {
                continue;
            };
            if !CUTSCENE_BUILTINS.contains(&f.name.as_str()) {
                continue;
            }
            let kind = table.builtin(*i).unwrap().kind;
            counts.entry(f.name.clone()).or_insert((0, kind)).0 += 1;
            if anim_builtins.contains(&f.name.as_str()) {
                let toks: Vec<_> = args.iter().map(|a| token(&t, a)).collect();
                let (obj, graph, anim) = if f.name == "camera_set_animation" {
                    (
                        String::from("<camera>"),
                        toks.first().cloned(),
                        toks.get(1).cloned(),
                    )
                } else {
                    (toks[0].clone(), toks.get(1).cloned(), toks.get(2).cloned())
                };
                if let (Some(g), Some(a)) = (graph, anim) {
                    graphs.entry(g.clone()).or_default().insert(a);
                    objects_by_graph.entry(g).or_default().insert(obj);
                }
            }
            if f.name == "sound_impulse_time" {
                impulse += 1;
                if let Some(a) = args.first() {
                    impulse_sounds.insert(token(&t, a));
                }
            }
        }
        println!("== {level}");
        for (n, (k, kind)) in &counts {
            println!("  {n}: {k} ({kind:?})");
        }
        println!(
            "  sound_impulse_time: {impulse} calls over {} sounds",
            impulse_sounds.len()
        );
        let antr = h1map::Group::new(b"antr");
        let mut total_nodes = 0;
        for (g, anims) in &graphs {
            let Some(tag) = h1.find(antr, g) else {
                println!(
                    "  graph {g}: not in the map ({} animations named)",
                    anims.len()
                );
                continue;
            };
            let graph = h1models::h1::Graph::read(&h1, tag.data_address).unwrap();
            let named: Vec<_> = anims
                .iter()
                .filter(|a| {
                    graph
                        .animations
                        .iter()
                        .any(|x| x.name.eq_ignore_ascii_case(a))
                })
                .collect();
            total_nodes += graph.nodes.len();
            println!(
                "  graph {g}: {} nodes, {} animations ({} named, {} found), objects {:?}",
                graph.nodes.len(),
                graph.animations.len(),
                anims.len(),
                named.len(),
                objects_by_graph[g].iter().take(4).collect::<Vec<_>>()
            );
        }
        let _ = total_nodes;
        // Recorded animations: Halo 1 scnr +0x36c, 0x40 each.
        let scnr = h1.tag_data(h1.scenario()).unwrap();
        let count = u32::from_le_bytes(scnr[0x36c..0x370].try_into().unwrap());
        println!("  recorded animations: {count}");
    }
}

fn scripting() -> h1campaign::Scripting {
    let mut iso = h1_iso();
    let e = iso.find("/default.xbe").unwrap();
    let h1_xbe = iso.read(&e).unwrap();
    let (h2_xbe, scenarios) = h2_scenarios();
    h1campaign::Scripting::new(&h1_xbe, &h2_xbe, &scenarios).unwrap()
}

fn h2_map(name: &str) -> map::Map<Cursor<Vec<u8>>> {
    let mut iso = h2_iso();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with(&format!("/{name}.map")))
        .unwrap();
    map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn donors() -> h1campaign::donor::Donors {
    let mut d = h1campaign::donor::Donors::default();
    let mut iso = h2_iso();
    let entries = iso.entries().unwrap();
    for name in h1campaign::donor::MAPS {
        let e = entries
            .iter()
            .find(|e| e.path.ends_with(&format!("/{name}.map")))
            .unwrap();
        d.maps.push((name.to_owned(), iso.read(e).unwrap()));
    }
    d
}

/// Script calls (by name) anywhere under node `d`.
fn script_calls(t: &h1scripts::Tables, d: hs::Datum, out: &mut BTreeSet<String>, depth: usize) {
    let Ok(n) = t.node(d) else { return };
    if n.atom() || depth > 400 {
        return;
    }
    let Ok(head) = t.node(hs::Datum(n.data())) else {
        return;
    };
    if n.script_call() {
        out.insert(token(t, head));
    }
    let mut a = head.next();
    while a != hs::Datum::NULL {
        script_calls(t, a, out, depth + 1);
        a = t.node(a).map(|x| x.next()).unwrap_or(hs::Datum::NULL);
    }
}

/// The scripts a level's skippable cinematics run: those called under
/// `(if (cinematic_skip_start) ...)`, and every script they call.
fn cinematic_scripts(t: &h1scripts::Tables, c: &h1scripts::Catalog) -> BTreeSet<String> {
    let if_ = c.index_of("if").unwrap();
    let mut roots = BTreeSet::new();
    for n in t
        .nodes
        .iter()
        .filter(|n| n.live() && !n.atom() && !n.script_call())
    {
        if n.opcode() != if_ {
            continue;
        }
        let Ok(head) = t.node(hs::Datum(n.data())) else {
            continue;
        };
        let Ok(cond) = t.node(head.next()) else {
            continue;
        };
        if !cond.script_call() || cond.atom() {
            continue;
        }
        let Ok(ch) = t.node(hs::Datum(cond.data())) else {
            continue;
        };
        if token(t, ch) != h1scripts::compile::SKIP_GUARD {
            continue;
        }
        script_calls(t, cond.next(), &mut roots, 0);
    }
    let mut all = roots.clone();
    let mut todo: Vec<String> = roots.into_iter().collect();
    while let Some(s) = todo.pop() {
        let Some(script) = t.scripts.iter().find(|x| x.name == s) else {
            continue;
        };
        let mut found = BTreeSet::new();
        script_calls(t, script.root, &mut found, 0);
        for f in found {
            if f != h1scripts::compile::SKIP_GUARD && all.insert(f.clone()) {
                todo.push(f);
            }
        }
    }
    all
}

#[test]
fn default_campaign_cinematics_play_every_authored_path() {
    use h1scripts::compile::Cutscenes;
    for level in LEVELS {
        assert_eq!(h1campaign::cutscenes::policy(&Default::default(), level), Cutscenes::Play);
    }
    assert_eq!(Cutscenes::default(), Cutscenes::Play);
}

fn command_targets(t: &h1scripts::Tables, d: hs::Datum, out: &mut Vec<u16>) {
    let n = t.node(d).unwrap();
    if n.atom() { return; }
    let head = t.node(hs::Datum(n.data())).unwrap();
    let mut arg = head.next();
    if !n.script_call() && matches!(token(t, head).as_str(), "ai_command_list" | "ai_command_list_by_unit") {
        let list = t.node(t.node(arg).unwrap().next()).unwrap();
        out.push(list.data() as u16);
    }
    while arg != hs::Datum::NULL {
        command_targets(t, arg, out);
        arg = t.node(arg).unwrap().next();
    }
}

fn command_script_links(level: &str, t: &h1scripts::Tables, output: &hs::Scenario, cinematic: &BTreeSet<String>) {
    let mut count = 0;
    for script in t.scripts.iter().filter(|s| matches!(level, "c10" | "c40") || cinematic.contains(&s.name)) {
        let mut source = Vec::new();
        command_targets(t, script.root, &mut source);
        if source.is_empty() { continue; }
        let mut written = Vec::new();
        let mut nodes = vec![output.script(&script.name).unwrap().root()];
        while let Some(node) = nodes.pop() {
            let hs::Expression::Call { name, children, .. } = output.expression(node).unwrap() else { continue };
            let args = children.collect::<hs::Result<Vec<_>>>().unwrap();
            if name == "cs_run_command_script" {
                let arg = output.node(args[1]).unwrap();
                if arg.data() == u32::MAX {
                    written.push(u16::MAX);
                    continue;
                }
                let target = &output.scripts()[arg.data() as u16 as usize];
                assert_eq!(target.kind(), 5);
                written.push(target.name().unwrap().strip_prefix("h1_cmd_").unwrap().parse::<u16>().unwrap());
            }
            nodes.extend(args.into_iter().rev());
        }
        assert_eq!(written,source,"{} command-list targets",script.name);
        count += source.len();
    }
    match level {
        "c10" => assert_eq!(count, 10, "x50 crew and Monitor command lists"),
        "c40" => assert_eq!(count, 20, "Monitor and sentinel command lists"),
        _ => {}
    }
}

/// Per level, played: each cutscene's builtins that are still stubbed, the
/// hooks a cutscene needs that do not resolve, the graphs and recordings
/// written, and the tag memory; then the converted
/// map read back: the recorded animations block (`scnr +0x110`) holds
/// Halo 1's in order with their streams, each graph the scripts name is a
/// `jmad`.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn cutscenes_convert_and_read_back() {
    use h1scripts::compile::{Cutscenes, Missing, compile_with};
    let s = scripting();
    let donors = donors();
    let mut failed = Vec::new();
    for level in levels() {
        let h1 = h1_level(&level);
        let t = h1scripts::Tables::read(&h1).unwrap();
        let cinematic = cinematic_scripts(&t, &s.catalog);
        let played = compile_with(
            &t,
            &s.catalog,
            &s.table,
            &s.target,
            Missing::Stub,
            Cutscenes::Play,
            &|_, _| true,
            &[],
        )
        .unwrap();
        let mut stubbed: BTreeMap<&str, usize> = BTreeMap::new();
        for m in played
            .missing
            .iter()
            .filter(|m| cinematic.contains(&m.definition))
        {
            *stubbed.entry(m.name.as_str()).or_default() += 1;
        }
        // The recordings play: calls reach Halo 2's recording builtins
        // (138 to 142) wherever Halo 1's scripts make them.
        let recording_calls = played
            .nodes
            .iter()
            .filter(|n| !n.atom() && !n.script_call() && (138..=142).contains(&n.opcode()))
            .count();
        println!("  calls of the recording builtins: {recording_calls}");
        if let Some(m) = played
            .missing
            .iter()
            .find(|m| m.name.starts_with("recording_") || m.name == "vehicle_hover")
        {
            failed.push(format!("{level}: {} still missing", m.name));
        }
        println!("== {level}: {} cinematic scripts", cinematic.len());
        println!("  stubbed in cinematics: {stubbed:?}");

        let opts = h1campaign::Options {
            scripts: Some(&s),
            sounds: usize::MAX,
            donors: Some(&donors),
            ..Default::default()
        };
        let mut base = h2_map(h1campaign::base_for(&level));
        let done = match h1campaign::convert(&h1, &mut base, &opts) {
            Ok(d) => d,
            Err(e) => {
                let without = h1campaign::convert(
                    &h1,
                    &mut h2_map(h1campaign::base_for(&level)),
                    &h1campaign::Options {
                        cutscene_data: false,
                        ..h1campaign::Options {
                            scripts: Some(&s),
                            sounds: usize::MAX,
                            donors: Some(&donors),
                            ..Default::default()
                        }
                    },
                )
                .map(|d| (d.report.memory, d.report.lean, d.bytes.len()));
                println!("  FAILED: {e}; without graphs and recordings: {without:x?}");
                failed.push(format!("{level}: {e}"));
                continue;
            }
        };
        let without = h1campaign::convert(
            &h1,
            &mut h2_map(h1campaign::base_for(&level)),
            &h1campaign::Options {
                cutscene_data: false,
                ..h1campaign::Options {
                    scripts: Some(&s),
                    sounds: usize::MAX,
                    donors: Some(&donors),
                    cutscenes: Some(Cutscenes::Play),
                    ..Default::default()
                }
            },
        )
        .map(|d| (d.report.memory, d.report.lean, d.bytes.len()));
        println!("  without graphs and recordings: {without:x?}");
        let r = &done.report;
        let sr = r.scenario.scripts.as_ref().unwrap();
        println!(
            "  tag memory {:#x} of {:#x}, lean {:?}, file {:#x}, graph data {:#x}",
            r.memory,
            r.metadata_max,
            r.lean,
            done.bytes.len(),
            sr.graphs.raw_bytes
        );
        println!(
            "  appended: {:x?}",
            r.appended.iter().take(4).collect::<Vec<_>>()
        );
        println!(
            "  graphs converted {:?}, shared {:?}, dropped {:?}",
            sr.graphs.converted, sr.graphs.shared, sr.graphs.dropped
        );
        println!(
            "  recordings {} ({} bytes of event streams)",
            sr.recordings, sr.recording_bytes
        );
        println!("  hooks left: {:?}", sr.unresolved);

        // Read back.
        let mut m = map::Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
        m.validate_checksum().unwrap();
        if level == "d40" {
            for (place, input) in h1convert::shad::COUNTDOWN_DIGITS.iter().enumerate().skip(3).take(4) {
                assert!(m.strings().iter().any(|s| s.as_deref() == Some(*input)));
                let shader = format!("levels\\d40\\shaders\\sd numbers_{place}");
                assert!(m.tags().iter().any(|t| {
                    t.group == map::FourCC(*b"shad") && t.name.as_deref() == Some(shader.as_str())
                }), "converted countdown shader {shader:?}");
            }
        }
        let tag = m.scenario().unwrap().clone();
        let bytes = m.read_tag_bytes(tag.id).unwrap();
        let w = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        let level_data = h1scripts::level::Level::read(&h1).unwrap();
        for (field, count) in [
            (h1scripts::level::h2::CAMERA_POINTS.0, level_data.camera_points.len()),
            (h1scripts::level::h2::FLAGS.0, level_data.flags.len()),
            (h1scripts::level::h2::TITLES.0, level_data.titles.len()),
        ] {
            assert_eq!(w(field as usize) as usize, count, "{level}: scenario +{field:#x}");
        }
        assert_eq!(r.lean, h1campaign::Lean::Everything, "{level}: {:?}", r.retries);
        assert!(sr.graphs.dropped.is_empty(), "{level}: {:?}", sr.graphs.dropped);
        for h in played.hooks.iter().filter(|h| h.type_name == "animation_graph"
            && h.h1_type != "command_unit_graph_candidate" && h1.find(h1map::Group::new(b"antr"), &h.token).is_some())
        {
            assert!(m.tags().iter().any(|tag| tag.group == map::FourCC(*b"jmad")
                && tag.name.as_deref() == Some(h.token.as_str()) && tag.location.is_some()),
                "{level}: missing cinematic graph {}", h.token);
        }
        let recs = h1campaign::cutscenes::read_recordings(&h1).unwrap();
        let scripts = hs::Scenario::read(&mut m).unwrap();
        command_script_links(&level, &t, &scripts, &cinematic);
        if level == "a10" {
            let wake = s.catalog.index_of("wake").unwrap();
            let expected = calls(&t).into_iter().filter(|(op, args)| {
                *op == wake && t.scripts.get(args[0].data() as u16 as usize)
                    .is_some_and(|target| target.name == "tutorial_setup")
            }).count();
            assert!(expected > 0, "the original tutorial must be reachable");
            let mut actual = 0;
            for n in scripts.nodes().iter().filter(|n| n.live() && !n.atom() && !n.script_call()) {
                let head = scripts.node(hs::Datum(n.data())).unwrap();
                if scripts.string(head.string_offset()).unwrap() == "wake" {
                    let arg = scripts.node(head.next()).unwrap();
                    if scripts.scripts().get(arg.data() as u16 as usize)
                        .is_some_and(|target| target.name().ok() == Some("tutorial_setup")) {
                        actual += 1;
                    }
                }
            }
            assert_eq!(actual, expected, "a10 tutorial wake targets");
        }
        if level == "b40" {
            let mut roots = BTreeMap::new();
            let mut reused = 0;
            for g in scripts.globals().iter().filter(|g| g.name().unwrap().starts_with("h1_ai_state_")) {
                let value = scripts.node(g.root()).unwrap().data();
                if let Some(existing) = roots.insert((g.value_type(), value), g.root()) {
                    assert_eq!(existing, g.root());
                    reused += 1;
                }
            }
            assert!(reused > 0, "b40 should have shared AI initializer roots");
            println!("b40 shared {reused} initializer nodes, saving {} bytes", reused * hs::Node::SIZE);
        }
        for u in sr.missing_builtins.iter().filter(|u| cinematic.contains(&u.definition)) {
            failed.push(format!("{level}: {} still drops {}", u.definition, u.name));
        }
        if let Some(op) = s.catalog.index_of("cinematic_screen_effect_set_video")
            && calls(&t).iter().any(|(index, _)| *index == op) {
            for name in h1scripts::cinematics::VIDEO_BITMAPS {
                assert!(scripts.references().iter().any(|r| r.group() == map::FourCC(*b"bitm") &&
                    m.tag(r.id()).is_ok_and(|t| t.name.as_deref() == Some(name) && t.location.is_some())));
            }
        }
        let ty = s.target.type_index("cutscene_recording").unwrap();
        for node in scripts.nodes().iter().filter(|n| {
            n.atom() && !n.global() && n.value_type() == ty && n.opcode() == ty
        }) {
            let name = scripts.string(node.string_offset()).unwrap();
            if name.is_empty() || name.eq_ignore_ascii_case("none") {
                assert_eq!(node.data(), u32::MAX);
            } else {
                assert_eq!(
                    h1campaign::cutscenes::recording_index(&recs, name),
                    Some(node.data()),
                    "{level} recording literal {name}"
                );
            }
        }
        let (n, at) = (w(0x110) as usize, w(0x114));
        if n != recs.len() {
            failed.push(format!(
                "{level}: {n} recordings written, {} in Halo 1",
                recs.len()
            ));
        }
        let space = m.metadata_space().unwrap();
        let meta = m
            .read_file_range(space.file_offset, u64::from(space.size))
            .unwrap();
        let mem = |a: u32, len: usize| {
            let o = (a - space.virtual_base) as usize;
            meta[o..o + len].to_vec()
        };
        let mut recs = recs;
        h1campaign::recordings::for_halo2(&mut recs).unwrap();
        for (i, rec) in recs.iter().enumerate().take(n) {
            let e = mem(at + (i * 0x34) as u32, 0x34);
            let size = u32::from_le_bytes(e[0x2c..0x30].try_into().unwrap()) as usize;
            let p = u32::from_le_bytes(e[0x30..0x34].try_into().unwrap());
            if &e[..rec.name.len()] != rec.name.as_bytes()
                || e[rec.name.len()..32].iter().any(|b| *b != 0)
                || e[0x20..0x24] != [rec.version, 0, rec.control_version, 0]
                || e[0x24..0x26] != rec.ticks.to_le_bytes()
                || e[0x26..0x2c] != [0; 6]
                || size != rec.stream.len()
            {
                failed.push(format!("{level}: recording {i} differs"));
            } else if size > 0 && mem(p, size) != rec.stream {
                failed.push(format!("{level}: recording {i}'s stream differs"));
            } else if let Err(e) = h1campaign::recordings::walk(
                h1campaign::recordings::Game::Halo2,
                rec.control_version,
                &mem(p, size),
            ) {
                failed.push(format!("{level}: recording {i} as Halo 2 reads it: {e}"));
            }
            if !(1..=4).contains(&rec.version) {
                failed.push(format!("{level}: recording {i} version {}", rec.version));
            }
        }
        let versions: BTreeSet<(u8, u8)> = recs
            .iter()
            .map(|r| (r.version, r.control_version))
            .collect();
        println!("  recording (version, unit control version): {versions:?}");
        let jmad = map::FourCC(*b"jmad");
        for (g, _) in &sr.graphs.converted {
            if !m
                .tags()
                .iter()
                .any(|t| t.group == jmad && t.name.as_deref() == Some(g.as_str()))
            {
                failed.push(format!("{level}: graph {g} not in the map"));
            }
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// A converted enter-vehicle call must use the exact seat label on the
/// vehicle it names. Check the source placements, vehicle presets and
/// rebuilt seat names on all ten campaign levels.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn seat_names_name_seats_the_vehicles_carry() {
    use h1campaign::scenario::h1 as scnr;
    use h1scripts::compile::SEAT_NAME;
    let s = scripting();
    let enter = s.target.builtins["unit_enter_vehicle"].index;
    let mut matched = 0;
    for level in levels() {
        let h1 = h1_level(&level);
        let t = h1scripts::Tables::read(&h1).unwrap();
        let compiled = h1scripts::compile::compile(
            &t,
            &s.catalog,
            &s.table,
            &s.target,
            h1scripts::compile::Missing::Stub,
            &[],
        )
        .unwrap();
        let sc = h1.tag_data(h1.scenario()).unwrap();
        let space = h1.tag_space();
        let names = space.block_at(sc, scnr::OBJECT_NAMES.0, scnr::OBJECT_NAMES.1).unwrap();
        let vehicles = space.block_at(sc, scnr::VEHICLES.0, scnr::VEHICLES.1).unwrap();
        let palette = space.block_at(sc, scnr::VEHICLE_PALETTE.0, scnr::VEHICLE_PALETTE.1).unwrap();
        let base = 0x8000_0000;
        let decoded = hs::Scenario::decode(&compiled.scenario_bytes(base), base).unwrap();
        let hooks: BTreeMap<_, _> = compiled.hooks.iter().map(|h| (h.slot, h)).collect();
        for call in decoded.nodes().iter().filter(|n| !n.atom() && !n.script_call() && n.opcode() == enter) {
            let head = decoded.node(hs::Datum(call.data())).unwrap();
            let rider = decoded.node(head.next()).unwrap();
            let vehicle_datum = rider.next();
            let seat_datum = decoded.node(vehicle_datum).unwrap().next();
            let vehicle = hooks[&(vehicle_datum.slot() as u16)];
            let seat = hooks[&(seat_datum.slot() as u16)];
            assert_eq!(seat.h1_type, SEAT_NAME);
            let name = names.iter().find(|entry| {
                let len = entry[..32].iter().position(|&c| c == 0).unwrap_or(32);
                entry[..len].eq_ignore_ascii_case(vehicle.token.as_bytes())
            }).unwrap_or_else(|| panic!("{level}: vehicle {:?} has no name", vehicle.token));
            let index = u16::from_le_bytes([name[0x22], name[0x23]]);
            let placement = vehicles[usize::from(index)];
            let index = u16::from_le_bytes([placement[0], placement[1]]);
            let tag = h1map::TagRef::read(palette[usize::from(index)], 0).unwrap();
            let unit = h1objects::h1::Unit::read(&h1, h1.tag(tag.id).unwrap()).unwrap();
            let expected = h1models::names::mode_name(&seat.token);
            assert!(
                unit.seats.iter().any(|s| h1objects::rebuild::seat_names(s, &[]).label == expected),
                "{level}: vehicle {:?} has no seat {expected:?}",
                vehicle.token
            );
            matched += 1;
        }
    }
    assert!(matched > 0);
}

/// The equipment adapter's starting profiles resolve in rebuilt scenario
/// order. Check the spelling against the source's encoded index.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn player_add_equipment_names_rebuilt_profiles() {
    let scripting = scripting();
    let add = scripting.catalog.index_of("player_add_equipment").unwrap();
    let mut count = 0;
    for level in levels() {
        let h1 = h1_level(&level);
        let profiles = h1campaign::profiles::read(&h1).unwrap();
        let tables = h1scripts::Tables::read(&h1).unwrap();
        for (index, args) in calls(&tables).into_iter().filter(|(index, _)| *index == add) {
            let name = tables.string(args[1].string_offset()).unwrap();
            let expected = u32::from(args[1].data() as u16);
            assert_eq!(
                h1campaign::profiles::script_index(&profiles, name),
                Some(expected),
                "{level}: builtin {index} profile {name:?}",
            );
            count += 1;
        }
    }
    assert!(count > 0);
}
