//! The Scripts tab of the Map workspace (docs/editor/rich-editors.md,
//! "Scripts"): the globals and scripts with a search, the builtins and
//! where their names come from, and in the Inspector the selected
//! definition's source with every name a link. Read only; each panel is
//! one scrolling body.

use dear_imgui_rs::{ListClipper, MouseButton, MouseCursor, TreeNodeFlags, Ui};
use hs::{Item, NameFrom};
use scenario_edit::layout;

use crate::app::scripts::{LinkKind, Loaded, Target};
use crate::app::{Action, App, ScriptAction};
use crate::theme::{self, Fonts, icon};

/// The tab's own state: the searches.
#[derive(Default)]
pub struct ScriptsUi {
    pub filter: String,
    pub builtins: String,
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
    doc_row(ui, app, Some(l), dialogs, actions);
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
pub fn source(ui: &Ui, app: &App, fonts: &Fonts, actions: &mut Vec<Action>) {
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
    ui.separator();
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
