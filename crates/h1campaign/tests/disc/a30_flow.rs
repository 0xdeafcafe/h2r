//! Assertions over individual script bodies in the converted disc map.
//! A matching call somewhere else on the level cannot satisfy these checks.
use hs::{Datum, Expression, Scenario};

struct Call<'a> {
    name: &'a str,
    opcode: u16,
    args: Vec<Datum>,
}

fn calls<'a>(s: &'a Scenario, script: &str) -> Vec<Call<'a>> {
    let mut out = Vec::new();
    let mut pending = vec![s.script(script).unwrap().root()];
    while let Some(d) = pending.pop() {
        let (name, opcode, children) = match s.expression(d).unwrap() {
            Expression::Call { name, function, children, .. } => (name, function, children),
            Expression::ScriptCall { script, index, children, .. } => {
                let name = script.name().unwrap();
                // Multi-squad AI operations share a generated static body.
                if name.starts_with("h1_ai_") {
                    pending.push(script.root());
                }
                (name, index, children)
            }
            _ => continue,
        };
        let args = children.collect::<hs::Result<Vec<_>>>().unwrap();
        pending.extend(args.iter().rev().copied());
        out.push(Call { name, opcode, args });
    }
    out
}

fn atom<'a>(s: &'a Scenario, d: Datum) -> (&'a str, u32) {
    let n = s.node(d).unwrap();
    assert!(n.atom());
    (s.string(n.string_offset()).unwrap(), n.data())
}

pub(super) fn mission(s: &Scenario, ai: &h1ai::h2::ScenarioAi, source: &h1map::H1Map) {
    let (threads, cells) = engine::hs_pool::capacities(s.globals().len() as u32).unwrap();
    assert!(cells > 3072, "a30 must include the generated families' state");
    assert!(s.scripts().iter().filter(|sc| sc.kind() <= 2).count() < threads as usize);
    let script = s.script("mission_a30").unwrap();
    assert_eq!(script.kind(), 0);
    let main = calls(s, "mission_a30");
    let wake = |c: &Call<'_>| {
        let (name, index) = atom(s, c.args[0]);
        assert_eq!(s.scripts()[index as u16 as usize].name().unwrap(), name);
        name
    };
    let main_wakes: Vec<_> = main.iter().filter(|c| c.name == "wake").map(wake).collect();
    assert_eq!(main_wakes, ["setup_dead", "objectives_a30", "music_a30", "save_mission_start",
        "mission_lz", "mission_first", "mission_cave", "mission_cliff", "mission_rubble", "mission_river", "mission_obj"]);
    let intro = calls(s, "cutscene_intro");
    // The original lifepod instruction, in the intro itself, makes one
    // convolution request (1, 1, 3.0, 0.0, 7.0) and timestamps its ramp.
    use h1scripts::cinematics::Field::*;
    let required = [(Passes, 1), (Kernel, 1), (RadiusFrom, 3f32.to_bits()),
        (RadiusTo, 0), (BlurSeconds, 7f32.to_bits())];
    assert!(intro.iter().filter(|c| c.name == "begin").any(|body| {
        body.args.len() >= 5 && required.iter().zip(&body.args).all(|(&(field, bits), &arg)| {
            let Ok(Expression::Call { name: "set", children, .. }) = s.expression(arg) else { return false };
            let Ok(args) = children.collect::<hs::Result<Vec<_>>>() else { return false };
            let [variable, value] = args.as_slice() else { return false };
            let (name, index) = atom(s, *variable);
            let n = s.node(*value).unwrap();
            n.atom() && !n.global() && name == field.name()
                && s.globals()[index as usize].name().unwrap() == name
                && (if field.ty() == 7 { n.data() & 0xffff } else { n.data() }) == bits
        })
    }), "missing authored lifepod blur");
    assert!(main.iter().any(|c| c.name == "cutscene_intro"));
    for (body, title, index) in [(&intro, "arrival", 0), (&main, "reunion", 1)] {
        let call = body.iter().find(|c| c.name == "cinematic_set_title").unwrap();
        assert_eq!(atom(s, call.args[0]), (title, index));
    }
    for (script, save) in [("save_mission_start", "game_save_immediate"), ("save_cave_exit", "game_save"),
        ("save_first_wave_1", "game_save_no_timeout")] {
        assert!(calls(s, script).iter().any(|c| c.name == save), "{script}: {save}");
    }
    let skip = calls(s, "cinematic_skip_start");
    let guards: Vec<_> = skip.iter().filter(|c| matches!(c.name, "cinematic_skip_start_internal" |
        "game_save_immediate" | "game_saving" | "game_reverted")).map(|c| c.name).collect();
    assert_eq!(guards, ["cinematic_skip_start_internal", "game_save_immediate", "game_saving", "game_reverted"]);

    let refs = h1text::h1::scenario_text(source).unwrap();
    let messages = h1text::h1::Hmt::read(source, source.tag(refs.hud_messages.unwrap().id).unwrap()).unwrap();
    let objectives = calls(s, "objectives_a30");
    for (setter, expected) in [
        ("hud_set_objective_text", ["dia_evade", "dia_protect", "dia_search1", "dia_search2", "dia_search3"]),
        ("hud_set_help_text", ["obj_evade", "obj_protect", "obj_search", "obj_search2", "obj_search3"]),
    ] {
        let actual: Vec<_> = objectives.iter().filter(|call| call.name == setter).map(|call| {
            let (token, index) = atom(s, call.args[0]);
            assert_eq!(messages.messages[index as u16 as usize].name, token);
            token
        }).collect();
        assert_eq!(actual, expected);
    }

    let group = |name| ai.squad_groups.iter().position(|g| g.name == name).unwrap();
    let startup = calls(s, h1campaign::scenario::INITIAL_SCRIPT);
    let placed: Vec<_> = startup.iter().filter(|c| c.name == "ai_place")
        .map(|c| atom(s, c.args[0])).collect();
    assert_eq!(placed, [("mid_cliff_1", 0x4000_0000 | group("mid_cliff_1") as u32)]);
    assert_eq!(main[1].name, h1campaign::scenario::INITIAL_SCRIPT);
    let toon = "first_wave/wave_1_lz_toon";
    let g = group(toon);
    assert_eq!(ai.squad_groups[g].parent, group("first_wave") as i16);
    let members: Vec<_> = ai.squads.iter().filter(|q| q.parent == g as i16).map(|q| q.name.as_str()).collect();
    assert_eq!(members.len(), 2);
    assert!(members.contains(&"first_wave/wave_1_lz_grunt"));
    assert!(members.contains(&"first_wave/wave_1_lz_elite"));
    let wave = calls(s, "mission_first_wave_1");
    let place = wave.iter().position(|c| c.name == "ai_place").unwrap();
    let board = wave.iter().position(|c| c.name == "vehicle_load_magic").unwrap();
    assert!(place < board);
    assert_eq!(atom(s, wave[place].args[0]), (toon, 0x4000_0000 | g as u32));
    let Expression::Call {name: "ai_actors", mut children, ..} = s.expression(wave[board].args[2]).unwrap()
        else { panic!("the passenger list must contain the actors of the landing platoon") };
    assert_eq!(atom(s, children.next().unwrap().unwrap()), (toon, 0x4000_0000 | g as u32));

    let rubble = calls(s, "mission_rubble_wave_2");
    let moves: Vec<_> = rubble.iter().filter(|c| c.name == "ai_migrate")
        .map(|c| (s.node(c.args[0]).unwrap().data(), s.node(c.args[1]).unwrap().data())).collect();
    let squad = |name| ai.squads.iter().position(|q| q.name == name).unwrap() as u32;
    for from in ["rubble_wave/wave_2_jackal", "rubble_wave/wave_2_elite"] {
        assert!(moves.contains(&(squad(from), squad("rubble_wave/wave_2_attack"))), "{from}");
    }
    assert!(moves.contains(&(squad("rubble_wave/wave_2_attack"), squad("rubble_wave/wave_2_defend"))));

    let first = calls(s, "mission_first");
    for (gate, opcode) in [("game_all_quiet", 587), ("game_safe_to_speak", 586)] {
        assert!(first.iter().any(|c| c.name == gate && c.opcode == opcode));
    }
    let timer = s.globals().iter().position(|g| g.name().unwrap() == "global_timer").unwrap();
    for site in ["cliff", "rubble", "river"] {
        let extract = calls(s, &format!("mission_extraction_{site}"));
        assert_eq!(extract.iter().filter(|c| c.name == "game_time").count(), 3);
        assert!(extract.iter().any(|c| c.name == "set" && atom(s, c.args[0]) == ("global_timer", timer as u32)));
        assert!(extract.iter().any(|c| c.name == "recording_play_and_hover"));
        assert!(extract.iter().any(|c| c.name == "vehicle_load_magic"));
        assert_eq!(extract.last().unwrap().name, "game_won");
    }
}

pub(super) fn titles(m: &mut map::Map<std::io::Cursor<Vec<u8>>>, bytes: &[u8], addr: u32, source: &h1map::H1Map) {
    let level = h1scripts::level::Level::read(source).unwrap();
    let reference = h1text::h1::scenario_text(source).unwrap().help_text.unwrap();
    let list = h1text::h1::Ustr::read(source, source.tag(reference.id).unwrap()).unwrap();
    let id = map::TagId(u32::from_le_bytes(bytes[0x204..0x208].try_into().unwrap()));
    let locale = tags::Localization::read(m).unwrap();
    let text = locale.unic(m, id).unwrap();
    let view = tags::geometry::TagView::new(bytes, addr);
    let rows = view.block(bytes, 0x1f0, 0x24).unwrap();
    assert_eq!(rows.len(), 2);
    for (row, title) in rows.into_iter().zip(level.titles) {
        let key = tags::StringId(u32::from_le_bytes(row[..4].try_into().unwrap()));
        let displayed = locale.text_in(&text, key, tags::Language::English).unwrap().unwrap();
        assert_eq!(displayed, list.strings[title.text_index as usize]);
    }
}
