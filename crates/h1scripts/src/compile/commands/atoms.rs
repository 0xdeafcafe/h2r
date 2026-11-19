//! Halo 1's dispatcher is at 0x16b2xx. Operands belong to the selected
//! case only. In particular a missing optional point does not mean point 0.
use super::*;
use h1ai::h1::{Atom, CommandList};

fn point(list: &CommandList, set: usize, n: i16) -> Option<Expr> {
    list.points.get(usize::try_from(n).ok()?)?;
    Some(lit(
        POINT,
        ((set as u32) << 16) | n as u32,
        format!("{}/p{n}", list.name),
    ))
}

fn pause(seconds: Expr) -> Expr {
    call("cs_pause", [seconds])
}

fn relative(mode: i16) -> Expr {
    // 0x277120 quantizes the third argument into the four direction codes.
    real(match mode {
        1 => 90.0,
        2 => -90.0,
        3 => 180.0,
        _ => 0.0,
    })
}

/// The two spare operands of the native direction command hold the H1
/// argument and its facing class. The engine handles the reserved bearings
/// only inside our generated command scripts (boot::h1_command_atoms).
fn native(kind: u16, arg: u16, amount: f32, mode: i16) -> Expr {
    let bearing = -((u32::from(kind + 1) * 65536 + u32::from(arg)) as f32);
    call(
        "cs_move_in_direction",
        [real(bearing), real(amount), relative(mode)],
    )
}

fn look(which: u16, modifier: i16, target: Option<Expr>, seconds: Expr) -> Expr {
    let (look, aim, face) = match which {
        24 => ("cs_look_player", "cs_aim_player", "cs_face_player"),
        25 => ("cs_look_object", "cs_aim_object", "cs_face_object"),
        _ => ("cs_look", "cs_aim", "cs_face"),
    };
    let turn = |action: &'static str, enable: bool| match &target {
        Some(_) if !enable => call(
            action,
            [
                boolean(false),
                if which == 25 {
                    lit(OBJECT, NONE, "none")
                } else {
                    lit(POINT, NONE, "none")
                },
            ],
        ),
        Some(t) => call(action, [boolean(enable), t.clone()]),
        None => call(action, [boolean(enable)]),
    };
    // H1 0x16b68f selects gaze strengths 5, 2, 8 and 7 for
    // modifiers 1..4. Strength 2 only turns the head. At 7 the
    // actor tracks the point with its body and weapon (0x15d157).
    let strong = |enable| begin(vec![turn(face, enable), turn(aim, enable)]);
    let (start, stop) = match modifier {
        1 => (turn(aim, true), turn(aim, false)),
        2 => (turn(look, true), turn(look, false)),
        3 | 4 => (strong(true), strong(false)),
        _ => {
            // The H1 default uses its lighter look while fighting. Choose
            // once and clear both controls if combat changes during the wait.
            let fighting = cmp(
                ">=",
                SHORT,
                call("ai_combat_status", [current()]),
                var("ai_combat_status_active"),
            );
            (
                call("if", [fighting, turn(look, true), strong(true)]),
                begin(vec![turn(look, false), strong(false)]),
            )
        }
    };
    begin(vec![start, pause(seconds), stop])
}

fn sound_for_slot<'a>(
    slots: &'a [Option<String>],
    fallback: &[i16],
    mut index: i16,
) -> Option<&'a str> {
    for _ in 0..slots.len() {
        let i = usize::try_from(index).ok()?;
        if let Some(name) = slots.get(i)?.as_deref() {
            return Some(name);
        }
        index = *fallback.get(i)?;
    }
    None
}

fn vocalize(t: &Tables, h1: &Catalog, slot: i16) -> Expr {
    let voice = || var("h1_cmd_voice_tag");
    let mut lines = Vec::new();
    for (name, slots) in &t.commands.voices {
        if let Some(sound) = sound_for_slot(slots, &h1.dialogue_fallback, slot) {
            lines.push(call(
                "if",
                [
                    cmp(
                        "=",
                        h2t::LONG,
                        voice(),
                        Symbol(h2t::LONG, name.clone(), "command_voice_tag".into()),
                    ),
                    call(
                        "sound_impulse_start",
                        [
                            Symbol(32, sound.into(), "command_voice_sound".into()),
                            unit(),
                            real(1.0),
                        ],
                    ),
                ],
            ));
        }
    }
    if lines.is_empty() {
        return lit(VOID, 0, "");
    }
    begin(vec![
        set("h1_cmd_voice_unit", unit()),
        call(
            "if",
            [
                call(
                    "not",
                    [cmp("=", h2t::LONG, voice(), lit(h2t::LONG, NONE, "-1"))],
                ),
                begin(lines),
            ],
        ),
    ])
}

fn atom(
    t: &Tables,
    h1: &Catalog,
    idx: usize,
    l: &CommandList,
    a: &Atom,
    radius: &mut Option<f32>,
    dynamic_radius: bool,
    track: bool,
) -> Result<Option<Expr>> {
    let [p, q] = a.points.map(|v| point(l, idx, v));
    let [x, y] = a.parameters;
    let m = a.modifier;
    let go = |p: Expr| {
        if dynamic_radius {
            call(
                "if",
                [
                    cmp(">", REAL, control::radius(), real(0.0)),
                    call("cs_go_to", [p.clone(), var("h1_cmd_radius_value")]),
                    call("cs_go_to", [p]),
                ],
            )
        } else {
            match *radius {
                Some(r) => call("cs_go_to", [p, real(r)]),
                None => call("cs_go_to", [p]),
            }
        }
    };
    let e = match a.kind {
        0 => pause(real(x)),
        1 => return Ok(p.map(go)),
        2 => {
            return Ok(p.map(|p| match q {
                None => go(p),
                Some(q) if !dynamic_radius && radius.is_none() => call("cs_go_to_and_face", [p, q]),
                Some(q) => begin(vec![
                    call("cs_face", [boolean(true), q.clone()]),
                    go(p),
                    call("cs_face", [boolean(false), q]),
                ]),
            }));
        }
        3 => match p {
            Some(_) => native(3, a.points[0] as u16, x, m),
            None if (0.0..=360.0).contains(&y) => {
                call("cs_move_in_direction", [real(y), real(x), relative(m)])
            }
            None => return Ok(None),
        },
        4 | 24 | 25 => {
            if x <= 0.0 {
                return Ok(None);
            }
            let target = match a.kind {
                24 => None,
                25 => {
                    let Some(n) = t.commands.objects.get(a.object_name as usize) else {
                        return Ok(None);
                    };
                    Some(Symbol(OBJECT_NAME, n.clone(), "object_name".into()))
                }
                _ => {
                    if p.is_none() {
                        return Ok(None);
                    }
                    p
                }
            };
            look(a.kind, m, target, real(x))
        }
        5 if (0..4).contains(&m) => call("cs_movement_mode", [short(m)]),
        5 => return Ok(None),
        6 => call("cs_crouch", [boolean(m == 1)]),
        7 => {
            return Ok(p.map(|p| {
                let mut burst = vec![
                    call("cs_aim", [boolean(true), p.clone()]),
                    native(7, a.points[0] as u16, 0.0, 0),
                    call("cs_shoot_point", [boolean(true), p.clone()]),
                    pause(real(x)),
                    call("cs_shoot_point", [boolean(false), p.clone()]),
                    call("cs_aim", [boolean(false), p]),
                ];
                // H1 uses +4 as the requested firing burst (0x1623ba) and
                // waits at least two seconds after acquiring its point (0x16af80).
                if !t.commands.timing.is_empty() {
                    burst.extend([
                        set("h1_cmd_timing_unit", unit()),
                        pause(call(
                            "max",
                            [real(0.0), call("-", [var("h1_cmd_recovery"), real(x)])],
                        )),
                    ]);
                } else if x < 2.0 {
                    burst.push(pause(real(2.0 - x)));
                }
                begin(burst)
            }));
        }
        8 => return Ok(p.map(|p| call("cs_grenade", [p, short(m)]))),
        9 => native(9, m as u16, x, 0),
        10 => call("cs_jump", [real(0.0), real(0.0)]),
        11 => call("cs_jump_to_point", [real(x), real(y)]),
        12 => {
            let Some(n) = t.commands.scripts.get(a.script as usize) else {
                return Ok(None);
            };
            let Some(id) = t
                .scripts
                .iter()
                .position(|s| s.name.eq_ignore_ascii_case(n))
            else {
                return Ok(None);
            };
            call("wake", [lit(h2t::SCRIPT, 0xffff0000 | id as u32, n)])
        }
        13 => {
            let Some(anim) = t.commands.animations.get(a.animation as usize) else {
                return Ok(None);
            };
            let (graph, name) = if let Some(g) = &anim.graph {
                (
                    Symbol(36, g.clone(), crate::commands::ANIMATION_GRAPH.into()),
                    Symbol(11, format!("{g}|{}", anim.name), ANIMATION_NAME.into()),
                )
            } else {
                let mut name = Symbol(
                    11,
                    h1models::names::map_name(&anim.name).full(),
                    "string".into(),
                );
                for (path, converted) in anim.unit_names.iter().rev() {
                    let matches = cmp(
                        "=",
                        36,
                        var("h1_cmd_unit_graph"),
                        Symbol(36, path.clone(), "command_unit_graph_candidate".into()),
                    );
                    name = call(
                        "if",
                        [
                            matches,
                            Symbol(11, converted.clone(), "string".into()),
                            name,
                        ],
                    );
                }
                (var("h1_cmd_unit_graph"), name)
            };
            let play = if m == 2 || m == 5 {
                let play = call(
                    "custom_animation_loop",
                    [unit(), graph, name, boolean(m == 2)],
                );
                begin(vec![
                    play,
                    call(
                        "sleep_until",
                        [
                            cmp(
                                "<=",
                                SHORT,
                                call("unit_get_custom_animation_time", [unit()]),
                                short(0),
                            ),
                            short(1),
                        ],
                    ),
                ])
            } else {
                call(
                    "cs_custom_animation",
                    [graph, name, real(1.0), boolean(m < 3)],
                )
            };
            if anim.graph.is_none() {
                begin(vec![set("h1_cmd_graph_unit", unit()), play])
            } else {
                play
            }
        }
        14 => {
            let Some(n) = t.commands.recordings.get(a.recording as usize) else {
                return Ok(None);
            };
            let rec = Symbol(17, n.clone(), "cutscene_recording".into());
            begin(vec![
                call("recording_play", [unit(), rec]),
                call("sleep", [call("recording_time", [unit()])]),
            ])
        }
        15 => native(15, m as u16, 0.0, 0),
        16 => vocalize(t, h1, m),
        17 => call("cs_enable_targeting", [boolean(m == 0)]),
        18 => call("cs_abort_on_alert", [boolean(m == 0)]),
        19 => {
            let cond = match m {
                0 | 1 => cmp(
                    ">=",
                    SHORT,
                    call("ai_combat_status", [current()]),
                    var(if m == 0 {
                        "ai_combat_status_alert"
                    } else {
                        "ai_combat_status_visible"
                    }),
                ),
                _ if track => control::stimulus(),
                _ => boolean(false),
            };
            call("sleep_until", [cond, short(1)])
        }
        21 => call("cs_die", [short(m)]),
        22 => native(22, 0, x, m),
        23 => {
            if p.is_none() || q.is_none() {
                return Ok(None);
            }
            let [lo, hi] = a.points;
            if hi < lo {
                return Err(invalid(format!("{}: reversed random look points", l.name)));
            }
            // One random integer selects the entire inclusive range. The
            // selected point stays in the blocking look and the reset.
            let upper = hi
                .checked_add(1)
                .ok_or_else(|| invalid("random point range exceeds short"))?;
            let pick = set(
                "h1_cmd_random_point",
                call("random_range", [short(lo), short(upper)]),
            );
            let duration = if x == 0.0 && y == 0.0 {
                if t.commands.timing.is_empty() {
                    return Err(invalid(format!("{}: no actor idle-look intervals", l.name)));
                }
                begin(vec![
                    set("h1_cmd_timing_unit", unit()),
                    call(
                        "real_random_range",
                        [var("h1_cmd_idle_min"), var("h1_cmd_idle_max")],
                    ),
                ])
            } else {
                call("real_random_range", [real(x), real(y)])
            };
            let mut choice = q.expect("checked point");
            for n in (lo..hi).rev() {
                choice = call(
                    "if",
                    [
                        cmp("=", SHORT, var("h1_cmd_random_point"), short(n)),
                        point(l, idx, n).expect("bounded point"),
                        choice,
                    ],
                );
            }
            begin(vec![
                pick,
                set("h1_cmd_look_point", choice),
                look(4, m, Some(var("h1_cmd_look_point")), duration),
            ])
        }
        26 => {
            if x > 0.0 {
                if dynamic_radius {
                    return Ok(Some(control::set_radius(x)));
                }
                *radius = Some(x);
            }
            return Ok(None);
        }
        27 => {
            return Ok(p.map(|p| {
                call(
                    "cs_teleport",
                    [p, q.unwrap_or_else(|| lit(POINT, NONE, "none"))],
                )
            }));
        }
        k => return Err(invalid(format!("{}: unknown command atom {k}", l.name))),
    };
    Ok(Some(e))
}

fn edge(list: &CommandList, i: usize) -> Option<usize> {
    let a = &list.atoms[i];
    let target = usize::try_from(a.command).ok()?;
    (a.kind == 20 && target < list.atoms.len() && target != i).then_some(target)
}

pub(super) fn needs_pc(list: &CommandList) -> bool {
    let mut spans = Vec::new();
    for i in 0..list.atoms.len() {
        if let Some(to) = edge(list, i) {
            if to > i
                || list.atoms[to..i].iter().any(|a| a.kind == 26)
                || spans.iter().any(|&(start, end)| start < to && to <= end)
            {
                return true;
            }
            spans.push((to, i));
        }
    }
    false
}

// A forward or crossing edge needs an explicit program counter. It lives
// on the current actor. The branch is selected before a blocking atom, so
// another actor can execute this same script while the first sleeps.
fn branches(t: &Tables, h1: &Catalog, idx: usize, l: &CommandList, track: bool) -> Result<Expr> {
    if l.atoms.len() > u8::MAX as usize {
        return Err(invalid("command list exceeds H1's byte-sized cursor"));
    }
    let mut boundaries = std::collections::BTreeSet::from([0, l.atoms.len()]);
    for i in 0..l.atoms.len() {
        if let Some(to) = edge(l, i) {
            boundaries.extend([to, i + 1]);
        }
    }
    let labels: Vec<_> = boundaries.into_iter().collect();
    let mut blocks = Vec::new();
    let mut radius = None;
    for pair in labels.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let mut seq = Vec::new();
        for i in start..end {
            let a = &l.atoms[i];
            if let Some(to) = edge(l, i) {
                seq.push(if a.modifier == 1 && track {
                    call(
                        "if",
                        [control::stimulus(), control::go(end), control::go(to)],
                    )
                } else {
                    control::go(to)
                });
            } else if a.kind != 20
                && let Some(e) = atom(t, h1, idx, l, a, &mut radius, true, track)?
            {
                seq.push(e);
            }
        }
        if edge(l, end - 1).is_none() {
            seq.push(control::go(end));
        }
        seq.push(boolean(false));
        blocks.push((start, begin(seq)));
    }
    fn dispatch(blocks: &[(usize, Expr)]) -> Expr {
        match blocks {
            [] => boolean(true),
            [(_, e)] => e.clone(),
            _ => {
                let mid = blocks.len() / 2;
                call(
                    "if",
                    [
                        cmp(
                            "<",
                            SHORT,
                            var("h1_cmd_pc_value"),
                            short(blocks[mid].0 as i16),
                        ),
                        dispatch(&blocks[..mid]),
                        dispatch(&blocks[mid..]),
                    ],
                )
            }
        }
    }
    let step = begin(vec![
        set("h1_cmd_pc_value", control::pc()),
        call(
            "if",
            [
                cmp(
                    ">=",
                    SHORT,
                    var("h1_cmd_pc_value"),
                    short(l.atoms.len() as i16),
                ),
                boolean(true),
                dispatch(&blocks),
            ],
        ),
    ]);
    Ok(begin(vec![
        control::go(0),
        control::set_radius(0.0),
        call("sleep_until", [step, short(1)]),
    ]))
}

pub(super) fn body(
    t: &Tables,
    h1: &Catalog,
    idx: usize,
    l: &CommandList,
    track: bool,
) -> Result<Expr> {
    let mut body = Vec::<(usize, Expr)>::new();
    let mut radius = None;
    if needs_pc(l) {
        body.push((0, branches(t, h1, idx, l, track)?));
    } else {
        for (i, a) in l.atoms.iter().enumerate() {
            if a.kind == 20 {
                if a.command < 0 || a.command as usize >= l.atoms.len() {
                    continue;
                }
                let to = a.command as usize;
                if to == i {
                    continue;
                }
                debug_assert!(to < i);
                let head = body.partition_point(|(at, _)| *at < to);
                let mut loop_body: Vec<_> = body.drain(head..).map(|(_, e)| e).collect();
                loop_body.push(if a.modifier == 1 && track {
                    control::stimulus()
                } else {
                    boolean(false)
                });
                body.push((to, call("sleep_until", [begin(loop_body), short(1)])));
            } else if let Some(e) = atom(t, h1, idx, l, a, &mut radius, false, track)? {
                body.push((i, e));
            }
        }
    }
    let mut out = vec![
        call("cs_abort_on_alert", [boolean(l.flags & 1 != 0)]),
        call("cs_enable_targeting", [boolean(l.flags & 2 != 0)]),
        call("cs_enable_looking", [boolean(l.flags & 4 == 0)]),
        call("cs_enable_dialogue", [boolean(l.flags & 8 == 0)]),
    ];
    let protect = l.flags & 16 != 0;
    if track || protect {
        out.push(control::start(protect));
    }
    out.extend(body.into_iter().map(|(_, e)| e));
    if track || protect {
        out.push(control::finish());
    }
    let body = begin(out);
    Ok(if l.bsp >= 0 {
        call(
            "if",
            [
                cmp("=", SHORT, call("structure_bsp_index", []), short(l.bsp)),
                body,
            ],
        )
    } else {
        body
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_uses_the_authored_slot_then_the_source_fallback_chain() {
        let slots = [
            None,
            None,
            Some("grunt/line".to_string()),
            Some("grunt/other".to_string()),
        ];
        assert_eq!(
            sound_for_slot(&slots, &[1, 2, -1, 2], 0),
            Some("grunt/line")
        );
        assert_eq!(
            sound_for_slot(&slots, &[1, 2, -1, 2], 3),
            Some("grunt/other")
        );
        for index in [-1, 0, 1, 4] {
            assert_eq!(sound_for_slot(&slots, &[1, 0, -1, -1], index), None);
        }
    }

    #[test]
    fn selects_an_actor_cursor_for_crossing_edges_or_a_radius_inside_a_loop() {
        let atom = |kind, command| Atom {
            kind,
            command,
            modifier: 0,
            parameters: [1.0, 0.0],
            points: [-1, -1],
            animation: -1,
            script: -1,
            recording: -1,
            object_name: -1,
        };
        let mut list = CommandList {
            name: "test".into(),
            flags: 0,
            manual_bsp: -1,
            bsp: -1,
            atoms: vec![atom(0, -1), atom(0, -1), atom(20, 0), atom(20, 1)],
            points: vec![],
        };
        assert!(needs_pc(&list));
        list.atoms[3].command = 0;
        assert!(!needs_pc(&list));
        list.atoms[1].kind = 26;
        assert!(needs_pc(&list));
        list.atoms[0] = atom(20, 2);
        assert!(needs_pc(&list));
        list.atoms = vec![atom(20, 0)];
        assert!(!needs_pc(&list));
    }
}
