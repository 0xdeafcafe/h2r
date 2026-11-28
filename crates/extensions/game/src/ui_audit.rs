//! The instantiated widgets retail draws. A tag or a kit model describes what
//! should appear; the text interfaces here hold what was actually resolved.
use std::collections::HashSet;

use agent_proto::{RpcError, code, method};
use guest_layout::datum::{DatumArray, layout};
use guest_layout::{DatumHandle, GuestMemory, points};
use serde_json::{Value, json};

use crate::{list_class, ui_kit::dialog_tag, ui_watch};

const LIST: u32 = 1;
const ITEM: u32 = 2;
const BUTTON: u32 = 3;
const TABS: u32 = 5;
const TEXT: u32 = 6;
const SHADER_BITMAP: u32 = 9;
const RECENT_PLAYERS: u32 = 0x0045_b4a0; // 0x2b3968
const SHORT_TEXT: u32 = 0x0045_88b0;
const LONG_TEXT: u32 = 0x0045_8930;

fn ptr(m: &dyn GuestMemory, at: u32, offset: u32) -> Option<u32> {
    m.u32(at.checked_add(offset)?).ok().filter(|p| *p != 0)
}

fn pane_index(m: &dyn GuestMemory, at: u32) -> Option<u32> {
    m.u16(at.checked_add(0x5f0)?)
        .ok()
        .filter(|i| *i < 256)
        .map(u32::from)
}

struct Focus {
    accepts_controller: bool,
    widget: u32,
    binding: Option<DatumHandle>,
    datum: Option<u32>,
    pane: Option<u32>,
    list: Option<u32>,
    row: Option<u32>,
}

// The dispatcher at 0x23515d climbs the focused leaf's parents to the
// first widget accepting that controller. 0x235246 requires its root to
// be the active screen, with neither transition bit set.
fn focus(m: &dyn GuestMemory, channel: u32, screen: u32) -> Option<Focus> {
    let widget = ptr(m, channel, 0x34)?;
    let mut at = widget;
    let mut child = None;
    let mut f = Focus {
        accepts_controller: false,
        widget,
        binding: None,
        datum: None,
        pane: None,
        list: None,
        row: None,
    };
    for _ in 0..64 {
        let kind = m.u32(at.checked_add(4)?).ok()?;
        let accepts_here = m.u16(at.checked_add(8)?).ok()? & 1 != 0;
        f.accepts_controller |= accepts_here;
        if kind == TABS && child != ptr(m, at, 0x70) {
            return None;
        }
        if kind == ITEM
            && f.accepts_controller
            && f.list.is_none()
            && m.u8(at.checked_add(0x6e)?).ok()? != 0
        {
            let parent = ptr(m, at, 0x10)?;
            if m.u32(parent.checked_add(4)?).ok()? == LIST {
                f.list = Some(parent);
                let h = DatumHandle(m.u32(at.checked_add(0x70)?).ok()?);
                f.datum = ptr(m, parent, 0x70)
                    .and_then(|a| DatumArray::at(a).try_get(m, h).ok().flatten());
                f.binding = f.datum.map(|_| h);
                f.row = f.binding.map(|h| u32::from(h.index()));
            }
        } else if kind == BUTTON && f.accepts_controller && f.list.is_none() {
            f.row = m
                .u16(at.checked_add(0xf8)?)
                .ok()
                .filter(|i| *i != u16::MAX)
                .map(u32::from);
        }
        if kind == 0 && f.pane.is_none() {
            f.pane = pane_index(m, at);
        }
        if at == screen {
            return (kind == 0 && m.u8(at.checked_add(0x42)?).ok()? & 3 == 0).then_some(f);
        }
        child = Some(at);
        at = ptr(m, at, 0x10)?;
    }
    None
}

// The same profile lists also offer sign-in, create and delete. Classify
// the side-effect-free branch of each retail handler (0x24a30e, 0x2ba862)
// and include the classification in the token rechecked on the input poll.
fn profile_navigation(
    m: &dyn GuestMemory,
    surface: (u32, u32),
    list: u32,
    datum: Option<u32>,
    row: u32,
) -> Option<&'static str> {
    let datum = datum?;
    match (surface, m.u32(list).ok()?) {
        ((193, 0x0045_b0b8), 0x0045_af88)
            if m.u8(list.checked_add(0x89)?).ok()? == 1
                && m.u32(datum.checked_add(4)?).ok()? != u32::MAX =>
        {
            Some("inspect_profile")
        }
        ((194, 0x0045_c3f8), 0x0045_c468)
            if row == 0 && m.u32(list.checked_add(0x88)?).ok()? == 0 =>
        {
            Some("edit_profile")
        }
        _ => None,
    }
}

pub(crate) struct Target {
    pub token: String,
    pub focused_row: Option<u32>,
    pub id: u32,
    pub screen: u32,
}

struct Owner {
    target: Target,
    window: u32,
    slot: u32,
    focus: Focus,
}

fn owner(m: &dyn GuestMemory, closing: Option<u32>) -> Option<Owner> {
    let mut receiver = None;
    for slot in [ui_watch::SHARED, 0] {
        for window in 0..6 {
            let Some(ch) = ui_watch::channel(window, slot) else {
                continue;
            };
            let screen = m.u32(ch + 8).ok()?;
            let pending = m.u32(ch + 12).ok()?;
            if pending == 0 && (screen == 0 || closing == Some(screen)) {
                continue;
            }
            // A pending screen may receive the next input poll.
            if pending != 0 {
                return None;
            }
            // A failed root/transition gate at 0x235246 consumes the event
            // (0x235199). Only a valid chain with no matching controller
            // passes unconditionally to the next slot.
            let focus = focus(m, ch, screen)?;
            if focus.accepts_controller {
                let id = u32::from(m.u16(screen.checked_add(0x70)?).ok()?);
                let vt = m.u32(screen).ok()?;
                let pane = focus.pane.map_or(-1, i64::from);
                let list = focus.list.unwrap_or(0);
                let leaf = focus.widget;
                let handle = focus.binding.map_or(u32::MAX, |h| h.0);
                let navigation = focus
                    .row
                    .and_then(|row| profile_navigation(m, (id, vt), list, focus.datum, row))
                    .unwrap_or("");
                let token = format!(
                    "{window}/{slot}/{screen:x}/{vt:x}/{id}/{pane}/{list:x}/{leaf:x}/{handle:x}/{navigation}"
                );
                // Retail tries the personal slot if the shared receiver
                // declines this particular event. An action-independent
                // token cannot promise which of two receivers will act.
                if receiver.is_some() {
                    return None;
                }
                receiver = Some(Owner {
                    target: Target {
                        token,
                        focused_row: focus.row,
                        id,
                        screen,
                    },
                    window,
                    slot,
                    focus,
                });
            }
            // The first occupied window consumes this slot's dispatch attempt.
            break;
        }
    }
    receiver
}

pub(crate) fn target(m: &dyn GuestMemory) -> Option<Target> {
    owner(m, None).map(|o| o.target)
}

/// The settled receiver exposed when a kit value picker dismisses its overlay.
pub(crate) fn target_beneath(m: &dyn GuestMemory, closing: u32) -> Option<Target> {
    owner(m, Some(closing)).map(|o| o.target)
}

struct Widget {
    at: u32,
    kind: u32,
    enabled: bool,
    children: Vec<Widget>,
}

impl Widget {
    fn shows_emblem(&self, m: &dyn GuestMemory) -> bool {
        self.enabled && ((self.kind == SHADER_BITMAP
            && rect(m, self.at).is_some_and(|r| {
                [r.top, r.left, r.bottom, r.right] == crate::clanker_card::EMBLEM_RECT
            }))
            || self.children.iter().any(|child| child.shows_emblem(m)))
    }

    // A damaged hierarchy must not monopolize a tick or recurse forever.
    fn read(
        m: &dyn GuestMemory,
        at: u32,
        seen: &mut HashSet<u32>,
        depth: u8,
        parent_enabled: bool,
    ) -> Option<Self> {
        if depth > 32 || seen.len() >= 512 || !seen.insert(at) {
            return None;
        }
        let kind = m.u32(at.checked_add(4)?).ok()?;
        // Permanent draw gates checked at 0x22e3b4, inherited by children
        // through the renderer's tree walk (0x23566a).
        let enabled = parent_enabled
            && m.u8(at.checked_add(0x6d)?).ok()? != 0
            && m.u8(at.checked_add(0x6e)?).ok()? != 0;
        let mut children = Vec::new();
        let mut next = m.u32(at.checked_add(0x14)?).ok()?;
        while next != 0 {
            if ptr(m, next, 0x10) != Some(at) {
                return None;
            }
            children.push(Self::read(m, next, seen, depth + 1, enabled)?);
            next = m.u32(next.checked_add(0x18)?).ok()?;
        }
        Some(Self {
            at,
            kind,
            enabled,
            children,
        })
    }
}

/// In UI units, after the list has placed its widgets (0x2bacbc).
fn rect(m: &dyn GuestMemory, widget: u32) -> Option<tags::ui::Rect> {
    let at = widget.checked_add(0x20)?;
    let r = tags::ui::Rect {
        top: m.u16(at).ok()? as i16,
        left: m.u16(at + 2).ok()? as i16,
        bottom: m.u16(at + 4).ok()? as i16,
        right: m.u16(at + 6).ok()? as i16,
    };
    (r.width() > 0 && r.height() > 0).then_some(r)
}

/// Live findings for one active screen. Resolve declared content after
/// dropping the guest guard: row models can read settings or Live state.
#[derive(Clone, Default)]
pub(crate) struct ObservedFindings {
    pub layout: Vec<Value>,
    pub content: Vec<Value>,
    authored: [Option<Authored>; 2],
}

#[derive(Clone, Copy)]
struct Authored {
    id: Option<u32>,
    screen: u32,
    vtable: u32,
    focus: Option<usize>,
}

impl ObservedFindings {
    pub fn resolve_authored(&mut self, metrics: &dyn crate::ui_kit::view::Metrics) {
        use crate::ui_kit::{content, preview::From};

        for authored in self.authored.into_iter().flatten() {
            let Some(spec) = content::runtime_spec(authored.vtable, authored.focus) else {
                continue;
            };
            let Ok(view) = From::Synthetic.view_with_metrics(&spec, "live", metrics) else {
                continue;
            };
            self.content.extend(content::check(&spec, &view, metrics).into_iter().map(|issue| {
                json!({
                    "issue": issue,
                    "screen": authored.id,
                    "screen_instance": authored.screen,
                    "source": "page_model",
                })
            }));
        }
    }

    pub fn append(&mut self, mut other: Self) {
        self.layout.append(&mut other.layout);
        self.content.append(&mut other.content);
        self.authored[1] = other.authored[0];
    }
}

/// Bounds and resolved text from the active UI tree, sampled at the end
/// of the update. The caller resolves host-authored metadata separately.
pub(crate) fn observe(
    m: &dyn GuestMemory,
    screen: u32,
    id: Option<u32>,
    channel: Option<u32>,
) -> ObservedFindings {
    use crate::ui_kit::{
        content::{Finding, case_findings, check_text},
        layout::{SAFE, bounds_issues},
    };

    struct Check<'a> {
        m: &'a dyn GuestMemory,
        screen: u32,
        id: Option<u32>,
        panel: Option<tags::ui::Rect>,
        out: ObservedFindings,
    }

    impl Check<'_> {
        fn content(&mut self, node: &Widget, issue: Finding, text: Option<&str>) {
            self.out.content.push(json!({
                "issue": issue,
                "screen": self.id,
                "widget": node.at,
                "text": text,
            }));
        }

        fn visit(&mut self, node: &Widget, parent: Option<&Widget>) {
            if !node.enabled {
                return;
            }
            if node.kind == ITEM && self.m.u32(node.at + 0x70).ok() == Some(u32::MAX) {
                return; // An unbound slot in the reusable widget pool.
            }
            let widget_id = || format!("widget.{:x}", node.at);
            let text = matches!(node.kind, TEXT | BUTTON)
                .then(|| read_text(self.m, node))
                .flatten();
            if matches!(node.kind, TEXT | BUTTON) {
                // Empty secondary cells and blank pane text are valid.
                // A button and the first text of a bound row require a label.
                let required = node.kind == BUTTON
                    || parent.is_some_and(|p| {
                        p.kind == ITEM
                            && p.children.iter().find(|w| w.kind == TEXT).map(|w| w.at) == Some(node.at)
                    });
                if let Some(text) = text.as_deref() {
                    for issue in check_text(&widget_id(), text, required) {
                        self.content(node, issue, Some(text));
                    }
                } else if required {
                    self.content(
                        node,
                        Finding::new("unreadable_text", widget_id(), "Required label could not be read"),
                        None,
                    );
                }
            }
            if node.kind == LIST {
                let labels = node.children.iter()
                    .filter(|item| item.enabled
                        && item.kind == ITEM
                        && self.m.u32(item.at + 0x70).ok().is_some_and(|h| h != u32::MAX))
                    .filter_map(|item| {
                        let label = item.children.iter().find(|w| w.enabled && w.kind == TEXT)?;
                        read_text(self.m, label).map(|text| (format!("widget.{:x}", label.at), text))
                    });
                for issue in case_findings(labels) {
                    self.content(node, issue, None);
                }
            }
            if matches!(node.kind, TEXT | BUTTON | ITEM)
                && let Some(r) = rect(self.m, node.at)
                && !text.as_deref().is_some_and(|s| s.trim().is_empty())
            {
                let lobby = (self.id == Some(crate::lobby_options::SCREEN))
                    .then(|| crate::lobby_options::container(self.m, self.screen, node.at))
                    .flatten();
                let panel = self.panel.map(|p| ("modal panel", p));
                let parent_box = parent.and_then(|p| {
                    let name = match (node.kind, p.kind) {
                        (ITEM, LIST) => "list",
                        (TEXT, ITEM | BUTTON) => "row",
                        _ => return None,
                    };
                    Some((name, rect(self.m, p.at)?))
                });
                for issue in bounds_issues(
                    widget_id,
                    r,
                    SAFE,
                    lobby.into_iter().chain(panel).chain(parent_box),
                ) {
                    self.out.layout.push(json!({
                        "issue": issue,
                        "screen": self.id,
                        "widget": node.at,
                        "rect": [r.top, r.left, r.bottom, r.right],
                        "text": text,
                    }));
                }
            }
            for child in &node.children {
                if node.kind != TABS || ptr(self.m, node.at, 0x70) == Some(child.at) {
                    self.visit(child, Some(node));
                }
            }
        }
    }

    let Some(tree) = Widget::read(m, screen, &mut HashSet::new(), 0, true) else {
        return ObservedFindings::default();
    };
    let vtable = m.u32(screen).ok();
    let dialog = vtable.and_then(dialog_tag::by_vtable);
    let mut check = Check {
        m,
        screen,
        id,
        panel: dialog.map(|d| d.base.content_area()),
        out: ObservedFindings {
            authored: [
                vtable.map(|vtable| Authored {
                    id,
                    screen,
                    vtable,
                    focus: channel.and_then(|ch| focus(m, ch, screen)?.row.map(|r| r as usize)),
                }),
                None,
            ],
            ..ObservedFindings::default()
        },
    };
    check.visit(&tree, None);
    if tree.enabled
        && dialog.is_some_and(|d| d.name == crate::clanker_card::SCREEN)
        && !tree.shows_emblem(m)
    {
        check.content(
            &tree,
            Finding::new("missing_content", "emblem", "Clanker card has no emblem image"),
            None,
        );
    }
    check.out
}

// 0x253b1a resolves the id into a temporary then calls the text
// interface's setter. 0x22f50d and 0x22f545 copy it to interface +0x44.
fn read_text(m: &dyn GuestMemory, widget: &Widget) -> Option<String> {
    let offset = if widget.kind == BUTTON { 0x70 } else { 0x74 };
    let interface = widget.at.checked_add(offset)?;
    let cap = match m.u32(interface).ok()? {
        SHORT_TEXT => 32,
        LONG_TEXT => 256,
        _ => return None,
    };
    m.utf16(interface.checked_add(0x44)?, cap - 1).ok()
}

fn label(m: &dyn GuestMemory, widget: &Widget) -> Value {
    let text = read_text(m, widget);
    let status = match &text {
        None => "unreadable",
        Some(s) if s.trim().is_empty() => "empty",
        Some(_) => "resolved",
    };
    json!({"address":widget.at, "text":text, "status":status, "enabled":widget.enabled})
}

fn row_labels(m: &dyn GuestMemory, node: &Widget, out: &mut Vec<Value>) {
    if matches!(node.kind, TEXT | BUTTON) {
        out.push(label(m, node));
    }
    for child in &node.children {
        row_labels(m, child, out);
    }
}

// Find the retail wgit via the same current-interface selection as 0x1482e8
// and 0x148098. Kit classes can supply a different wgit for the same id.
fn screen_tag(m: &dyn GuestMemory, vtable: u32, id: u32) -> Option<u32> {
    if let Some(tag) = list_class::built_tag(m, vtable) {
        return Some(tag);
    }
    let scenario = ptr(m, points::SCENARIO.base, 0)?;
    let globals = ptr(m, points::GLOBALS_TAG.base, 0)?;
    if m.u32(globals.checked_add(0x110)?).ok()? == 0 {
        return None;
    }
    let interface = ptr(m, globals, 0x114)?;
    let offset = match m.u16(scenario.checked_add(0x10)?).ok()? {
        2 => 0x84,
        0 => 0x8c,
        1 => 0x94,
        _ => return None,
    };
    let tag = |handle| dialog_tag::tag_of(m, handle).ok().flatten();
    let wgtz = tag(m.u32(interface.checked_add(offset)?).ok()?)?;
    let count = m.u32(wgtz.checked_add(8)?).ok()?;
    let entries = ptr(m, wgtz, 12)?;
    for i in 0..count.min(1024) {
        let reference = m.u32(entries.checked_add(i * 8 + 4)?).ok()?;
        if let Some(root) = tag(reference)
            && m.u16(root.checked_add(4)?).ok().map(u32::from) == Some(id)
        {
            return Some(root);
        }
    }
    None
}

fn list_spec(m: &dyn GuestMemory, tag: Option<u32>, index: Option<u32>) -> Option<(u32, usize)> {
    let (tag, index) = (tag?, index?);
    if index >= m.u32(tag.checked_add(list_class::SCREEN_PANES)?).ok()? {
        return None;
    }
    let pane = ptr(m, tag, list_class::SCREEN_PANES + 4)?
        .checked_add(index.checked_mul(list_class::PANE_SIZE)?)?;
    let lists = m.u32(pane.checked_add(0xc)?).ok()?;
    if lists == 0 {
        return Some((0, 0));
    }
    let list = ptr(m, pane, 0x10)?;
    m.u16(list.checked_add(6)?)
        .ok()
        .filter(|n| *n <= 256)
        .map(|n| (lists, usize::from(n)))
}

struct Pane<'a> {
    surface: Option<(u32, u32)>,
    m: &'a dyn GuestMemory,
    focus: Option<&'a Focus>,
    visible: Option<usize>,
    lists: Vec<Value>,
    labels: Vec<Value>,
    buttons: Vec<Value>,
    issues: Vec<Value>,
}

impl Pane<'_> {
    fn check_label(&mut self, label: &Value, row: Option<u32>) {
        if label["enabled"] != true {
            return;
        }
        let kind = match label["status"].as_str() {
            Some("empty") => "empty_label",
            Some("unreadable") => "unreadable_label",
            _ => return,
        };
        self.issues
            .push(json!({"kind":kind, "widget":label["address"], "row":row}));
    }

    fn list(&mut self, node: &Widget) {
        let m = self.m;
        let vt = m.u32(node.at).ok();
        let recent = vt == Some(RECENT_PLAYERS);
        let array = ptr(m, node.at, 0x70).map(DatumArray::at);
        let count = array
            .and_then(|a| a.header_field(m, layout::ACTIVE_COUNT).ok())
            .and_then(|c| usize::try_from(c).ok());
        let mut rows = Vec::new();
        let mut slots = 0;
        let mut bound = HashSet::new();
        for item in node.children.iter().filter(|c| c.kind == ITEM) {
            slots += 1;
            let Some(handle) = item.at.checked_add(0x70).and_then(|a| m.u32(a).ok()) else {
                self.issues
                    .push(json!({"kind":"unreadable_row", "widget":item.at}));
                continue;
            };
            if handle == u32::MAX {
                continue;
            } // Unbound pool slot in a scrolling list.
            let index = handle & 0xffff;
            let datum = array.and_then(|a| a.try_get(m, DatumHandle(handle)).ok().flatten());
            if datum.is_some() {
                if item.enabled {
                    bound.insert(index);
                }
            } else {
                self.issues
                    .push(json!({"kind":"missing_row_datum", "widget":item.at, "row":index}));
            }
            let clanker = if recent {
                datum.and_then(|at| {
                    let mut id = [0u8; 8];
                    m.read(at.checked_add(4)?, &mut id).ok()?;
                    Some(live_proto::bots::is_bot_xuid(live_proto::Xuid(
                        u64::from_le_bytes(id),
                    )))
                })
            } else {
                None
            };
            let mut labels = Vec::new();
            row_labels(m, item, &mut labels);
            let text = labels
                .iter()
                .filter(|l| l["enabled"] == true)
                .filter_map(|l| l["text"].as_str())
                .filter(|s| !s.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let status = if labels.is_empty() {
                "missing"
            } else if labels.iter().all(|l| l["status"] == "unreadable") {
                "unreadable"
            } else if text.is_empty() {
                "empty"
            } else {
                "resolved"
            };
            for label in &labels {
                self.check_label(label, Some(index));
            }
            if status != "resolved" && item.enabled {
                self.issues.push(
                    json!({"kind":format!("{status}_row_text"), "widget":item.at, "row":index}),
                );
            }
            let navigation = self
                .surface
                .and_then(|surface| profile_navigation(m, surface, node.at, datum, index));
            rows.push(json!({"index":index, "address":item.at, "text":if status == "missing" || status == "unreadable" { None } else { Some(text) },
                "status":status, "labels":labels, "clanker":clanker, "navigation":navigation}));
        }
        let expected = count.map(|n| n.min(self.visible.unwrap_or(slots)));
        if count.is_none() {
            self.issues
                .push(json!({"kind":"unreadable_item_count", "list":node.at}));
        } else if count.is_some_and(|n| n > 0)
            && (slots == 0 || expected.is_some_and(|n| bound.len() < n))
        {
            self.issues
                .push(json!({"kind":"fewer_rows_than_items", "list":node.at,
                "bound":bound.len(), "expected":expected, "item_count":count}));
        }
        let focused = self
            .focus
            .filter(|f| f.list == Some(node.at))
            .and_then(|f| f.row);
        self.lists.push(json!({"address":node.at, "vtable":vt, "kind":if recent {"recent_players"} else {"list"},
            "focused_row":focused, "item_count":count, "visible_capacity":self.visible, "rows":rows}));
    }

    fn visit(&mut self, node: &Widget) {
        match node.kind {
            TABS => return, // Each materialized tab gets its own pane.
            LIST => {
                if node.enabled {
                    self.list(node)
                }
                return;
            }
            BUTTON => {
                let l = label(self.m, node);
                let index = node
                    .at
                    .checked_add(0xf8)
                    .and_then(|a| self.m.u16(a).ok())
                    .map(u32::from);
                self.check_label(&l, index);
                self.buttons.push(json!({"index":index,"address":node.at,"text":l["text"],"status":l["status"],"labels":[l]}));
            }
            TEXT => {
                let l = label(self.m, node);
                self.check_label(&l, None);
                self.labels.push(l);
            }
            _ => {}
        }
        for child in &node.children {
            self.visit(child)
        }
    }
}

fn screen(
    m: &dyn GuestMemory,
    ch: u32,
    at: u32,
    window: u32,
    slot: u32,
    issues: &mut Vec<Value>,
) -> Value {
    let id = at
        .checked_add(0x70)
        .and_then(|a| m.u16(a).ok())
        .map(u32::from);
    let vt = m.u32(at).ok();
    let class = vt.and_then(list_class::class_name);
    let name = class.or_else(|| id.filter(|_| window < 5).and_then(ui_watch::dialog_name));
    let focus = focus(m, ch, at).filter(|f| f.accepts_controller);
    let mode = id
        .filter(|id| *id == ui_watch::JOIN_SCREEN)
        .and_then(|_| m.u32(at.checked_add(0x610)?).ok());
    let tag = vt.zip(id).and_then(|(v, i)| screen_tag(m, v, i));
    let mut panes = Vec::new();
    let mut selected = pane_index(m, at);
    let tree = Widget::read(m, at, &mut HashSet::new(), 0, true);
    if let Some(root) = tree.filter(|t| t.kind == 0) {
        let tabs = root.children.iter().find(|c| c.kind == TABS);
        if tabs.is_some_and(|tabs| tabs.children.is_empty()) {
            issues.push(json!({"kind":"empty_pane", "screen":id, "window":window, "slot":slot}));
        }
        if let Some(tabs) = tabs {
            selected = ptr(m, tabs.at, 0x70).and_then(|t| pane_index(m, t));
        }
        let roots: Vec<_> = tabs.map_or_else(|| vec![&root], |t| t.children.iter().collect());
        for pane in roots {
            let index = pane_index(m, pane.at);
            let active = if let Some(t) = tabs {
                ptr(m, t.at, 0x70) == Some(pane.at)
            } else {
                true
            };
            let spec = list_spec(m, tag, index);
            let mut view = Pane {
                surface: id.zip(vt),
                m,
                focus: focus.as_ref(),
                visible: spec
                    .filter(|(count, _)| *count != 0)
                    .map(|(_, visible)| visible),
                lists: vec![],
                labels: vec![],
                buttons: vec![],
                issues: vec![],
            };
            view.visit(pane);
            // Header and footer are children of the outer screen.
            if tabs.is_some() && active {
                view.visit(&root);
            }
            if let Some((expected, _)) = spec
                && (view.lists.len() as u32) < expected
            {
                view.issues.push(json!({"kind":"missing_list", "expected_lists":expected, "lists":view.lists.len()}));
            }
            if !view.buttons.is_empty() {
                let focused = focus
                    .as_ref()
                    .filter(|f| f.list.is_none())
                    .and_then(|f| f.row);
                view.lists.push(json!({"kind":"buttons","item_count":view.buttons.len(),"rows":view.buttons,"focused_row":focused}));
            }
            if view.lists.is_empty() && view.labels.iter().all(|l| l["enabled"] != true) {
                view.issues.push(json!({"kind":"empty_pane"}));
            }
            // Inactive tabs may have deferred row text until the first draw.
            if active {
                for mut issue in view.issues {
                    issue["pane"] = json!(index);
                    issue["window"] = json!(window);
                    issue["slot"] = json!(slot);
                    issue["screen"] = json!(id);
                    issues.push(issue);
                }
            }
            panes.push(
                json!({"index":index, "active":active, "labels":view.labels, "lists":view.lists}),
            );
        }
    } else {
        issues.push(json!({"kind":"unreadable_widgets","screen":id,"window":window,"slot":slot}));
    }
    json!({"id":id, "name":name, "kit":class.is_some(), "window":window, "slot":slot,
        "address":at, "vtable":vt, "mode":mode, "pane":selected, "panes":panes})
}

fn inspect(m: &dyn GuestMemory) -> Value {
    let owner = owner(m, None);
    let mut issues = Vec::new();
    let mut current = Value::Null;
    let mut active = Value::Null;
    let mut dialogs = Vec::new();
    for window in 0..6 {
        for slot in [ui_watch::SHARED, 0, 1, 2, 3] {
            if window == 5 && slot != 0 && slot != ui_watch::SHARED {
                continue;
            }
            let Some(ch) = ui_watch::channel(window, slot) else {
                continue;
            };
            let Ok(at) = m.u32(ch + 8) else {
                issues.push(json!({"kind":"unreadable_channel", "window":window, "slot":slot}));
                continue;
            };
            if at == 0 {
                continue;
            }
            let owner_here = owner
                .as_ref()
                .is_some_and(|o| o.window == window && o.slot == slot);
            if window == 5 && !current.is_null() && !owner_here {
                continue;
            }
            let data = screen(m, ch, at, window, slot, &mut issues);
            if owner_here {
                active = data.clone();
            }
            if window == 5 {
                current = data;
            } else {
                dialogs.push(data);
            }
        }
    }
    json!({"screen":current, "dialogs":dialogs, "active":active, "issues":issues,
        "target":owner.as_ref().map(|o| &o.target.token),
        "focused_row":owner.as_ref().and_then(|o| o.target.focused_row),
        "focused_widget":owner.as_ref().map(|o| o.focus.widget)})
}

pub(crate) fn install(agent: &agent::Agent) {
    let mut history = crate::ui_transition::History::default();
    agent.add_game_method(method::UI_AUDIT, agent::Access::Read, move |params, ctx| {
        if !params.is_null() && !params.as_object().is_some_and(|p| p.is_empty()) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "ui.audit takes no parameters",
            ));
        }
        let at = ctx.at.clone();
        ctx.read_guest(|m| {
            let mut result = inspect(m);
            let has_dialog = result["dialogs"].as_array().is_some_and(|d| !d.is_empty());
            let (transition, flags) = history.read(m, &at, has_dialog);
            result["transition"] = transition;
            if let Some(issues) = result["issues"].as_array_mut() {
                issues.extend(flags);
            }
            result
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const SCREEN: u32 = 0x1000;
    const LIST_AT: u32 = 0x2000;
    const ITEM_AT: u32 = 0x3000;
    const LABEL_AT: u32 = 0x4000;
    const ARRAY: u32 = 0x5000;

    fn widget(m: &mut FakeGuest, addr: u32, kind: u32, parent: u32) {
        m.map(addr, 0x800)
            .put_u32(addr + 4, kind)
            .put_u16(addr + 8, 1)
            .put_u32(addr + 0x10, parent)
            .put_u8(addr + 0x6d, 1)
            .put_u8(addr + 0x6e, 1);
        if parent != 0 {
            let mut link = parent + 0x14;
            while m.u32(link).unwrap() != 0 {
                link = m.u32(link).unwrap() + 0x18;
            }
            m.put_u32(link, addr);
        }
    }

    #[test]
    fn layout_findings_report_live_rows_and_their_container() {
        let mut m = FakeGuest::new();
        widget(&mut m, SCREEN, 0, 0);
        widget(&mut m, LIST_AT, LIST, SCREEN);
        widget(&mut m, ITEM_AT, ITEM, LIST_AT);
        widget(&mut m, LABEL_AT, TEXT, ITEM_AT);
        let place = |m: &mut FakeGuest, at, bounds: [i16; 4]| {
            for (i, edge) in bounds.into_iter().enumerate() {
                m.put_u16(at + 0x20 + i as u32 * 2, edge as u16);
            }
        };
        place(&mut m, LIST_AT, [100, -200, -160, 200]);
        place(&mut m, ITEM_AT, [-145, -180, -170, 180]);
        place(&mut m, LABEL_AT, [-146, -172, -164, 278]);
        let findings = observe(&m, SCREEN, Some(80), None).layout;
        assert_eq!(findings.len(), 3);
        assert_eq!(findings[0]["issue"]["container"], "list");
        assert_eq!(findings[0]["widget"], ITEM_AT);
        assert_eq!(findings[1]["issue"]["kind"], "unsafe");
        assert_eq!(findings[1]["widget"], LABEL_AT);
        assert_eq!(findings[2]["issue"]["container"], "row");
        m.put_u8(ITEM_AT + 0x6e, 0);
        assert!(observe(&m, SCREEN, Some(80), None).layout.is_empty());
    }

    #[test]
    fn live_content_reports_bad_strings_in_the_active_tree() {
        let mut m = FakeGuest::new();
        widget(&mut m, SCREEN, 0, 0);
        widget(&mut m, LIST_AT, LIST, SCREEN);
        widget(&mut m, ITEM_AT, ITEM, LIST_AT);
        widget(&mut m, LABEL_AT, TEXT, ITEM_AT);
        let interface = LABEL_AT + 0x74;
        m.put_u32(interface, LONG_TEXT);
        let set = |m: &mut FakeGuest, text: &str| {
            for (i, unit) in text.encode_utf16().chain([0]).enumerate() {
                m.put_u16(interface + 0x44 + i as u32 * 2, unit);
            }
        };
        set(&mut m, "Invite {player}");
        let issues = observe(&m, SCREEN, Some(80), None).content;
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0]["issue"]["kind"], "placeholder");
        assert_eq!(issues[0]["widget"], LABEL_AT);
        set(&mut m, "");
        let issues = observe(&m, SCREEN, Some(80), None).content;
        assert_eq!(issues[0]["issue"]["kind"], "empty_text");
        set(&mut m, "Invite a Friend");
        assert!(observe(&m, SCREEN, Some(80), None).content.is_empty());
        set(&mut m, "TODO");
        m.put_u8(ITEM_AT + 0x6e, 0);
        assert!(observe(&m, SCREEN, Some(80), None).content.is_empty());
    }

    #[test]
    fn the_card_emblem_must_be_a_visible_shader_widget() {
        let mut m = FakeGuest::new();
        widget(&mut m, SCREEN, 0, 0);
        let shows_emblem = |m: &FakeGuest| {
            Widget::read(m, SCREEN, &mut HashSet::new(), 0, true)
                .unwrap()
                .shows_emblem(m)
        };
        assert!(!shows_emblem(&m));

        widget(&mut m, ITEM_AT, SHADER_BITMAP, SCREEN);
        for (i, edge) in crate::clanker_card::EMBLEM_RECT.into_iter().enumerate() {
            m.put_u16(ITEM_AT + 0x20 + i as u32 * 2, edge as u16);
        }
        assert!(shows_emblem(&m));
        m.put_u8(ITEM_AT + 0x6d, 0);
        assert!(!shows_emblem(&m));
    }

    fn channel(m: &mut FakeGuest, window: u32, slot: u32, screen: u32, leaf: u32) {
        let ch = ui_watch::channel(window, slot).unwrap();
        m.put_u32(ch + 8, screen).put_u32(ch + 0x34, leaf);
    }

    fn text(m: &mut FakeGuest, at: u32, string: &str) {
        m.put_u32(at + 0x74, LONG_TEXT);
        for (i, unit) in string.encode_utf16().chain(Some(0)).enumerate() {
            m.put_u16(at + 0xb8 + 2 * i as u32, unit);
        }
    }

    fn list(m: &mut FakeGuest, parent: u32) -> DatumArray {
        widget(m, LIST_AT, LIST, parent);
        widget(m, ITEM_AT, ITEM, LIST_AT);
        widget(m, LABEL_AT, TEXT, ITEM_AT);
        m.put_u32(LIST_AT, RECENT_PLAYERS)
            .put_u32(LIST_AT + 0x70, ARRAY)
            .map(ARRAY, 0x1000)
            .put(ARRAY + 0x800, b"audit\0");
        let array = DatumArray::at(ARRAY);
        array
            .initialize_contiguous(m, ARRAY + 0x800, 2, 0x5c, 2, 0)
            .unwrap();
        array.make_valid(m).unwrap();
        let row = array.new_datum(m).unwrap().unwrap();
        let data = array.try_get(m, row).unwrap().unwrap();
        m.put(data + 4, &live_proto::bots::local_xuid(3).0.to_le_bytes());
        m.put_u32(ITEM_AT + 0x70, row.0);
        text(m, LABEL_AT, "Example");
        array
    }

    fn fixture() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.map(0x0054_d000, 0x1000);
        widget(&mut m, SCREEN, 0, 0);
        m.put_u16(SCREEN + 0x70, 91);
        list(&mut m, SCREEN);
        channel(&mut m, 5, ui_watch::SHARED, SCREEN, ITEM_AT);
        m
    }

    fn flags(audit: &Value) -> Vec<&str> {
        audit["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["kind"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn audit_reads_bound_rows_and_keeps_the_menu_under_a_dialog() {
        let mut m = fixture();
        let dialog = 0x7000;
        widget(&mut m, dialog, 0, 0);
        widget(&mut m, 0x8000, BUTTON, dialog);
        m.put_u16(dialog + 0x70, 0x21).put_u32(0x8070, SHORT_TEXT);
        channel(&mut m, 3, 0, dialog, 0x8000);
        // A shared active menu would consume player one's input first.
        channel(&mut m, 5, ui_watch::SHARED, 0, 0);
        channel(&mut m, 5, 0, SCREEN, ITEM_AT);

        let audit = inspect(&m);
        assert_eq!(audit["screen"]["id"], 91);
        assert_eq!(audit["active"]["id"], 0x21);
        assert_eq!(audit["dialogs"][0]["name"], "confirmation_dialog");
        assert!(flags(&audit).contains(&"empty_label"));
        let row = &audit["screen"]["panes"][0]["lists"][0]["rows"][0];
        assert_eq!(row["text"], "Example");
        assert_eq!(row["clanker"], true);
        assert_eq!(row["index"], 0);

        // A dialog aimed exclusively at another controller remains visible
        // in the report. It cannot grant a controller-zero activation.
        channel(&mut m, 3, 0, 0, 0);
        channel(&mut m, 3, 1, dialog, 0x8000);
        assert_eq!(inspect(&m)["active"]["id"], 91);
    }

    #[test]
    fn a_transition_on_the_shared_channel_blocks_the_personal_receiver() {
        let mut m = fixture();
        let dialog = 0x7000;
        widget(&mut m, dialog, 0, 0);
        channel(&mut m, 3, 0, dialog, dialog);
        m.put_u8(SCREEN + 0x42, 1);
        assert!(target(&m).is_none());

        // A settled screen aimed solely at another controller declines it.
        m.put_u8(SCREEN + 0x42, 0);
        for w in [SCREEN, LIST_AT, ITEM_AT] {
            m.put_u16(w + 8, 2);
        }
        assert_eq!(target(&m).unwrap().screen, dialog);
    }

    #[test]
    fn distinguishes_empty_missing_and_unreadable_resolved_text() {
        let mut m = fixture();
        text(&mut m, LABEL_AT, "  ");
        let empty = inspect(&m);
        assert!(flags(&empty).contains(&"empty_row_text"));
        assert_eq!(
            empty["active"]["panes"][0]["lists"][0]["rows"][0]["text"],
            ""
        );

        m.put_u32(LABEL_AT + 0x74, 123);
        assert!(flags(&inspect(&m)).contains(&"unreadable_row_text"));
        m.put_u32(ITEM_AT + 0x14, 0);
        assert!(flags(&inspect(&m)).contains(&"missing_row_text"));
        m.put_u32(LIST_AT + 0x14, 0);
        assert!(flags(&inspect(&m)).contains(&"fewer_rows_than_items"));
    }

    #[test]
    fn scrolling_empty_lists_and_hidden_labels_are_not_missing_text() {
        let mut m = fixture();
        let array = DatumArray::at(ARRAY);
        array.new_datum(&mut m).unwrap().unwrap(); // Two items, one scrolling slot.
        widget(&mut m, 0x8000, TEXT, SCREEN);
        text(&mut m, 0x8000, "");
        m.put_u8(0x806e, 0);
        let audit = inspect(&m);
        assert_eq!(audit["active"]["panes"][0]["lists"][0]["item_count"], 2);
        assert!(flags(&audit).is_empty());
        array.delete_all(&mut m).unwrap();
        m.put_u32(ITEM_AT + 0x70, u32::MAX);
        assert!(flags(&inspect(&m)).is_empty());
        assert!(target(&m).unwrap().focused_row.is_none());
    }

    // A synthetic wgtz referencing one wgit with one two-row list.
    fn viewport(m: &mut FakeGuest) {
        let (scnr, globals, interface, entries) = (0x9000, 0xa000, 0xb000, 0xc000);
        let (wgtz, wgit, refs, pane, list) = (0xd000, 0xe000, 0xf000, 0x10000, 0x11000);
        m.put_u32(points::SCENARIO.base, scnr)
            .put_u16(scnr + 0x10, 2)
            .put_u32(points::GLOBALS_TAG.base, globals)
            .put_u32(globals + 0x110, 1)
            .put_u32(globals + 0x114, interface)
            .put_u32(interface + 0x84, 1)
            .put_u32(points::TAG_ENTRIES.base, entries)
            .put_u32(entries + 0x18, wgtz)
            .put_u32(entries + 0x28, wgit)
            .put_u32(wgtz + 8, 1)
            .put_u32(wgtz + 12, refs)
            .put_u32(refs + 4, 2)
            .put_u16(wgit + 4, 91)
            .put_u32(wgit + 0x20, 1)
            .put_u32(wgit + 0x24, pane)
            .put_u32(pane + 12, 1)
            .put_u32(pane + 16, list)
            .put_u16(list + 6, 2);
    }

    #[test]
    fn list_viewport_and_partial_dialog_survive_an_intact_header() {
        let mut m = fixture();
        viewport(&mut m);
        DatumArray::at(ARRAY).new_datum(&mut m).unwrap().unwrap();
        let audit = inspect(&m);
        assert_eq!(
            audit["active"]["panes"][0]["lists"][0]["visible_capacity"],
            2
        );
        assert!(flags(&audit).contains(&"fewer_rows_than_items"));
        m.put_u32(SCREEN + 0x14, 0);
        widget(&mut m, 0x7000, TEXT, SCREEN);
        text(&mut m, 0x7000, "Example title");
        let audit = inspect(&m);
        assert!(flags(&audit).contains(&"missing_list"));
        assert_eq!(
            audit["screen"]["panes"][0]["labels"][0]["text"],
            "Example title"
        );
    }

    #[test]
    fn an_unbound_or_changing_target_never_authorizes_a_row() {
        let mut m = fixture();
        assert_eq!(target(&m).unwrap().focused_row, Some(0));
        m.put_u32(ITEM_AT + 0x70, 0x1234_0000);
        assert!(target(&m).unwrap().focused_row.is_none());
        m.put_u8(SCREEN + 0x42, 2);
        assert!(target(&m).is_none());
        m.put_u8(SCREEN + 0x42, 0).put_u32(SCREEN + 0x14, SCREEN);
        assert!(flags(&inspect(&m)).contains(&"unreadable_widgets"));
    }

    #[test]
    fn a_profile_entry_cannot_turn_into_sign_in_or_create_between_audit_and_press() {
        let mut m = fixture();
        m.put_u16(SCREEN + 0x70, 193)
            .put_u32(SCREEN, 0x0045_b0b8)
            .put_u32(LIST_AT, 0x0045_af88)
            .put_u8(LIST_AT + 0x89, 1);
        let audit = inspect(&m);
        assert_eq!(
            audit["active"]["panes"][0]["lists"][0]["rows"][0]["navigation"],
            "inspect_profile"
        );
        let before = target(&m).unwrap().token;
        m.put_u8(LIST_AT + 0x89, 0);
        assert_ne!(target(&m).unwrap().token, before);
        m.put_u8(LIST_AT + 0x89, 1);
        let h = DatumHandle(m.u32(ITEM_AT + 0x70).unwrap());
        let datum = DatumArray::at(ARRAY).try_get(&m, h).unwrap().unwrap();
        m.put_u32(datum + 4, u32::MAX);
        assert_ne!(target(&m).unwrap().token, before);
        m.put_u16(SCREEN + 0x70, 194)
            .put_u32(SCREEN, 0x0045_c3f8)
            .put_u32(LIST_AT, 0x0045_c468)
            .put_u32(LIST_AT + 0x88, 0);
        let audit = inspect(&m);
        assert_eq!(
            audit["active"]["panes"][0]["lists"][0]["rows"][0]["navigation"],
            "edit_profile"
        );
        let before = target(&m).unwrap().token;
        m.put_u32(LIST_AT + 0x88, 1);
        assert_ne!(target(&m).unwrap().token, before);
    }

    #[test]
    fn replacing_the_focused_datum_invalidates_the_press_token() {
        let mut m = fixture();
        let before = target(&m).unwrap();
        let array = DatumArray::at(ARRAY);
        let old = DatumHandle(m.u32(ITEM_AT + 0x70).unwrap());
        array.delete(&mut m, old).unwrap();
        let new = array
            .new_datum_at_absolute(&mut m, i32::from(old.index()))
            .unwrap()
            .unwrap();
        m.put_u32(ITEM_AT + 0x70, new.0);
        let after = target(&m).unwrap();
        assert_eq!(before.focused_row, after.focused_row);
        assert_ne!(before.token, after.token);
    }

    #[test]
    fn reports_all_social_panes_and_follows_the_selected_child() {
        let mut m = FakeGuest::new();
        m.map(0x0054_d000, 0x1000);
        widget(&mut m, SCREEN, 0, 0);
        let (tabs, first, recent) = (0x7000, 0x8000, 0x9000);
        widget(&mut m, tabs, TABS, SCREEN);
        widget(&mut m, first, 0, tabs);
        widget(&mut m, recent, 0, tabs);
        m.put_u32(tabs + 0x70, recent).put_u16(recent + 0x5f0, 2);
        list(&mut m, recent);
        channel(&mut m, 3, ui_watch::SHARED, SCREEN, ITEM_AT);
        let audit = inspect(&m);
        assert_eq!(audit["active"]["pane"], 2);
        assert_eq!(audit["active"]["panes"][0]["active"], false);
        assert_eq!(
            audit["active"]["panes"][1]["lists"][0]["kind"],
            "recent_players"
        );
        assert_eq!(audit["focused_row"], 0);
        assert!(flags(&audit).is_empty(), "{audit}");
        m.put_u32(tabs + 0x70, first);
        assert!(target(&m).is_none(), "stale focus in a different tab");
    }
}
