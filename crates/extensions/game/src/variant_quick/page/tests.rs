use super::*;
use guest_layout::entry::{Regs, testing};
use guest_layout::testing::FakeGuest;
use variants::ext::{Block, Mode};
use variants::record::{GameType, RECORD_SIZE};

use crate::ui_kit::page::{Focus, ListEffect, Live, Page};

/// Where the setter's record is written in these tests.
const AT: u32 = 0x8300_0000;

fn superbounce() -> (Arc<Ir>, GameVariant) {
    let ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
    let record = variants::portable::record_for(&ir, &[]).unwrap();
    (Arc::new(ir), record)
}

/// A dialog on Superbounce with the sample offer, over its own queue.
fn opened() -> (Arc<Dialog>, Arc<Changes>, GameVariant) {
    let (ir, record) = superbounce();
    let changes = Arc::new(Changes::default());
    let d = Arc::new(Dialog::new(Arc::clone(&changes)));
    d.open(record.clone(), ir, Some(sample_offer()));
    (d, changes, record)
}

/// A value of row `slot` other than the one it holds.
fn another(d: &Dialog, slot: usize) -> i32 {
    let row = d.rows()[slot].clone();
    row.values
        .iter()
        .map(|(v, _)| *v)
        .find(|v| *v != row.value)
        .unwrap()
}

/// Runs the queue's drain once, the setter answering `al`: the records
/// it was handed.
fn drain(changes: &Arc<Changes>, dialog: &Arc<Dialog>, al: u32) -> Vec<Vec<u8>> {
    let (c, d) = (Arc::clone(changes), Arc::clone(dialog));
    let f = move |cx: Cx| -> EntryFuture {
        let (c, d) = (Arc::clone(&c), Arc::clone(&d));
        Box::pin(async move {
            c.drain(&cx, AT, &d).await?;
            Ok(Ret::new(0, 0))
        })
    };
    let mut g = FakeGuest::new();
    g.map(AT, 0x1000);
    let mut handed = Vec::new();
    let run = testing::run(&f, g, Regs::default(), |call, m| {
        assert_eq!(
            call.target,
            points::LOBBY_SET_VARIANT.base,
            "only the setter"
        );
        assert_eq!(call.args, [AT], "stack: the record");
        handed.push(m.bytes(AT, RECORD_SIZE).unwrap());
        Regs {
            eax: al,
            ..Default::default()
        }
    });
    assert_eq!(run.result, Ok(Ret::new(0, 0)));
    handed
}

/// The setter is the named point the dedicated host calls.
#[test]
fn the_setter_is_the_named_point() {
    assert_eq!(points::LOBBY_SET_VARIANT.base, 0x0019_a728);
    assert_eq!(
        crate::host_lobby::SET_VARIANT.base,
        points::LOBBY_SET_VARIANT.base
    );
    assert_eq!(
        points::LOBBY_SET_VARIANT.sig.arg("variant"),
        Some(points::Loc::Stack(0))
    );
}

/// A CTF script can mark a type-area setting. On index 6 only common
/// settings have a record field, and score uses the common score page.
#[test]
fn quick_marks_follow_the_records_available_fields() {
    let ir = ::rules::compile(
        "rules \"Example\" 1\ntype ctf\nuses ctf\nquick score_limit, ctf_flag_type\n",
    )
    .unwrap();
    let record = GameVariant::stock(GameType::Script).unwrap();
    let rows = crate::variant_quick::rows_with(&record, &ir, None);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "score_limit");
    assert!(matches!(rows[0].mark, Mark::Setting { setting: 2, .. }));

    let view = variants::panel::script_view(&ir, &record);
    assert_eq!(view.marks.len(), 1);
    assert_eq!(view.marked_settings, [SettingId(2)]);
}

/// A row's change is the lobby's record with that one setting changed:
/// it is shown at once, waits for the UI update, and the drain writes it
/// and calls the lobby variant setter with it on the stack. The setter
/// taking it settles the change.
#[test]
fn a_change_writes_one_setting_and_calls_the_setter() {
    let (d, changes, record) = opened();
    let to = another(&d, 0);
    let Mark::Setting { setting, .. } = d.rows()[0].mark else {
        panic!("Superbounce's first mark is a setting");
    };
    let line = d.choose(0, to);
    assert!(line.starts_with("variant quick: Respawn Time "), "{line}");
    assert_eq!(d.rows()[0].value, to, "shown at once");
    assert!(changes.waiting());
    let handed = drain(&changes, &d, 1);
    let mut expect = record.clone();
    expect.set(SettingId(setting), to).unwrap();
    assert_eq!(
        handed,
        [expect.bytes().to_vec()],
        "the record, one setting changed"
    );
    assert_eq!(handed[0].len(), RECORD_SIZE);
    assert!(!changes.waiting());
    assert_eq!(changes.outcome(1), Some(Ok(())));
    assert_eq!(changes.outcome(1), None, "collected once");
    assert_eq!(d.rows()[0].value, to);
    // Nothing waits: the drain calls nothing.
    assert!(drain(&changes, &d, 1).is_empty());
}

/// An option's change writes its bits in the block in effect and keeps
/// the rest of the block: it decodes (its CRC made again), the other
/// option's bits and the script's prefix as they were.
#[test]
fn an_option_change_keeps_the_blocks_crc() {
    let (d, changes, record) = opened();
    let rows = d.rows();
    let slot = rows
        .iter()
        .position(|r| matches!(r.mark, Mark::Option { .. }))
        .unwrap();
    let to = another(&d, slot);
    let before = Ext::in_effect(record.ext_block());
    d.choose(slot, to);
    let handed = drain(&changes, &d, 1);
    let written = GameVariant::from_bytes(&handed[0]).unwrap();
    let Block::Ext(after) = Ext::decode(written.ext_block()) else {
        panic!("the block decodes, its CRC good");
    };
    assert_eq!(after.script.mode, Mode::Rules);
    assert_eq!(after.script.prefix, before.script.prefix);
    assert_ne!(after.script.options, before.script.options);
    assert_eq!(
        Ext {
            script: before.script,
            ..after
        },
        before,
        "the rest of the block kept"
    );
    // The rows read from the written record: only that option moved.
    let (ir, _) = superbounce();
    let now = crate::variant_quick::rows_with(&written, &ir, Some(&sample_offer()));
    for (i, (a, b)) in rows.iter().zip(&now).enumerate() {
        if i == slot {
            assert_eq!(b.value, to);
        } else {
            assert_eq!(a.value, b.value, "row {i}");
        }
    }
    // Every bit outside the block is the record's.
    assert_eq!(
        handed[0][..variants::ext::AT],
        record.bytes()[..variants::ext::AT]
    );
}

/// A change to a record the game would not keep is refused by
/// `record::refused`: said in the row's help, nothing queued, the row
/// as it was. A record the setter refuses (AL 0) shows the lobby's
/// record again and says so; its outcome is the refusal.
#[test]
fn a_refused_record_changes_nothing() {
    use variants::record::{CYCLING, RESPAWN, RESPAWN_MODIFIER};
    let (ir, mut record) = superbounce();
    record.set(RESPAWN_MODIFIER, CYCLING).unwrap();
    record.set(RESPAWN, 5).unwrap();
    let changes = Arc::new(Changes::default());
    let d = Arc::new(Dialog::new(Arc::clone(&changes)));
    d.open(record.clone(), Arc::clone(&ir), Some(sample_offer()));
    let line = d.choose(0, 0);
    assert!(line.contains("not changed"), "{line}");
    assert!(!changes.waiting(), "nothing queued");
    assert_eq!(d.rows()[0].value, 5);
    assert_eq!(d.record().unwrap().bytes(), record.bytes());
    let help = d.fill(0).unwrap().help.unwrap();
    assert!(help.contains("divides by zero"), "{help}");
    // A value the row does not offer is refused the same way.
    d.choose(0, 7);
    assert!(!changes.waiting());
    assert_eq!(
        d.fill(0).unwrap().help.unwrap(),
        "7 is not a value of Respawn Time"
    );

    // The setter refuses.
    let (d, changes, record) = opened();
    d.choose(0, another(&d, 0));
    assert_eq!(drain(&changes, &d, 0).len(), 1);
    assert_eq!(changes.outcome(1), Some(Err(REFUSED.to_string())));
    assert_eq!(
        d.record().unwrap().bytes(),
        record.bytes(),
        "the lobby's record again"
    );
    assert_eq!(d.fill(0).unwrap().help.as_deref(), Some(REFUSED));
    // The next change clears it.
    d.choose(0, another(&d, 0));
    assert_ne!(d.fill(0).unwrap().help.as_deref(), Some(REFUSED));
}

/// Game Setup's Quick Options opens this page only for a variant whose
/// script this machine has and that marks rows; a variant with no
/// script, or with the page off, keeps retail's.
#[test]
fn quick_options_opens_ours_only_for_a_script_variant() {
    let (_, record) = superbounce();
    let ir = crate::variant_quick::script_for(&record);
    assert_eq!(entry(ir.as_deref(), true), Entry::Ours);
    assert_eq!(entry(ir.as_deref(), false), Entry::Retail, "the page off");
    let stock = GameVariant::stock(GameType::Slayer).unwrap();
    assert!(crate::variant_quick::script_for(&stock).is_none());
    assert_eq!(
        entry(crate::variant_quick::script_for(&stock).as_deref(), true),
        Entry::Retail
    );
    assert_eq!(entry(None, true), Entry::Retail);
    // Opened on the record, the page shows its name and its marks.
    let d = Dialog::new(Arc::default());
    d.open(record.clone(), ir.unwrap(), Some(sample_offer()));
    assert_eq!(d.name(), "Superbounce");
    assert_eq!(d.rows().len(), 5);
    assert_eq!(d.fill(5), None);
}

/// A script with no `quick` line greys the row: its press does nothing,
/// and Change Rules edits everything.
#[test]
fn quick_options_is_greyed_for_a_script_with_no_marks() {
    let ir = ::rules::compile(::rules::builtin::SLAYER).unwrap();
    assert!(ir.quick.is_empty());
    let record = variants::portable::record_for(&ir, &[]).unwrap();
    let found = crate::variant_quick::script_for(&record);
    assert_eq!(found.as_ref().map(|i| i.id()), Some(ir.id()));
    assert_eq!(entry(found.as_deref(), true), Entry::Greyed);
    assert_eq!(entry(found.as_deref(), false), Entry::Retail);
}

/// The page in every state the preview draws: Superbounce's five rows
/// (a setting focused, an option, the last), three marks (the last two
/// slots not drawn) and a refusal said in the row's help; each with the
/// variant's name in the header, no arrows, the fallback checks clean,
/// and A opening the picker on a row's values.
#[test]
fn the_page_draws_in_every_state() {
    let live = Samples::new();
    let specs = live.specs();
    let names: Vec<&str> = specs.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["focus-0", "focus-3", "focus-4", "three-rows", "refused", "siege", "siege-rounds"]
    );
    for (name, s) in &specs {
        let siege = name.starts_with("siege");
        let rows = match name.as_str() {
            "three-rows" => 3,
            _ if siege => 7,
            _ => 5,
        };
        assert_eq!(s.rows.len(), rows, "{name}");
        assert_eq!(s.title, "QUICK OPTIONS", "{name}");
        let variant = if siege { "Siege" } else { "Superbounce" };
        assert_eq!(s.texts[0].text, variant, "{name}: the variant's name");
        assert!(
            s.rows
                .iter()
                .all(|r| r.description.as_deref().is_some_and(|h| !h.is_empty())),
            "{name}"
        );
        let findings: Vec<_> = crate::ui_kit::spec::check_fallback(s)
            .into_iter()
            .filter(|f| {
                !matches!(
                    f,
                    crate::ui_kit::spec::Finding::Wide { .. }
                        | crate::ui_kit::spec::Finding::Description { .. }
                )
            })
            .collect();
        assert!(findings.is_empty(), "{name}: {findings:#?}");
        let (v, _) = crate::ui_kit::preview::From::Synthetic
            .draw(s, name)
            .unwrap();
        for i in 0..rows {
            assert!(
                v.widget(&format!("row.{i}.label")).is_some(),
                "{name}: row {i}"
            );
        }
        assert!(
            v.widget(&format!("row.{rows}.label")).is_none(),
            "{name}: no row {rows}"
        );
    }
    let lines = |n: &str| -> Vec<String> {
        let s = &specs.iter().find(|(m, _)| m == n).unwrap().1;
        s.rows.iter().map(|r| r.label.clone()).collect()
    };
    assert_eq!(
        lines("focus-0"),
        [
            "Respawn Time",
            "Starting Weapon",
            "Shields",
            "Bounce points",
            "Suicide penalty"
        ]
    );
    assert_eq!(
        lines("three-rows"),
        ["Respawn Time", "Starting Weapon", "Shields"]
    );
    // Siege's page is every option of its: the marks, then the rest.
    assert_eq!(
        lines("siege"),
        [
            "Respawn Time",
            "Respawn wave (seconds)",
            "Phase time (seconds)",
            "Capture time (seconds)",
            "Sites",
            "Overtime (seconds)",
            "Rounds"
        ]
    );
    let focused = |n: &str| {
        let (_, s) = specs.iter().find(|(m, _)| m == n).unwrap();
        s.description_text().unwrap()
    };
    assert_eq!(
        focused("focus-0"),
        "Sets the respawn time for this lobby's games."
    );
    assert_eq!(
        focused("focus-3"),
        "Points for each superbounce, times its place in the chain"
    );
    assert_eq!(focused("refused"), "7 is not a value of Respawn Time");
    assert_eq!(focused("siege-rounds"), "How many rounds; the sides swap at each");
    // A opens the picker on the row's values, the held one focused; left
    // and right step nothing.
    let page = sample_page();
    let mut f = Focus(0);
    assert_eq!(
        page.step(&mut f, crate::ui_kit::input::Input::A),
        ListEffect::Open(crate::ui_kit::picker::NAME)
    );
    let pick = crate::ui_kit::picker::current().unwrap();
    assert_eq!(pick.title, "RESPAWN TIME");
    assert_eq!(
        pick.values,
        ["0 seconds", "3 seconds", "5 seconds", "10 seconds"]
    );
    assert_eq!(
        page.step(&mut f, crate::ui_kit::input::Input::Right),
        ListEffect::None
    );
}

/// The row's press and the setter's driver are on the plan, after the
/// Halo 1 Options row's.
#[test]
fn the_hooks_are_on_the_plan() {
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    r.add(&h1row::SetupRow {
        data_root: std::env::temp_dir(),
        page: h1row::test_page(),
    })
    .unwrap();
    r.add(&SetupRow {
        dialog: Arc::new(Dialog::new(Arc::default())),
        changes: Arc::default(),
    })
    .unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    let line = text
        .lines()
        .find(|l| l.contains("SQUAD_SETTINGS_ROW_CHOSEN"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        line.find("h1-options-row:").unwrap() < line.find("variant-quick-row:").unwrap(),
        "{line}"
    );
    assert!(
        text.contains("UI_UPDATE: before [variant-quick-row:"),
        "{text}"
    );
}

/// Siege's options page, row by row: every option opens a picker by A on
/// a short list with the held value focused (a wide range a ladder), a
/// pick writes that option's bits alone and the row shows it at once, and
/// the two options past the marks (the overtime and the rounds) change as
/// the marked ones do.
#[test]
fn sieges_page_changes_every_option_through_a_picker() {
    let ir = Arc::new(::rules::compile(::rules::builtin::SIEGE).unwrap());
    let record = variants::portable::record_for(&ir, &[]).unwrap();
    let changes = Arc::new(Changes::default());
    let d = Arc::new(Dialog::new(Arc::clone(&changes)));
    d.open(record.clone(), Arc::clone(&ir), Some(sample_offer()));
    let rows = d.rows();
    assert_eq!(rows.len(), 7);
    assert_eq!(d.name(), "Siege");
    // Game Setup's Quick Options row opens this page for the listed Siege
    // entry, a record on Slayer's index.
    let listed = crate::community_variants::listed_named("Siege").unwrap();
    assert_eq!(listed.record.game_type(), GameType::Slayer);
    assert_eq!(
        entry(crate::variant_quick::script_for(&listed.record).as_deref(), true),
        Entry::Ours
    );
    assert!(d.fill(7).is_none(), "no eighth row");
    for (slot, row) in rows.iter().enumerate() {
        let filled = d.fill(slot).unwrap();
        assert_eq!(filled.label, row.label);
        assert!(filled.help.is_some_and(|h| !h.is_empty()), "{}", row.label);
        let pick = d.offer(slot).unwrap_or_else(|e| panic!("{}: {e}", row.label));
        assert!((2..=crate::variant_quick::PICKER_VALUES).contains(&pick.values.len()), "{}: {:?}", row.label, pick.values);
        // An option's held value is always on its list; a setting's is
        // when the editor offered it.
        if matches!(row.mark, Mark::Option { .. }) {
            assert_eq!(pick.values[pick.current], row.text, "{}: the held value is focused", row.label);
        }
    }
    // The phase time's picker is a minute apart; the third rung is 240 s.
    let phase = rows.iter().position(|r| r.name == "phase_time").unwrap();
    let pick = d.offer(phase).unwrap();
    assert_eq!(pick.values, ["60", "120", "180", "240", "300", "360", "420", "480", "540", "600"]);
    (pick.set)(3);
    assert_eq!(d.rows()[phase].value, 240);
    // The rounds and the overtime, past the marks.
    for (name, to) in [("round_count", 4), ("overtime_time", 45)] {
        let slot = rows.iter().position(|r| r.name == name).unwrap();
        assert!(slot >= ROWS, "{name} is past the marks");
        let line = d.choose(slot, to);
        assert!(line.contains("->"), "{line}");
        assert_eq!(d.rows()[slot].value, to);
    }
    // Each change was handed to the setter in turn; the last record holds
    // all three, and every other option is at its default.
    let handed = drain(&changes, &d, 1);
    assert_eq!(handed.len(), 3);
    let written = GameVariant::from_bytes(&handed[2]).unwrap();
    let bits = Ext::in_effect(written.ext_block()).script.options;
    for (i, o) in ir.options.iter().enumerate() {
        let name = ir.option_name(i);
        let want = match name {
            "phase_time" => 240,
            "round_count" => 4,
            "overtime_time" => 45,
            _ => o.default,
        };
        assert_eq!(ir.option_value(bits, name), Some(want), "{name}");
    }
    // The record is still Siege's lobby record: Slayer's index, teams on.
    assert_eq!(written.game_type(), GameType::Slayer);
    assert!(written.team_play());
    // A value outside an option's range is refused in the row's help.
    let rounds = rows.iter().position(|r| r.name == "round_count").unwrap();
    let line = d.choose(rounds, 9);
    assert!(line.contains("not changed"), "{line}");
    assert_eq!(d.fill(rounds).unwrap().help.unwrap(), "9 is not a value of Rounds");
}
