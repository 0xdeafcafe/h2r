//! The Scripts tab of the Map workspace (docs/editor/rich-editors.md,
//! "Scripts"): the globals and scripts with a search, the builtins and
//! where their names come from, and in the Inspector the selected
//! definition's source with every name a link. Read only; each panel is
//! one scrolling body. "Open in VS Code" and "Open in Zed" write the
//! scripts as a working copy and open it there (`app::external`); what a
//! save there changed shows under the buttons. **Edit** puts the shown
//! definition's source in a text box and **Compile** writes it into the
//! map's tables (`app::script_edit`); **Compile working copy** does the
//! same for what a save in VS Code or Zed changed.

use dear_imgui_rs::{
    InputTextMultilineFlags, ListClipper, MouseButton, MouseCursor, TreeNodeFlags, Ui,
};
use hs::{Item, NameFrom};
use scenario_edit::layout;

use crate::app::script_edit::{Outcome, Refusal};
use crate::app::scripts::{LinkKind, Loaded, Target};
use crate::app::{Action, App, ScriptAction};
use crate::theme::{self, Fonts, icon};

/// The tab's own state: the searches.
#[derive(Default)]
pub struct ScriptsUi {
    pub filter: String,
    pub builtins: String,
    /// The recording shown under Recordings (an index into the block).
    pub recording: Option<usize>,
    /// The definition being edited and its text.
    pub edit: Option<Edit>,
    /// The expression box (**Evaluate in game**).
    pub expression: String,
}

/// A definition's source in the text box.
pub struct Edit {
    pub item: Item,
    pub text: String,
    /// `ScriptsTool::compiles` when Compile was pressed: the edit closes
    /// when a later compile applied.
    pub sent: Option<u64>,
}

/// The left tab: counts, the search, the definitions, the builtins and
/// the documentation listing.
pub fn list(
    ui: &Ui,
    app: &App,
    st: &mut ScriptsUi,
    fonts: &Fonts,
    dialogs: bool,
    actions: &mut Vec<Action>,
) {
    let l = match app.scripts_loaded() {
        Ok(l) => l,
        Err(why) => {
            if !app.session.is_open() {
                theme::empty_card(ui, fonts, "Open a map to see its scripts", || {});
                return;
            }
            ui.text_colored(theme::ERROR, icon::ALERT);
            ui.same_line();
            ui.text_wrapped(&why);
            ui.spacing();
            if ui.collapsing_header("Recordings##scripts-recordings", TreeNodeFlags::NONE) {
                recordings(ui, app, None, st, actions);
            }
            doc_row(ui, app, None, dialogs, actions);
            return;
        }
    };
    ui.text_disabled(format!(
        "{} globals · {} scripts · {} free nodes of {}",
        l.tables.globals().len(),
        l.tables.scripts().len(),
        l.free_nodes(),
        hs::RETAIL_NODES
    ));
    ui.text_disabled(format!("{} {}", icon::FILE, l.executable));
    external_row(ui, app, l, actions);
    eval_row(ui, app, st, actions);
    ui.set_next_item_width(-1.0);
    ui.input_text("##scripts-filter", &mut st.filter)
        .hint(format!("{} Search names and source", icon::SEARCH))
        .build();
    let selected = app.scripts.selected;
    let mut section = None;
    let mut shown = 0;
    for e in l.matching(&st.filter) {
        let item = e.source.item;
        let global = matches!(item, Item::Global(_));
        if section != Some(global) {
            theme::section(ui, if global { "Globals" } else { "Scripts" });
            section = Some(global);
        }
        let label = format!(
            "{}  {}##script-{item:?}",
            e.source.name,
            if global {
                e.type_name.clone()
            } else {
                format!("{} · {}", e.kind, e.type_name)
            }
        );
        if ui
            .selectable_config(label)
            .selected(selected == Some(item))
            .build()
            && selected != Some(item)
        {
            actions.push(Action::Script(ScriptAction::Select(item)));
        }
        shown += 1;
    }
    if shown == 0 {
        ui.text_disabled(if l.entries.is_empty() {
            "This map has no scripts or globals."
        } else {
            "Nothing matches the search."
        });
    }
    ui.spacing();
    if ui.collapsing_header("Builtins##scripts-builtins", TreeNodeFlags::NONE) {
        builtins(ui, l, st);
    }
    if ui.collapsing_header("Recordings##scripts-recordings", TreeNodeFlags::NONE) {
        recordings(ui, app, Some(l), st, actions);
    }
    doc_row(ui, app, Some(l), dialogs, actions);
}

/// The map's recorded animations: each with its length, and for the picked
/// one the scripts that play it, its initial state and every event. Read
/// only (docs/editor/rich-editors.md, "Recordings").
fn recordings(
    ui: &Ui,
    app: &App,
    l: Option<&Loaded>,
    st: &mut ScriptsUi,
    actions: &mut Vec<Action>,
) {
    use crate::app::recordings::NONE;
    let rows = match app.recordings_loaded() {
        Ok(rows) => rows,
        Err(why) => {
            ui.text_disabled(why);
            return;
        }
    };
    if rows.is_empty() {
        ui.text_disabled(NONE);
        return;
    }
    for (i, r) in rows.iter().enumerate() {
        let events = match &r.decoded {
            Ok(d) => format!("{} events", d.events.len()),
            Err(_) => "does not read".to_owned(),
        };
        let label = format!(
            "{}  {} ticks · {events}##recording-{i}",
            r.recorded.name, r.recorded.ticks
        );
        if ui
            .selectable_config(label)
            .selected(st.recording == Some(i))
            .build()
        {
            st.recording = Some(i);
        }
    }
    let Some((i, r)) = st
        .recording
        .and_then(|i| rows.get(i).map(|r| (i, r)))
    else {
        return;
    };
    theme::section(ui, &r.recorded.name);
    ui.text_disabled(format!(
        "stream version {} · control version {} · {} bytes",
        r.recorded.version,
        r.recorded.control_version,
        r.recorded.stream.len()
    ));
    let users = l.map(|l| l.recording_uses(rows, i));
    if users.as_ref().is_none_or(Vec::is_empty) {
        ui.text_disabled(if users.is_some() {
            "No script plays it."
        } else {
            "Which scripts play it needs the scripts, which need the game's executable."
        });
    } else if let Some(users) = users {
        ui.text_disabled(format!(
            "Played by {}",
            users
                .iter()
                .map(|(_, name, n)| format!("{name} ×{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    play_row(ui, app, r, actions);
    let d = match &r.decoded {
        Ok(d) => d,
        Err(why) => {
            ui.text_colored(theme::ERROR, icon::ALERT);
            ui.same_line();
            ui.text_wrapped(why);
            return;
        }
    };
    ui.text_disabled(format!(
        "Starts from: {}",
        fields_text(&d.initial.fields)
    ));
    let clipper = ListClipper::new(d.events.len()).begin(ui);
    for k in clipper.iter() {
        let e = &d.events[k];
        ui.text(format!(
            "{:>5}  {}  {}",
            e.tick,
            e.name,
            fields_text(&e.fields)
        ));
    }
    if d.trailing > 0 {
        ui.text_disabled(format!("{} bytes follow the end event.", d.trailing));
    }
}

/// **Play on unit** (the first player's): the paired game plays the
/// recording and the Scene viewport draws the path the unit takes.
fn play_row(ui: &Ui, app: &App, r: &crate::app::recordings::Row, actions: &mut Vec<Action>) {
    let paired = app
        .pairing
        .published
        .as_ref()
        .is_some_and(|s| s["state"] == "connected");
    let disabled = ui.begin_disabled_with_cond(!paired || r.decoded.is_err());
    if ui.button(format!("{} Play on unit##recording-play", icon::RUN)) {
        actions.push(Action::Script(ScriptAction::PlayRecording {
            name: r.recorded.name.clone(),
            player: 0,
        }));
    }
    disabled.end();
    ui.same_line();
    ui.text_disabled(if paired {
        "on player 0, in the paired game"
    } else {
        "pair with a game to play it"
    });
}

fn fields_text(fields: &[hs::recording::Named]) -> String {
    use hs::recording::Value;
    fields
        .iter()
        .map(|(name, v)| {
            let v = match v {
                Value::Unsigned(n) => format!("{n:#x}"),
                Value::Signed(n) => n.to_string(),
                Value::Reals(r) => format!(
                    "({})",
                    r.iter().map(|f| format!("{f:.3}")).collect::<Vec<_>>().join(", ")
                ),
            };
            format!("{name} {v}")
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// The expression box and **Evaluate in game**: the paired game compiles
/// the expression against the loaded map's scripts and answers its value
/// (`hs.eval`); the last answer shows under the box.
fn eval_row(ui: &Ui, app: &App, st: &mut ScriptsUi, actions: &mut Vec<Action>) {
    let paired = app
        .pairing
        .published
        .as_ref()
        .is_some_and(|s| s["state"] == "connected");
    ui.set_next_item_width(-1.0);
    ui.input_text("##scripts-eval", &mut st.expression)
        .hint(format!("{} An expression, such as (+ 1 2)", icon::RUN))
        .build();
    let disabled = ui.begin_disabled_with_cond(!paired || st.expression.trim().is_empty());
    if ui.button("Evaluate in game##scripts-eval-run") {
        actions.push(Action::Script(ScriptAction::Eval {
            expression: st.expression.trim().to_owned(),
            ty: None,
        }));
    }
    disabled.end();
    if ui.is_item_hovered() {
        ui.tooltip_text(
            "Compile the expression against the loaded map's scripts and run it in\nthe paired game, over a snapshot of the map's global values. A builtin\nthat needs the game's objects is refused by name.",
        );
    }
    ui.same_line();
    if !paired {
        ui.text_disabled("pair with a game to evaluate");
    }
    match &app.scripts.evaluated {
        Some((expression, Ok(value))) => ui.text_wrapped(format!("{expression}  ->  {value}")),
        Some((expression, Err(why))) => {
            ui.text_colored(theme::ERROR, icon::ALERT);
            ui.same_line();
            ui.text_wrapped(format!("{expression}: {why}"));
        }
        None => {}
    }
}

/// "Open in VS Code" and "Open in Zed", and the working copy's state.
fn external_row(ui: &Ui, app: &App, l: &Loaded, actions: &mut Vec<Action>) {
    if ui.button("Open in VS Code##scripts-code") {
        actions.push(Action::Script(ScriptAction::OpenInEditor(Some("code"))));
    }
    if ui.is_item_hovered() {
        ui.tooltip_text(
            "Write every script and global to a working copy and open it in VS Code\nat the one shown. The extension (h2 rules editors install) checks it\nagainst this map's builtins and names, and asks Studio where a name is\nused. Each save there comes back here; Compile working copy writes what\nit changed into the map.",
        );
    }
    ui.same_line();
    if ui.button("Open in Zed##scripts-zed") {
        actions.push(Action::Script(ScriptAction::OpenInEditor(Some("zed"))));
    }
    if ui.is_item_hovered() {
        ui.tooltip_text("As Open in VS Code, in Zed.");
    }
    let Some(w) = app.scripts.external.as_ref().filter(|w| w.map == l.map) else {
        return;
    };
    ui.text_disabled(format!("{} {}", icon::FILE, w.path.display()));
    let color = if w.changes.is_empty() { theme::DIM } else { theme::WARN };
    ui.text_colored(color, w.summary());
    if w.stale {
        ui.text_colored(
            theme::WARN,
            "The map's scripts changed since the working copy was written; Studio left your edits in it.",
        );
    }
    let disabled = ui.begin_disabled_with_cond(w.changes.changed.is_empty() && w.changes.added.is_empty());
    if ui.button("Compile working copy##scripts-compile-working") {
        actions.push(Action::Script(ScriptAction::CompileWorking));
    }
    disabled.end();
    if ui.is_item_hovered() {
        ui.tooltip_text(
            "Compile the definitions the file changed or added into the map's\nscripts. Ones it removed stay: the compiler adds and replaces.",
        );
    }
}

fn builtins(ui: &Ui, l: &Loaded, st: &mut ScriptsUi) {
    let count = l.naming.len();
    let by =
        |want: fn(Option<NameFrom>) -> bool| (0..count).filter(|&i| want(l.naming.from(i))).count();
    ui.text_disabled(format!(
        "{} named by this map's scripts · {} by the documentation · {} unnamed",
        by(|f| f == Some(NameFrom::Map)),
        by(|f| matches!(f, Some(NameFrom::Doc(_)))),
        by(|f| f.is_none()),
    ));
    ui.text_disabled("An unnamed builtin shows as #builtin:<index>; nothing guesses a name.");
    ui.set_next_item_width(-1.0);
    ui.input_text("##scripts-builtins-filter", &mut st.builtins)
        .hint(format!("{} Filter builtins", icon::SEARCH))
        .build();
    let want = st.builtins.to_lowercase();
    let rows: Vec<(usize, String)> = (0..count)
        .filter_map(|i| {
            let call = l.naming.call_form(&l.catalog, i).ok()?;
            let returns = l
                .catalog
                .function(i as u16)
                .ok()
                .and_then(|f| l.catalog.type_name(f.return_type()).ok())
                .unwrap_or("?");
            let row = format!("{call} → {returns}");
            (want.is_empty() || row.to_lowercase().contains(&want)).then_some((i, row))
        })
        .collect();
    let clipper = ListClipper::new(rows.len()).begin(ui);
    for k in clipper.iter() {
        let (i, row) = &rows[k];
        let from = l.naming.from(*i);
        let color = if from.is_none() {
            theme::DIM
        } else {
            theme::TEXT
        };
        ui.text_colored(color, row);
        if ui.is_item_hovered() {
            let mut tip = match from {
                Some(NameFrom::Map) => "Named by a call in this map's scripts.".to_owned(),
                Some(NameFrom::Doc(d)) => {
                    let mut t =
                        "Named by the script documentation: its return and parameter types match."
                            .to_owned();
                    if let Some(Ok(doc)) = l.doc.as_ref().map(|(_, d)| d)
                        && let Some(e) = doc.functions.get(d)
                        && !e.description.is_empty()
                    {
                        t.push_str("\n\n");
                        t.push_str(&e.description);
                    }
                    t
                }
                None => {
                    "No scenario calls it and no documentation entry fits its signature.".to_owned()
                }
            };
            if let Ok(f) = l.catalog.function(*i as u16) {
                if f.flags() & 2 != 0 {
                    tip.push_str("\nIt reads its own arguments: the parameter list is not its whole signature.");
                }
                if let Some(u) = f.usage() {
                    tip.push_str(&format!("\nUsage text in the executable: {u}"));
                }
            }
            ui.set_item_tooltip(tip);
        }
    }
}

/// The script documentation listing: where it is, what it named, and a
/// file picker.
fn doc_row(ui: &Ui, app: &App, l: Option<&Loaded>, dialogs: bool, actions: &mut Vec<Action>) {
    theme::section(ui, "Script documentation");
    ui.text_wrapped(
        "Names for builtins no script calls can come from the editing kit's script \
         documentation (hs_doc). It is read from your copy each time and never stored.",
    );
    match &app.scripts.doc_path {
        None => ui.text_disabled("None chosen."),
        Some(p) => ui.text_disabled(format!("{} {}", icon::FILE, p.display())),
    }
    match l.and_then(|l| l.doc.as_ref().map(|d| (l, d))) {
        Some((_, (_, Err(e)))) => {
            ui.text_colored(theme::ERROR, icon::ALERT);
            ui.same_line();
            ui.text_wrapped(e);
        }
        Some((l, (_, Ok(doc)))) => {
            if let Some(r) = &l.naming.report {
                ui.text_disabled(format!(
                    "{} entries ({} without a fixed signature) · {} agree with this map · {} builtins named",
                    r.functions, r.without_signature, r.anchors, r.named
                ));
                if !r.disagree.is_empty() {
                    ui.text_colored(
                        theme::WARN,
                        format!(
                            "{} entries name a builtin with other types: {}",
                            r.disagree.len(),
                            r.disagree.join(", ")
                        ),
                    );
                }
            }
            if !doc.unreadable.is_empty() {
                ui.text_disabled(format!(
                    "{} signature lines did not read",
                    doc.unreadable.len()
                ));
            }
        }
        None => {}
    }
    let pick = ui.begin_disabled_with_cond(!dialogs);
    if ui.button(format!("{} Choose...##scripts-doc", icon::FOLDER_OPEN))
        && let Some(p) = rfd::FileDialog::new()
            .set_title("Script documentation (hs_doc)")
            .add_filter("Text", &["txt"])
            .pick_file()
    {
        actions.push(Action::Script(ScriptAction::SetDoc(Some(p))));
    }
    pick.end();
    if app.scripts.doc_path.is_some() {
        ui.same_line();
        if ui.button(format!("{} Clear##scripts-doc-clear", icon::CLOSE)) {
            actions.push(Action::Script(ScriptAction::SetDoc(None)));
        }
    }
}

/// The Inspector: the selected definition's source, names as links, and
/// the definitions that use it.
pub fn source(
    ui: &Ui,
    app: &App,
    st: &mut ScriptsUi,
    fonts: &Fonts,
    actions: &mut Vec<Action>,
) {
    let l = match app.scripts_loaded() {
        Ok(l) => l,
        Err(why) => {
            ui.text_wrapped(why);
            return;
        }
    };
    let Some(e) = app.scripts.selected.and_then(|i| l.entry(i)) else {
        ui.text_disabled("Pick a script or a global in the Scripts tab.");
        return;
    };
    let item = e.source.item;
    {
        let _font = ui.push_font(fonts.bold);
        ui.text(&e.source.name);
    }
    ui.text_disabled(format!("{} · {} · {} nodes", e.kind, e.type_name, e.nodes));
    ui.text_disabled(
        "Decompiled from the map: comments, layout and the file it came from are not kept.",
    );
    edit_row(ui, app, st, item, &e.source.source, actions);
    ui.separator();
    if st.edit.as_ref().is_some_and(|x| x.item == item) {
        return;
    }
    let text = &e.source.source;
    let scenario = app.session.scenario();
    let mut links = e.links.iter().enumerate().peekable();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let end = at + line.len();
        let body = if line.ends_with('\n') { end - 1 } else { end };
        let mut cursor = at;
        let mut first = true;
        let plain = |s: &str, first: &mut bool| {
            if s.is_empty() {
                return;
            }
            if !*first {
                ui.same_line_with_spacing(0.0, 0.0);
            }
            ui.text(s);
            *first = false;
        };
        while let Some(&(k, link)) = links.peek() {
            if link.start >= body {
                break;
            }
            plain(&text[cursor..link.start], &mut first);
            if !first {
                ui.same_line_with_spacing(0.0, 0.0);
            }
            let target = l.resolve(link, scenario);
            let color = if target.is_ok() {
                theme::ACCENT
            } else {
                theme::WARN
            };
            ui.text_colored(color, &text[link.start..link.end]);
            first = false;
            if ui.is_item_hovered() {
                ui.set_mouse_cursor(Some(MouseCursor::Hand));
                ui.set_item_tooltip(match &target {
                    Ok(t) => describe(l, t),
                    Err(why) => format!("{} {}: {why}", link.kind.name(), link.name),
                });
                if ui.is_mouse_clicked(MouseButton::Left) {
                    actions.push(Action::Script(ScriptAction::Follow { item, link: k }));
                }
            }
            cursor = link.end;
            links.next();
        }
        plain(&text[cursor..body], &mut first);
        if first {
            ui.text("");
        }
        at = end;
    }
    let (kind, name) = match item {
        Item::Script(_) => (LinkKind::Script, e.source.name.as_str()),
        Item::Global(_) => (LinkKind::Global, e.source.name.as_str()),
    };
    theme::section(ui, "Used by");
    let users = l.uses(kind, name);
    if users.is_empty() {
        ui.text_disabled("No script or global names it.");
    }
    for (user, user_name, n) in users {
        let label = format!("{user_name}  ×{n}##script-use-{user:?}");
        if ui.selectable(label) {
            actions.push(Action::Script(ScriptAction::Select(user)));
        }
    }
}

/// **Edit**, the text box with **Compile** and **Check**, the undo and
/// redo of compiles, and how the last compile ended.
fn edit_row(
    ui: &Ui,
    app: &App,
    st: &mut ScriptsUi,
    item: Item,
    source: &str,
    actions: &mut Vec<Action>,
) {
    let compiles = app.scripts.compiles;
    if let Some(x) = &st.edit
        && x.sent.is_some_and(|seen| compiles > seen)
        && matches!(&app.scripts.compile, Some(Outcome::Done(r)) if r.applied)
    {
        st.edit = None;
    }
    let editing = st.edit.as_ref().is_some_and(|x| x.item == item);
    if !editing {
        if ui.button("Edit##script-edit") {
            st.edit = Some(Edit {
                item,
                text: source.to_owned(),
                sent: None,
            });
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "Edit this definition's source here. Compile writes it into the\nmap's scripts: only its nodes change.",
            );
        }
    } else if let Some(x) = &mut st.edit {
        ui.input_text_multiline("##script-edit-text", &mut x.text, [-1.0, 180.0])
            .flags(InputTextMultilineFlags::ALLOW_TAB_INPUT)
            .build();
        if ui.button("Compile##script-compile") {
            x.sent = Some(compiles);
            actions.push(Action::Script(ScriptAction::Compile {
                source: x.text.clone(),
                apply: true,
            }));
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "Write the compiled source into the map as one undo step. A new\nstring, script or global, or more nodes than free slots, makes a table\ngrow, which needs Edit > Allow growing full blocks.",
            );
        }
        ui.same_line();
        if ui.button("Check##script-check") {
            actions.push(Action::Script(ScriptAction::Compile {
                source: x.text.clone(),
                apply: false,
            }));
        }
        ui.same_line();
        if ui.button("Revert##script-revert") {
            x.text = source.to_owned();
        }
        ui.same_line();
        if ui.button("Close##script-edit-close") {
            st.edit = None;
        }
    }
    if let Some(doc) = &app.scripts.edit {
        let (undo, redo) = doc.depth();
        if undo + redo > 0 {
            ui.same_line();
            let off = ui.begin_disabled_with_cond(undo == 0);
            if ui.button("Undo compile##script-undo") {
                actions.push(Action::Script(ScriptAction::CompileUndo { redo: false }));
            }
            off.end();
            ui.same_line();
            let off = ui.begin_disabled_with_cond(redo == 0);
            if ui.button("Redo compile##script-redo") {
                actions.push(Action::Script(ScriptAction::CompileUndo { redo: true }));
            }
            off.end();
        }
    }
    match &app.scripts.compile {
        None => {}
        Some(Outcome::Done(r)) => {
            let names: Vec<&str> = r.definitions.iter().map(|(n, _)| n.as_str()).collect();
            ui.text_colored(
                theme::DIM,
                format!(
                    "{} {}: {} nodes ({} reused, {} added), {} new string bytes, {} free nodes{}",
                    if r.applied { "Compiled" } else { "Checked" },
                    names.join(", "),
                    r.nodes,
                    r.reused,
                    r.grown,
                    r.new_strings,
                    r.free_nodes,
                    if r.grows { "; a table grows" } else { "" },
                ),
            );
        }
        Some(Outcome::Failed(Refusal::Problems(messages))) => {
            for m in messages {
                ui.text_colored(theme::ERROR, format!("{} {m}", icon::ALERT));
            }
        }
        Some(Outcome::Failed(other)) => {
            ui.text_colored(theme::ERROR, format!("{} {}", icon::ALERT, other.line()));
        }
    }
}

fn describe(l: &Loaded, target: &Target) -> String {
    match *target {
        Target::Item(item) => match l.entry(item) {
            Some(e) if matches!(item, Item::Script(_)) => format!("Show script {}", e.source.name),
            Some(e) => format!("Show global {}", e.source.name),
            None => format!("{item:?}"),
        },
        Target::Placement(kind, index) => {
            format!("Select {} {index} in Scene", layout::block(kind).label)
        }
        Target::Volume(index) => format!("Select trigger volume {index} in Scene"),
        Target::Tag(id) => format!("Open tag {:#010x} in Tags", id.0),
    }
}
