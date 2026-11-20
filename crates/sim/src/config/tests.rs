use std::path::Path;

use super::*;

const MATCH: &str = r#"
map = "lockout"
game_type = "slayer"
time_limit_s = 300
score_limit = 25
team_play = true
seed = 7
max_ticks = 9000
pace = "x4"

[settings]
83 = 1

[[bot]]
name = "A1"
skill = "legendary"
team = 0
params = "tuning/aggr.toml"

[[bot]]
name = "B1"
team = 1
[bot.tuning]
aggression = 0.9
reaction = 5
"#;

fn no_files(_: &Path) -> Result<BotParams, String> {
    Err("no files here".into())
}

fn aggr(p: &Path) -> Result<BotParams, String> {
    assert_eq!(p, Path::new("tuning/aggr.toml"));
    Ok(BotParams {
        skill: Some("easy".into()),
        aggression: Some(0.8),
        reaction: Some(9),
        ..BotParams::default()
    })
}

#[test]
fn reads_a_match_file() {
    let c = parse_match(MATCH, false).unwrap();
    let m = resolve(c, &aggr).unwrap();
    assert_eq!(m.game_type, GameType::Slayer);
    assert_eq!(m.pace, Pace::Times(4.0));
    assert!(m.team_play);
    assert_eq!(
        m.variant,
        [
            (TIME_LIMIT_SETTING, 300),
            (2, 25),
            (TEAM_PLAY_SETTING, 1),
            (83, 1)
        ]
    );
    assert_eq!(m.bots.len(), 2);
    let a = &m.bots[0];
    assert_eq!((a.port, a.name.as_str(), a.team), (Some(0), "A1", Some(0)));
    // The bot's own skill wins over its file's.
    assert_eq!(a.skill, Skill::Legendary);
    assert_eq!(a.params.aggression, Some(0.8));
    assert_eq!(a.params.reaction, Some(9));
    let b = &m.bots[1];
    assert_eq!(b.skill, Skill::Normal);
    assert_eq!(b.params.reaction, Some(5));
    assert_eq!(b.params.skill.as_deref(), Some("normal"));
    assert_ne!(a.seed, b.seed);
    assert_eq!(a.seed, bot_seed(7, 0));
}

#[test]
fn json_too() {
    let c = parse_match(
        r#"{"map": "midship", "game_type": "CTF", "bot": [{"skill": "heroic"}, {}]}"#,
        true,
    )
    .unwrap();
    let m = resolve(c, &no_files).unwrap();
    assert_eq!(m.game_type, GameType::Ctf);
    assert_eq!(m.pace, Pace::Unpaced);
    assert_eq!(m.bots[1].name, "Clanker 1");
    // Free-for-all: no teams unless given.
    assert_eq!(m.bots[0].team, None);
    assert!(m.variant.is_empty());
}

#[test]
fn unknown_fields_are_refused() {
    assert!(parse_match("map = \"lockout\"\nmpa = 1\n", false).is_err());
    assert!(parse_match("map = \"lockout\"\n[[bot]]\nskil = \"easy\"\n", false).is_err());
    let e = parse_match(
        "map = \"x\"\n[[bot]]\n[bot.tuning]\nagression = 1.0\n",
        false,
    );
    assert!(e.is_err());
}

#[test]
fn every_problem_is_listed() {
    let c = parse_match(
        r#"
map = ""
game_type = "firefight"
pace = "warp"
max_ticks = 0
[settings]
200 = 1
x = 2
[[bot]]
skill = "godlike"
team = 9
params = "missing.toml"
[bot.tuning]
aggression = 2.0
fire_cone_deg = -1.0
burst = false
burst_on = 3
"#,
        false,
    )
    .unwrap();
    let e = resolve(c, &no_files).unwrap_err();
    for want in [
        "map: empty",
        "game_type",
        "pace",
        "max_ticks = 0",
        "settings.200",
        "settings.x",
        "godlike",
        "team 9",
        "params: no files here",
        "aggression = 2",
        "fire_cone_deg = -1",
        "burst = false",
    ] {
        assert!(e.contains(want), "{want:?} missing from:\n{e}");
    }
}

#[test]
fn forcing_phantom_seats_keeps_the_logical_lineup_and_batch_arms() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/runtime/sim-examples");
    let path = dir.join("lockout-2v2.toml");
    let original = load_match(&path).unwrap();
    let all = load_match_with_seating(&path, true).unwrap();
    assert!(all.distributed() && all.playerless_host());
    assert!(!original.playerless_host());
    assert_eq!(all.bots.len(), original.bots.len());
    for (mut expected, actual) in original.bots.into_iter().zip(&all.bots) {
        expected.port = None;
        expected.kind = SeatKind::Phantom;
        assert_eq!(&expected, actual);
    }
    let path = dir.join("batch.toml");
    let batch = load_batch(&path).unwrap();
    let all = load_batch_with_seating(&path, true).unwrap();
    assert_eq!(all.runs.len(), batch.runs.len());
    for (mut expected, actual) in batch.runs.into_iter().zip(all.runs) {
        for bot in &mut expected.config.bots {
            bot.kind = SeatKind::Phantom;
        }
        assert_eq!(expected, actual);
    }
    assert!(
        load_batch_with_seating(&dir.join("midship-seat-swap.toml"), true)
            .unwrap_err()
            .contains("H2_BOTS_ALL_PHANTOM=0")
    );
}

#[test]
fn the_host_peer_leaves_room_for_fifteen_phantoms() {
    let mut c = parse_match(&format!("map='midship'\n{}", "[[bot]]\n".repeat(15)), false).unwrap();
    c.force_phantom_seats(false).unwrap();
    let m = resolve(c.clone(), &no_files).unwrap();
    assert!(m.bots.iter().all(|b| b.port.is_none()));
    c.bots.push(c.bots[0].clone());
    assert!(
        resolve(c, &no_files)
            .unwrap_err()
            .contains("at most 15 phantoms")
    );
}

#[test]
fn bot_counts() {
    let none = parse_match("map = \"lockout\"\n", false).unwrap();
    assert!(
        resolve(none, &no_files)
            .unwrap_err()
            .contains("at least one bot")
    );
    let five = parse_match(
        &format!("map = \"lockout\"\n{}", "[[bot]]\n".repeat(5)),
        false,
    )
    .unwrap();
    assert!(
        resolve(five, &no_files)
            .unwrap_err()
            .contains("System Link")
    );
}

#[test]
fn team_play_fills_teams_and_needs_two() {
    let c = parse_match(
        "map = \"lockout\"\nteam_play = true\n[[bot]]\n[[bot]]\n[[bot]]\n[[bot]]\n",
        false,
    )
    .unwrap();
    let m = resolve(c, &no_files).unwrap();
    let teams: Vec<_> = m.bots.iter().map(|b| b.team).collect();
    assert_eq!(teams, [Some(0), Some(1), Some(0), Some(1)]);
    let one = parse_match(
        "map = \"lockout\"\nteam_play = true\n[[bot]]\nteam = 1\n[[bot]]\nteam = 1\n",
        false,
    )
    .unwrap();
    assert!(resolve(one, &no_files).unwrap_err().contains("one team"));
}

#[test]
fn settings_may_not_repeat_the_fields() {
    let c = parse_match(
        "map = \"lockout\"\ntime_limit_s = 60\n[settings]\n10 = 120\n[[bot]]\n",
        false,
    )
    .unwrap();
    assert!(resolve(c, &no_files).unwrap_err().contains("settings.10"));
}

#[test]
fn score_settings_follow_the_type() {
    for (t, id) in [
        ("slayer", 2),
        ("ctf", 1),
        ("oddball", 3),
        ("king", 4),
        ("juggernaut", 7),
        ("territories", 8),
        ("assault", 9),
    ] {
        let c = parse_match(
            &format!("map = \"x\"\ngame_type = \"{t}\"\nscore_limit = 3\n[[bot]]\n"),
            false,
        )
        .unwrap();
        assert_eq!(resolve(c, &no_files).unwrap().variant, [(id, 3)], "{t}");
    }
}

#[test]
fn game_types() {
    for (i, t) in GameType::ALL.iter().enumerate() {
        assert_eq!(t.list_index(), i as u32);
        assert_eq!(GameType::parse(t.name()), Ok(*t));
    }
    assert_eq!(GameType::parse("King of the Hill"), Ok(GameType::King));
    assert_eq!(GameType::parse("capture_the_flag"), Ok(GameType::Ctf));
    assert!(GameType::Ctf.objective());
    assert!(!GameType::Slayer.objective());
}

#[test]
fn overlay_takes_the_later() {
    let a = BotParams {
        skill: Some("easy".into()),
        reaction: Some(10),
        lead: Some(0.5),
        ..BotParams::default()
    };
    let b = BotParams {
        reaction: Some(4),
        caution: Some(0.1),
        ..BotParams::default()
    };
    let c = a.overlay(&b);
    assert_eq!(c.skill.as_deref(), Some("easy"));
    assert_eq!(c.reaction, Some(4));
    assert_eq!(c.lead, Some(0.5));
    assert_eq!(c.caution, Some(0.1));
}

#[test]
fn human_input_tuning_parses_overlays_and_rejects_out_of_range_values() {
    let base: BotParams = toml::from_str(
        "perception_ticks = 2\nmotor_ticks = 1\nbutton_sigma_ms = 4.5\n          stick_step = 0.45\nthumb_switch_ticks = 2\nphantom_ping_ms = 75.0\nphantom_jitter_ms = 3.0",
    ).unwrap();
    assert!(base.problems("base").is_empty());
    let over: BotParams = toml::from_str("motor_ticks = 3\nphantom_ping_ms = 0.0").unwrap();
    let got = base.overlay(&over);
    assert_eq!(got.motor_ticks, Some(3));
    assert_eq!(got.perception_ticks, Some(2));
    assert_eq!(got.phantom_ping_ms, Some(0.0));
    assert_eq!(got.button_sigma_ms, Some(4.5));
    assert_eq!(got.stick_step, Some(0.45));
    assert_eq!(got.thumb_switch_ticks, Some(2));
    assert_eq!(got.phantom_jitter_ms, Some(3.0));

    let bad: BotParams =
        toml::from_str("thumb_switch_ticks = 0\nstick_step = nan\nphantom_ping_ms = -1.0").unwrap();
    assert_eq!(bad.problems("bad").len(), 3);
}

#[test]
fn bot_seeds_spread() {
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..50 {
        for port in 0..4 {
            assert!(seen.insert(bot_seed(seed, port)));
        }
    }
    assert_eq!(bot_seed(3, 1), bot_seed(3, 1));
}

#[test]
fn resolved_json_is_self_contained() {
    let m = resolve(parse_match(MATCH, false).unwrap(), &aggr).unwrap();
    let v = m.resolved_json();
    assert_eq!(v["game_type"], "slayer");
    assert_eq!(
        v["bot"][0]["tuning"]["aggression"],
        serde_json::json!(0.8f32)
    );
    assert!(v["bot"][0].get("params").is_none());
    // And it reads back as a match.
    let again: MatchConfig = serde_json::from_value(v).unwrap();
    let m2 = resolve(again, &no_files).unwrap();
    assert_eq!(m2.bots, m.bots);
}

// ---- batches ----

const BATCH: &str = r#"
out = "results/x"
parallel = 3
match = "m.toml"
maps = ["lockout", "midship"]
seeds = [1, 2, 3]
[a]
name = "base"
[b]
name = "aggr"
params = "aggr.toml"
"#;

fn batch(text: &str, m: &str) -> Result<Batch, String> {
    let c: BatchConfig = toml::from_str(text).map_err(|e| e.to_string())?;
    let mc = parse_match(m, false)?;
    plan(c, mc, &aggr_any, &no_files, Path::new("/b"))
}

fn aggr_any(_: &Path) -> Result<BotParams, String> {
    Ok(BotParams {
        aggression: Some(0.9),
        ..BotParams::default()
    })
}

#[test]
fn versus_splits_bots_between_arms() {
    let b = batch(
        BATCH,
        "map = \"lockout\"\nteam_play = true\n[[bot]]\n[[bot]]\n[[bot]]\n[[bot]]\n",
    )
    .unwrap();
    assert_eq!(b.mode, Mode::Versus);
    assert_eq!(b.out, Path::new("/b/results/x"));
    assert_eq!(b.runs.len(), 6);
    assert_eq!(b.runs[0].id, "lockout-s1");
    assert_eq!(b.runs[5].id, "midship-s3");
    let r = &b.runs[4];
    assert_eq!((r.map.as_str(), r.seed), ("midship", 2));
    assert_eq!(r.config.map, "midship");
    assert_eq!(r.config.seed, 2);
    let arms: Vec<_> = r
        .config
        .bots
        .iter()
        .map(|b| b.arm.clone().unwrap())
        .collect();
    assert_eq!(arms, ["base", "aggr", "base", "aggr"]);
    let teams: Vec<_> = r.config.bots.iter().map(|b| b.team).collect();
    assert_eq!(teams, [Some(0), Some(1), Some(0), Some(1)]);
    assert_eq!(
        r.config.bots[1].tuning.as_ref().unwrap().aggression,
        Some(0.9)
    );
    assert_eq!(r.config.bots[0].tuning.as_ref().unwrap().aggression, None);
}

#[test]
fn separate_runs_each_arm() {
    // Top-level keys come before the tables in TOML.
    let text = format!("mode = \"separate\"\n{BATCH}");
    let b = batch(&text, "map = \"lockout\"\n[[bot]]\n[[bot]]\n").unwrap();
    assert_eq!(b.mode, Mode::Separate);
    assert_eq!(b.runs.len(), 12);
    assert_eq!(b.runs[0].id, "lockout-s1-base");
    assert_eq!(b.runs[1].id, "lockout-s1-aggr");
    assert!(
        b.runs[1]
            .config
            .bots
            .iter()
            .all(|x| x.arm.as_deref() == Some("aggr"))
    );
}

#[test]
fn one_arm_is_a_plain_batch() {
    let b = batch(
        "out = \"o\"\nmatch = \"m.toml\"\nseeds = [4, 5]\n[a]\n",
        "map = \"lockout\"\nseed = 1\n[[bot]]\n[[bot]]\n",
    )
    .unwrap();
    assert_eq!(b.arms.len(), 1);
    assert_eq!(b.arms[0].name, "a");
    let ids: Vec<_> = b.runs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["lockout-s4", "lockout-s5"]);
}

#[test]
fn batch_problems() {
    let e = batch(
        "out = \"o\"\nmatch = \"m.toml\"\nparallel = 0\nmode = \"both\"\nseeds = [1, 1]\n[a]\nname = \"x\"\n[b]\nname = \"x\"\n",
        "map = \"lockout\"\n[[bot]]\nparams = \"gone.toml\"\n",
    )
    .unwrap_err();
    for want in [
        "parallel = 0",
        "unknown mode",
        "both named",
        "versus needs",
        "bot 1: params",
        "share an id",
    ] {
        assert!(e.contains(want), "{want:?} missing from:\n{e}");
    }
}

#[test]
fn safe_names() {
    assert_eq!(safe("lockout"), "lockout");
    assert_eq!(safe("my map/1"), "my_map_1");
    assert_eq!(safe(""), "_");
}

/// The examples in docs/runtime/sim-examples stay valid.
#[test]
fn the_documented_examples_load() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/runtime/sim-examples");
    let m = load_match(&dir.join("lockout-2v2.toml")).unwrap();
    assert_eq!(m.bots.len(), 4);
    assert_eq!(m.bots[0].params.skill.as_deref(), Some("legendary"));
    assert_eq!(m.bots[3].params.aggression, Some(0.5));
    let b = load_batch(&dir.join("batch.toml")).unwrap();
    assert_eq!(b.runs.len(), 15);
    assert_eq!(b.arms[1].params.aggression, Some(0.95));
    let blue = &b.runs[0].config.bots[3];
    assert_eq!(blue.arm.as_deref(), Some("aggressive"));
    assert_eq!(blue.tuning.as_ref().unwrap().aggression, Some(0.95));
    for name in ["base.toml", "aggressive.toml"] {
        let p = BotParams::load(&dir.join("tuning").join(name)).unwrap();
        assert!(p.problems(name).is_empty(), "{name}");
    }
}

#[test]
fn light_host_corpus_expands_all_maps_modes_and_seeds() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/runtime/sim-examples/light-host-corpus.toml");
    let batch = load_batch(&path).unwrap();
    assert!(batch.config.audit);
    assert_eq!(batch.runs.len(), 23 * 7 * 3);
    for run in batch.runs {
        let match_ = resolve(run.config, &no_files).unwrap();
        assert!(match_.distributed());
        assert_eq!(match_.bots.len(), 4);
        match match_.config.game_type.as_str() {
            "ctf" | "assault" => assert!(match_.team_play),
            "juggernaut" => assert!(!match_.team_play),
            _ => {}
        }
    }
}

#[test]
fn the_aim_model_is_code_or_net() {
    let p: BotParams = toml::from_str("aim_model = \"net\"").unwrap();
    assert_eq!(p.aim_model.as_deref(), Some("net"));
    assert!(p.problems("x").is_empty());
    let over = BotParams {
        aim_model: Some("code".into()),
        ..BotParams::default()
    };
    assert_eq!(p.overlay(&over).aim_model.as_deref(), Some("code"));
    assert_eq!(
        over.overlay(&BotParams::default()).aim_model,
        over.aim_model
    );
    let bad: BotParams = toml::from_str("aim_model = \"mlp\"").unwrap();
    let e = bad.problems("bot 1").join("\n");
    assert!(e.contains("bot 1: aim_model = \"mlp\": code or net"), "{e}");
    // Left out, it is not written back.
    assert!(
        !toml::to_string(&BotParams::default())
            .unwrap()
            .contains("aim_model")
    );
}

const ARMS: &str = "[a]\nname = \"code\"\n[a.tuning]\naim_model = \"code\"\n[b]\nname = \"net\"\n[b.tuning]\naim_model = \"net\"\n";
const TWO_V_TWO: &str = "map = \"midship\"\nteam_play = true\n[[bot]]\nname = \"R1\"\nskill = \"heroic\"\n[[bot]]\nname = \"B1\"\n[[bot]]\nname = \"R2\"\n[[bot]]\nname = \"B2\"\n";

#[test]
fn an_arm_swap_plays_each_arm_in_every_seat() {
    let text =
        format!("out = \"o\"\nmatch = \"m.toml\"\nseeds = [3, 4]\nmode = \"arm_swap\"\n{ARMS}");
    let b = batch(&text, TWO_V_TWO).unwrap();
    assert_eq!(b.mode, Mode::ArmSwap);
    assert_eq!(Mode::parse("arm_swap").unwrap().name(), "arm_swap");
    let ids: Vec<_> = b.runs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "midship-s3",
            "midship-s3-swapped",
            "midship-s4",
            "midship-s4-swapped"
        ]
    );
    for pair in b.runs.chunks_exact(2) {
        let (first, second) = (&pair[0].config, &pair[1].config);
        assert_eq!(first.arms_swapped, Some(false));
        assert_eq!(second.arms_swapped, Some(true));
        assert_eq!((&first.map, first.seed), (&second.map, second.seed));
        let arms = |c: &MatchConfig| -> Vec<String> {
            c.bots.iter().map(|b| b.arm.clone().unwrap()).collect()
        };
        assert_eq!(arms(first), ["code", "net", "code", "net"]);
        assert_eq!(arms(second), ["net", "code", "net", "code"]);
        let (a, b) = (
            resolve(first.clone(), &no_files).unwrap(),
            resolve(second.clone(), &no_files).unwrap(),
        );
        for (x, y) in a.bots.iter().zip(&b.bots) {
            // The same seat, name, team, transport, skill and brain seed;
            // only the arm and what it tunes change.
            assert_eq!(
                (&x.name, x.seat, x.port, x.kind, x.team, x.skill, x.seed),
                (&y.name, y.seat, y.port, y.kind, y.team, y.skill, y.seed)
            );
            assert_ne!(x.arm, y.arm);
            assert_ne!(x.params.aim_model, y.params.aim_model);
            let model = if x.arm.as_deref() == Some("net") {
                "net"
            } else {
                "code"
            };
            assert_eq!(x.params.aim_model.as_deref(), Some(model));
        }
        // The legs write back and read as they were planned.
        for c in [first, second] {
            let back = parse_match(&toml::to_string(c).unwrap(), false).unwrap();
            assert_eq!(&back, c);
        }
    }
    // It needs both arms.
    let one = batch(
        "out = \"o\"\nmatch = \"m.toml\"\nmode = \"arm_swap\"\n[a]\n",
        TWO_V_TWO,
    )
    .unwrap_err();
    assert!(one.contains("arm_swap needs two arms"), "{one}");
    // The other modes leave the leg marker out.
    let plain = batch(
        &format!("out = \"o\"\nmatch = \"m.toml\"\n{ARMS}"),
        TWO_V_TWO,
    )
    .unwrap();
    assert_eq!(plain.runs[0].config.arms_swapped, None);
}

#[test]
fn skills_are_an_axis_over_every_bot() {
    let text = format!(
        "out = \"o\"\nmatch = \"m.toml\"\nmaps = [\"midship\", \"lockout\"]\nseeds = [1, 2]\nskills = [\"easy\", \"Legendary\"]\n{ARMS}"
    );
    let b = batch(&text, TWO_V_TWO).unwrap();
    let ids: Vec<_> = b.runs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "midship-easy-s1",
            "midship-easy-s2",
            "midship-legendary-s1",
            "midship-legendary-s2",
            "lockout-easy-s1",
            "lockout-easy-s2",
            "lockout-legendary-s1",
            "lockout-legendary-s2",
        ]
    );
    for r in &b.runs {
        let want = if r.id.contains("easy") {
            "easy"
        } else {
            "legendary"
        };
        let m = resolve(r.config.clone(), &no_files).unwrap();
        assert!(m.bots.iter().all(|b| b.skill.name() == want), "{}", r.id);
    }
    // Without the axis each bot keeps its own.
    let own = batch(
        &format!("out = \"o\"\nmatch = \"m.toml\"\n{ARMS}"),
        TWO_V_TWO,
    )
    .unwrap();
    let m = resolve(own.runs[0].config.clone(), &no_files).unwrap();
    let skills: Vec<_> = m.bots.iter().map(|b| b.skill.name()).collect();
    assert_eq!(skills, ["heroic", "normal", "normal", "normal"]);
    let e = batch(
        &format!("out = \"o\"\nmatch = \"m.toml\"\nskills = [\"mythic\"]\n{ARMS}"),
        TWO_V_TWO,
    )
    .unwrap_err();
    assert!(e.contains("skills:"), "{e}");
}

#[test]
fn game_types_and_skills_compose_with_arm_swaps() {
    let text = format!(
        r#"out = 'o'
match = 'm.toml'
maps = ['midship', 'lockout']
game_types = ['Capture the Flag', 'assault', 'juggernaut']
skills = ['easy', 'Heroic']
seeds = [3, 4]
mode = 'arm_swap'
{ARMS}"#
    );
    let base = r#"map = 'midship'
team_play = false
[[bot]]
team = 1
skill = 'legendary'
[[bot]]
team = 0
"#;
    let batch = batch(&text, base).unwrap();
    assert_eq!(batch.runs.len(), 2 * 3 * 2 * 2 * 2);
    let mut runs = batch.runs.into_iter();
    for map in ["midship", "lockout"] {
        for (game_type, team_play) in [("ctf", true), ("assault", true), ("juggernaut", false)] {
            for skill in ["easy", "heroic"] {
                for seed in [3, 4] {
                    for (swapped, suffix) in [(false, ""), (true, "-swapped")] {
                        let run = runs.next().unwrap();
                        assert_eq!(run.id, format!("{map}-{game_type}-{skill}-s{seed}{suffix}"));
                        let m = resolve(run.config, &no_files).unwrap();
                        assert_eq!(m.game_type.name(), game_type);
                        assert_eq!(m.team_play, team_play);
                        assert_eq!(m.config.arms_swapped, Some(swapped));
                        assert!(m.bots.iter().all(|b| b.skill.name() == skill));
                        let arms = if team_play ^ swapped {
                            ["net", "code"]
                        } else {
                            ["code", "net"]
                        };
                        assert_eq!(
                            m.bots
                                .iter()
                                .map(|b| b.arm.as_deref().unwrap())
                                .collect::<Vec<_>>(),
                            arms
                        );
                    }
                }
            }
        }
    }
}

/// The aim-model comparison in docs/runtime/sim-examples stays valid.
#[test]
fn the_aim_model_comparison_loads() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/runtime/sim-examples");
    let b = load_batch(&dir.join("aim-net-vs-code.toml")).unwrap();
    assert_eq!(b.mode, Mode::ArmSwap);
    assert_eq!(
        b.arms.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        ["code", "net"]
    );
    // Two maps, four skills, five seeds, two legs.
    assert_eq!(b.runs.len(), 80);
    assert_eq!(b.runs[0].id, "midship-easy-s1");
    assert_eq!(b.runs[79].id, "lockout-legendary-s5-swapped");
    for r in &b.runs {
        let m = resolve(r.config.clone(), &no_files).unwrap();
        assert_eq!(m.bots.len(), 4);
        assert!(!m.distributed());
        for model in ["code", "net"] {
            let on: Vec<_> = m
                .bots
                .iter()
                .filter(|b| b.params.aim_model.as_deref() == Some(model))
                .collect();
            // One team a model, and the arm named after it.
            assert_eq!(on.len(), 2, "{}", r.id);
            assert_eq!(on[0].team, on[1].team);
            assert!(on.iter().all(|b| b.arm.as_deref() == Some(model)));
        }
        // Apart from the aim model and the skill the arms are untuned.
        for bot in &m.bots {
            let plain = BotParams {
                skill: bot.params.skill.clone(),
                aim_model: bot.params.aim_model.clone(),
                ..BotParams::default()
            };
            assert_eq!(bot.params, plain);
        }
    }
}

#[test]
fn planned_runs_write_back_as_toml() {
    let b = batch(
        BATCH,
        "map = \"lockout\"\nteam_play = true\n[settings]\n83 = 1\n[[bot]]\nname = \"A\"\n[[bot]]\n",
    )
    .unwrap();
    for r in &b.runs {
        let text = toml::to_string(&r.config).unwrap();
        let back = parse_match(&text, false).unwrap();
        assert_eq!(back, r.config, "{text}");
        resolve(back, &no_files).unwrap();
    }
}

#[test]
fn swapping_seats_preserves_brains_teams_and_allocates_dense_local_ports() {
    let b = batch("mode='seat_swap'\nout='o'\nmatch='m'\nseeds=[41,42]\n[a]\n",
        "map='midship'\nteam_play=true\n[[bot]]\nkind='phantom'\nteam=1\n[[bot]]\nname='R'\nteam=0\n[[bot]]\nkind='phantom'\nteam=0\n").unwrap();
    assert_eq!(b.runs.len(), 4);
    for pair in b.runs.chunks_exact(2) {
        let leg = |swapped| {
            resolve(
                pair.iter()
                    .find(|r| r.config.seats_swapped == Some(swapped))
                    .unwrap()
                    .config
                    .clone(),
                &no_files,
            )
            .unwrap()
        };
        let (a, b) = (leg(false), leg(true));
        assert!(a.distributed() && b.distributed());
        assert_eq!(a.config.seed, b.config.seed);
        assert_eq!(a.config.map, b.config.map);
        assert_eq!(
            a.bots.iter().map(|b| b.port).collect::<Vec<_>>(),
            [None, Some(0), None]
        );
        assert_eq!(
            b.bots.iter().map(|b| b.port).collect::<Vec<_>>(),
            [Some(0), None, Some(1)]
        );
        for (a, b) in a.bots.iter().zip(&b.bots) {
            assert_eq!(
                (&a.name, a.seat, a.seed, a.team, &a.params, &a.arm),
                (&b.name, b.seat, b.seed, b.team, &b.params, &b.arm)
            );
            assert_eq!(a.kind, b.kind.swapped());
        }
    }
}

#[test]
fn midship_parity_example_disables_transport_delay_in_both_legs() {
    let plan = batch(
        include_str!("../../../../docs/runtime/sim-examples/midship-seat-swap.toml"),
        include_str!("../../../../docs/runtime/sim-examples/midship-seats.toml"),
    )
    .unwrap();
    assert_eq!(plan.runs.len(), 20);
    for run in plan.runs {
        let played = resolve(run.config, &no_files).unwrap();
        for bot in played.bots {
            assert_eq!(bot.params.phantom_ping_ms, Some(0.));
            assert_eq!(bot.params.phantom_jitter_ms, Some(0.));
        }
    }
}

#[test]
fn swapped_layout_can_be_playerless_but_still_has_at_most_four_pads() {
    let spec = "out='o'\nmatch='m'\nmode='seat_swap'\n[a]\n";
    let one = "map='midship'\n[[bot]]\n";
    let runs = batch(spec, one).unwrap().runs;
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].config.bots[0].kind, SeatKind::Local);
    assert_eq!(runs[1].config.bots[0].kind, SeatKind::Phantom);
    let five_remotes = format!("{one}{}", "[[bot]]\nkind='phantom'\n".repeat(5));
    assert!(
        batch(spec, &five_remotes)
            .unwrap_err()
            .contains("at most 4 controller ports")
    );
}

#[test]
fn bot_work_rates_overlay_per_field_and_validate_tick_limits() {
    let base: BotParams = toml::from_str("foot_plan_ticks = 6\nplayer_los_ticks = 3").unwrap();
    let overlay: BotParams = toml::from_str(
        "player_los_ticks = 1\nvehicle_lookahead_ticks = 2\nvehicle_plan_ticks = 30",
    )
    .unwrap();
    let merged = base.overlay(&overlay);
    assert_eq!(merged.foot_plan_ticks, Some(6));
    assert_eq!(merged.player_los_ticks, Some(1));
    assert_eq!(merged.vehicle_lookahead_ticks, Some(2));
    assert_eq!(merged.vehicle_plan_ticks, Some(30));
    assert!(merged.problems("bot").is_empty());

    let bad: BotParams = toml::from_str("foot_plan_ticks = 0\nvehicle_plan_ticks = 31").unwrap();
    assert_eq!(bad.problems("bot").len(), 2);
}
