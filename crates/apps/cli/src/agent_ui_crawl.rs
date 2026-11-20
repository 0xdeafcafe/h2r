//! A bounded walk over menu inputs whose handlers are known to open a view.
//! The server validates the instance and focused row again when it queues A.
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use agent_proto::{Client, method, sim::menu};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

struct Options {
    depth: usize,
    budget: Duration,
    require_clanker: bool,
    from_current: bool,
}

impl Options {
    fn parse(words: &[String]) -> Result<Self> {
        let mut o = Self {
            depth: 6,
            budget: Duration::from_secs(120),
            require_clanker: false,
            from_current: false,
        };
        let mut it = words.iter();
        while let Some(word) = it.next() {
            match word.as_str() {
                "--depth" => {
                    o.depth = it.next().context("--depth needs a number")?.parse()?;
                    ensure!(o.depth <= 32, "--depth must be at most 32");
                }
                "--budget-ms" => {
                    let ms = it
                        .next()
                        .context("--budget-ms needs milliseconds")?
                        .parse()?;
                    ensure!((1..=600_000).contains(&ms), "--budget-ms must be 1..600000");
                    o.budget = Duration::from_millis(ms);
                }
                "--from-current" => o.from_current = true,
                "--require-clanker" => o.require_clanker = true,
                _ => bail!("unknown ui-crawl option {word}"),
            }
        }
        Ok(o)
    }
}

#[derive(Deserialize)]
struct Audit {
    active: Surface,
    target: String,
    focused_row: Option<u32>,
    issues: Vec<Value>,
}

#[derive(Deserialize)]
struct Surface {
    id: u32,
    address: u32,
    vtable: Option<u32>,
    window: u32,
    slot: u32,
    kit: bool,
    name: Option<String>,
    mode: Option<u32>,
    pane: Option<u32>,
    panes: Vec<Pane>,
}

#[derive(Deserialize)]
struct Pane {
    index: Option<u32>,
    active: bool,
    lists: Vec<List>,
}

#[derive(Deserialize)]
struct List {
    vtable: Option<u32>,
    kind: Option<String>,
    focused_row: Option<u32>,
    item_count: Option<u32>,
    rows: Vec<Row>,
}

#[derive(Deserialize)]
struct Row {
    index: Option<u32>,
    text: Option<String>,
    clanker: Option<bool>,
    navigation: Option<String>,
}

impl Audit {
    fn list(&self) -> Option<&List> {
        let pane = self
            .active
            .panes
            .iter()
            .find(|p| p.active && p.index == self.active.pane)?;
        let lists = &pane.lists;
        lists
            .iter()
            .find(|l| l.focused_row.is_some())
            .or_else(|| (lists.len() == 1).then(|| &lists[0]))
    }

    fn row(&self, index: u32) -> Option<&Row> {
        self.list()?.rows.iter().find(|r| r.index == Some(index))
    }

    fn social(&self) -> bool {
        self.active.vtable == Some(0x458e58) && matches!(self.active.id, 0x1a | 0x1c)
    }

    fn recent(&self) -> bool {
        self.list().is_some_and(|l| {
            l.kind.as_deref() == Some("recent_players") && l.vtable == Some(0x45b4a0)
        })
    }

    // Pane numbers on ordinary retail screens change with row focus (the
    // preview to the right). The Y menu's pane identifies a different list.
    fn place(&self) -> Value {
        let s = &self.active;
        let tab = self.social().then_some(s.pane).flatten();
        json!({"id":s.id, "name":s.name, "kit":s.kit, "window":s.window,
            "slot":s.slot, "mode":s.mode, "tab":tab, "list":self.list().and_then(|l| l.vtable)})
    }

    fn same_page(&self, other: &Audit) -> bool {
        self.active.address == other.active.address && self.place() == other.place()
    }

    fn card(&self) -> bool {
        self.active.kit && self.active.name.as_deref() == Some("clanker_player_options")
    }

    // This predicate is checked again on the focused row immediately before
    // choose. Only the known fixed Live menu uses a displayed label as a key.
    // A configurable gamertag, file name or confirmation button can never
    // grant permission to activate its row.
    fn can_open(&self, row: &Row, campaign_picker: bool, campaign_actions: &[Value]) -> bool {
        let Some(index) = row.index else { return false };
        if self.recent() {
            return true;
        }
        let s = &self.active;
        if s.kit {
            return match s.name.as_deref() {
                // A on a film or catalog entry opens its detail. Pack row 0 downloads.
                Some("campaigns" | "mods" | "theater.films") => true,
                Some("mods.pack") => index > 0,
                _ => false,
            };
        }
        match s.id {
            menu::MAIN_MENU => (1..=4).contains(&index) || index == 0 && campaign_picker,
            // Edit Profile consists of settings submenus. Controls rows 0..5
            // open their retail pickers; the extensions after them toggle values.
            38 => self.list().and_then(|l| l.vtable) == Some(0x45bea0) && index < 7,
            39 => self.list().and_then(|l| l.vtable) == Some(0x45d240) && index < 6,
            0xda => matches!(
                campaign_actions.get(index as usize).and_then(Value::as_str),
                Some("new_game" | "select_level")
            ),
            // ui.audit binds each key into the focused target. Profile-list
            // context and the selected datum are rechecked on delivery.
            193 => {
                s.vtable == Some(0x45b0b8)
                    && self.list().and_then(|l| l.vtable) == Some(0x45af88)
                    && row.navigation.as_deref() == Some("inspect_profile")
            }
            194 => {
                s.vtable == Some(0x45c3f8)
                    && self.list().and_then(|l| l.vtable) == Some(0x45c468)
                    && index == 0
                    && row.navigation.as_deref() == Some("edit_profile")
            }
            19 => self.list().and_then(|l| l.vtable) == Some(0x45b060) && index < 3,
            menu::LIVE_MENU => {
                self.list().and_then(|l| l.vtable) == Some(0x458b48)
                    && matches!(
                        row.text.as_deref(),
                        Some("Custom Game Browser" | "Content Download" | "THEATER" | "Mods")
                    )
            }
            _ => false,
        }
    }
}

// A modal may keep the backdrop blurred indefinitely. Only a transition in
// progress delays input. An overdue/stuck transition is itself evidence.
fn settling(v: &Value) -> bool {
    let t = &v["transition"];
    let layers = t["layers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if layers.iter().any(|l| l["overdue"] == true)
        || v["issues"].as_array().is_some_and(|issues| {
            issues
                .iter()
                .any(|i| i == "stuck_fade" || i["kind"] == "stuck_fade")
        })
    {
        return false;
    }
    t["startup_fade_remaining_ms"]
        .as_u64()
        .is_some_and(|n| n > 0)
        || layers.iter().any(|l| {
            (v["active"].is_null() || l["slot"] == v["active"]["slot"])
                && (l["role"] == "pending" || l["direction"].is_string())
        })
}

#[derive(Default)]
struct Report {
    visits: Vec<Value>,
    findings: Vec<Value>,
    skipped: Vec<Value>,
    clanker_cards: usize,
    recent_scanned: bool,
}

impl Report {
    fn observe(&mut self, path: &[String], a: &Audit) {
        let visit = json!({"path":path, "screen":a.place()});
        if !self.visits.contains(&visit) {
            self.visits.push(visit);
        }
        // This class has three information rows and Back. A valid empty
        // list is normal on a friends page, and a regression on this card.
        let observed = a.list().and_then(|l| l.item_count);
        let card_count = (a.card() && observed != Some(4))
            .then(|| json!({"kind":"clanker_card_row_count","expected":4,"observed":observed}));
        for issue in a.issues.iter().chain(card_count.as_ref()) {
            let finding = json!({"path":path, "screen":a.place(), "issue":issue});
            if !self.findings.contains(&finding) {
                self.findings.push(finding);
            }
        }
    }

    fn skip(&mut self, path: &[String], what: impl Into<String>) {
        self.skipped.push(json!({"path":path,"reason":what.into()}));
    }
}

type Call<'a> = dyn FnMut(&str, Value, Duration) -> Result<Value> + 'a;

struct Crawl<'a> {
    rpc: &'a mut Call<'a>,
    end: Instant,
    max_depth: usize,
    campaign_picker: bool,
    report: Report,
}

impl Crawl<'_> {
    fn call(&mut self, name: &str, params: Value) -> Result<Value> {
        let left = self.end.saturating_duration_since(Instant::now());
        ensure!(!left.is_zero(), "crawl time budget exhausted");
        (self.rpc)(name, params, left).with_context(|| format!("calling {name}"))
    }

    fn audit(&mut self, path: &[String]) -> Result<Audit> {
        loop {
            let v = self.call(method::UI_AUDIT, Value::Null)?;
            ensure!(
                v.pointer("/at/phase") == Some(&json!("menu")),
                "left the menu"
            );
            if !settling(&v) {
                if v["active"].is_null() || v["target"].is_null() {
                    if let Some(issues) = v["issues"].as_array() {
                        for issue in issues {
                            self.report
                                .findings
                                .push(json!({"path":path, "issue":issue}));
                        }
                    }
                    bail!(
                        "ui.audit has no readable input target (screen {}, dialogs {})",
                        v["screen"]["id"],
                        v["dialogs"].as_array().map_or(0, Vec::len)
                    );
                }
                return serde_json::from_value(v).context("invalid ui.audit screen or dialog");
            }
            self.ticks(3)?;
        }
    }

    fn campaign_actions(&mut self, a: &Audit) -> Result<Vec<Value>> {
        if self.campaign_picker && !a.active.kit && a.active.id == 0xda {
            let c = self.call(method::UI_CAMPAIGN, Value::Null)?;
            ensure!(c["page"] == "options", "campaign options page changed");
            return c["rows"]
                .as_array()
                .cloned()
                .context("campaign action keys unavailable");
        }
        Ok(Vec::new())
    }

    fn ticks(&mut self, ticks: u32) -> Result<()> {
        let ms = self
            .end
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(2000);
        self.call(
            method::WAIT,
            json!({"until":{"ticks":ticks},"timeout_ms":ms.max(1) as u64}),
        )?;
        Ok(())
    }

    fn press(
        &mut self,
        a: &Audit,
        action: &str,
        row: Option<u32>,
        path: &[String],
    ) -> Result<Audit> {
        let mut p = json!({"target":a.target,"action":action,"phase":"menu"});
        if let Some(r) = row {
            p["row"] = json!(r);
        }
        let ack = self.call(method::UI_PRESS, p)?;
        ensure!(
            ack["delivered"] == true,
            "ui.press did not confirm delivery"
        );
        // The acknowledged press includes its neutral release. Let the menu
        // dispatcher start its transition, then audit the settled screen.
        self.ticks(4)?;
        self.audit(path)
    }

    // The active count is a count of allocated datums. Their indices may
    // have holes, and a scrolling list only materializes its visible window.
    // Follow the actual cursor until every occupied row has been observed.
    fn rows(&mut self, mut at: Audit, path: &[String], lineage: &[Value]) -> Result<Audit> {
        let n = at
            .list()
            .and_then(|l| l.item_count)
            .context("expected a readable navigation list")?;
        let recent = at.recent();
        ensure!(n <= 4096 && (!recent || n <= 100), "invalid list size {n}");
        ensure!(
            n > 0 || at.active.id != menu::MAIN_MENU || at.active.kit,
            "main menu has no items"
        );
        let here = at.place();
        let mut visited = BTreeSet::new();
        let mut moved_from = BTreeSet::new();
        let mut direction = "down";
        while visited.len() < n as usize {
            ensure!(
                at.list().and_then(|l| l.item_count) == Some(n),
                "navigation list changed during crawl"
            );
            let index = at
                .focused_row
                .context("expected list has no readable focus")?;
            if visited.insert(index) {
                moved_from.clear();
                at = self.branch(at, path, Route::row(index), lineage)?;
                if at.place() != here || visited.len() == n as usize {
                    break;
                }
            }
            let from = at.focused_row.context("list lost focus on return")?;
            if !moved_from.insert(from) {
                ensure!(
                    direction != "up",
                    "list focus reached only {} of {n} rows",
                    visited.len()
                );
                moved_from.clear();
                direction = "up";
            }
            let next = self.press(&at, direction, Some(from), path)?;
            ensure!(at.same_page(&next), "menu changed while moving list focus");
            self.report.observe(path, &next);
            at = next;
        }
        if recent && n > 0 && visited.len() == n as usize {
            self.report.recent_scanned = true;
        }
        Ok(at)
    }

    // A branch always gives control back on the page that opened it. A
    // failed unwind stops the entire crawl before any sibling receives A.
    fn branch(
        &mut self,
        parent: Audit,
        path: &[String],
        route: Route,
        ancestors: &[Value],
    ) -> Result<Audit> {
        let mut expect_card = false;
        let mut read_only_profile_may_decline = false;
        if let Some(index) = route.row {
            ensure!(parent.focused_row == Some(index), "navigation row changed");
            let row = parent
                .row(index)
                .context("focused row missing from audit")?;
            let actions = self.campaign_actions(&parent)?;
            if !parent.can_open(row, self.campaign_picker, &actions) {
                self.report.skip(
                    path,
                    format!("row {index}: unverified or state-changing action"),
                );
                return Ok(parent);
            }
            expect_card = parent.recent() && row.clanker == Some(true);
            read_only_profile_may_decline = row.navigation.as_deref() == Some("edit_profile");
        }
        let mut next_path = path.to_vec();
        next_path.push(route.label.clone());
        let child = self.press(&parent, route.action, route.row, &next_path)?;
        if child.same_page(&parent) {
            if parent.recent()
                || route.action == "open_live_menu"
                || route.row.is_some()
                    && !read_only_profile_may_decline
                    && (!parent.active.kit || parent.active.name.as_deref() == Some("campaigns"))
            {
                bail!(
                    "{} did not open its expected screen at {}",
                    route.label,
                    path.join(" / ")
                );
            }
            self.report
                .skip(path, format!("{} did not change page", route.label));
            return Ok(child);
        }
        self.report.observe(&next_path, &child);
        if expect_card {
            if child.card() {
                self.report.clanker_cards += 1;
            } else {
                self.report
                    .findings
                    .push(json!({"path":next_path,"screen":child.place(),
                    "issue":"expected_clanker_player_options"}));
            }
        }
        let opened = child.place();
        // A profile action can dismiss itself and return directly to the
        // picker. The tab ring, on the other hand, has an explicit inverse.
        if route.row.is_some() && ancestors.contains(&opened) {
            return Ok(child);
        }
        if route.action == "right" {
            ensure!(
                child.social() && child.active.address == parent.active.address,
                "tab switch left the social menu"
            );
        }
        let child = if ancestors.contains(&opened) {
            child
        } else {
            self.visit(child, &next_path, ancestors)?
        };
        if child.place() != opened && ancestors.contains(&child.place()) {
            return Ok(child);
        }
        let back = self.press(&child, route.undo, None, path)?;
        ensure!(
            ancestors.contains(&back.place()),
            "return from {} reached {} instead of {}",
            next_path.join(" / "),
            back.place(),
            parent.place()
        );
        Ok(back)
    }

    fn visit(&mut self, mut at: Audit, path: &[String], lineage: &[Value]) -> Result<Audit> {
        self.report.observe(path, &at);
        if path.len() >= self.max_depth {
            self.report.skip(path, "depth limit");
            return Ok(at);
        }
        let mut lineage = lineage.to_vec();
        lineage.push(at.place());
        let s = &at.active;
        let mut edges = Vec::new();
        if !s.kit && matches!(s.id, menu::MAIN_MENU | menu::LIVE_MENU) {
            if self.report.recent_scanned {
                self.report
                    .skip(path, "the shared Recent Players list has been scanned");
            } else {
                edges.push(Route::button("friends", "back"));
            }
        }
        if !s.kit && s.id == menu::JOIN_SCREEN && s.mode == Some(menu::MODE_LIVE) {
            edges.push(Route::button("open_live_menu", "back"));
        }
        if at.social() {
            ensure!(
                s.pane != Some(2) || at.recent(),
                "Recent Players has no readable history list"
            );
            edges.push(Route::button("right", "left"));
        }
        let scan = at.recent()
            || match (s.kit, s.id, s.name.as_deref()) {
                (false, menu::MAIN_MENU | 19 | 38 | 39 | 193 | 194 | menu::LIVE_MENU, _) => true,
                (false, 0xda, _) => self.campaign_picker,
                (
                    true,
                    _,
                    Some(
                        "campaigns" | "mods" | "mods.pack" | "mods.detail" | "theater.films"
                        | "theater.film" | "video" | "voice",
                    ),
                ) => true,
                _ => false,
            };
        let here = at.place();
        for edge in edges {
            at = self.branch(at, path, edge, &lineage)?;
            if at.place() != here {
                self.report
                    .skip(path, "Back returned directly to an ancestor");
                return Ok(at);
            }
        }
        if scan {
            return self.rows(at, path, &lineage);
        }
        if let Some(n) = at.list().and_then(|l| l.item_count) {
            self.report.skip(
                path,
                format!("{n} items without a verified navigation handler"),
            );
        }
        Ok(at)
    }
}

struct Route {
    action: &'static str,
    undo: &'static str,
    row: Option<u32>,
    label: String,
}

impl Route {
    fn button(action: &'static str, undo: &'static str) -> Self {
        Self {
            action,
            undo,
            row: None,
            label: action.into(),
        }
    }

    fn row(index: u32) -> Self {
        Self {
            action: "choose",
            undo: "back",
            row: Some(index),
            label: format!("row {index}"),
        }
    }
}

pub fn run(client: &mut Client, args: &[String], timeout: Option<Duration>) -> Result<()> {
    let o = Options::parse(args)?;
    let mut rpc = |name: &str, p: Value, left: Duration| -> Result<Value> {
        client.set_timeout(Some(timeout.unwrap_or(left).min(left)))?;
        Ok(client.call(name, p)?)
    };
    let mut crawl = Crawl {
        rpc: &mut rpc,
        end: Instant::now() + o.budget,
        max_depth: o.depth,
        campaign_picker: false,
        report: Report::default(),
    };
    let result = (|| -> Result<()> {
        let methods = crawl.call(method::METHODS, Value::Null)?;
        let methods = methods
            .as_array()
            .context("invalid session.methods result")?;
        for name in [method::UI_AUDIT, method::UI_PRESS] {
            ensure!(
                methods.iter().any(|v| v == name),
                "running game does not register {name}"
            );
        }
        crawl.call(method::CLAIM, Value::Null)?;
        if methods.iter().any(|v| v == method::UI_CAMPAIGN) {
            crawl.campaign_picker =
                crawl.call(method::UI_CAMPAIGN, Value::Null)?["installed"] == true;
        }
        let root = crawl.audit(&[])?;
        ensure!(
            o.from_current || (!root.active.kit && root.active.id == menu::MAIN_MENU),
            "open the main menu or use --from-current to audit this subtree"
        );
        crawl.visit(root, &[], &[])?;
        ensure!(
            !o.require_clanker || crawl.report.clanker_cards > 0,
            "no Recent Players entry opened a Clanker card"
        );
        Ok(())
    })();
    // This command owns the connection. Returning drops it and releases
    // the claim even after a timeout, without issuing a late input.
    let r = &crawl.report;
    let pass = result.is_ok() && r.findings.is_empty();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"pass":pass, "visited":r.visits,
        "findings":r.findings, "skipped":r.skipped, "clanker_cards":r.clanker_cards,
        "error":result.err().map(|e| format!("{e:#}"))}))?
    );
    ensure!(pass, "ui-crawl did not pass");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_budgets() {
        let opts =
            |s: &str| Options::parse(&s.split_whitespace().map(str::to_owned).collect::<Vec<_>>());
        assert!(opts("--depth 0 --budget-ms 1").is_ok());
        assert!(opts("--depth 33").is_err());
        assert!(opts("--budget-ms 0").is_err());
        assert!(opts("--budget-ms").is_err());
        assert!(opts("--typo").is_err());
    }

    #[test]
    fn ordinary_modal_blur_is_settled_but_an_incoming_layer_is_not() {
        let mut v = json!({"active":{"slot":4}, "issues":[], "transition":{"dialog_open":true,
            "layers":[{"slot":4,"role":"active","direction":null,"overdue":false}]}});
        assert!(!settling(&v));
        v["transition"]["layers"][0]["direction"] = json!("in");
        assert!(settling(&v));
        v["transition"]["layers"][0]["overdue"] = json!(true);
        assert!(!settling(&v));
    }

    #[derive(Copy, Clone, Debug, PartialEq)]
    enum Page {
        Main,
        Tab(u32),
        Card,
    }

    struct Game {
        page: Page,
        focus: u32,
        opened: Vec<u32>,
    }

    impl Game {
        fn audit(&self) -> Value {
            let (id, name, tab, vtable, kind, rows) = match self.page {
                Page::Main => (9, None, 0, 100, None, vec!["Game"]),
                Page::Tab(t) => {
                    let (vt, kind, names) = if t == 2 {
                        (0x45b4a0, Some("recent_players"), vec!["Woosh", "Sprocket"])
                    } else {
                        (120 + t, None, vec!["Invite"])
                    };
                    (0x1a, None, t, vt, kind, names)
                }
                Page::Card if self.focus == 4 => (
                    240,
                    Some("clanker_player_options"),
                    0,
                    200,
                    None,
                    vec!["Woosh", "One", "Calm", "Close"],
                ),
                Page::Card => (31, None, 0, 111, None, vec!["", ""]),
            };
            let focus = if self.page == Page::Tab(2) {
                self.focus
            } else {
                0
            };
            let rows: Vec<_> = rows
                .into_iter()
                .enumerate()
                .map(|(i, s)| json!({"index":if kind.is_some() { [4,9][i] } else { i }, "text":s,"clanker":kind.is_some()}))
                .collect();
            let issues = if id == 31 {
                json!([{"kind":"empty_row","row":0}])
            } else {
                json!([])
            };
            json!({"at":{"phase":"menu"}, "active":{"id":id,"name":name,"kit":name.is_some(),
                "mode":null, "pane":tab, "address":id+1234,"slot":4,"window":5,
                "vtable":if id == 0x1a { 0x458e58 } else { 70 },
                "panes":[{"index":tab,"active":true,"lists":[{"item_count":rows.len(),"vtable":vtable,"kind":kind,
                    "rows":rows,"focused_row":focus}]}]},
                "issues":issues,"target":format!("{id}/{tab}"),"focused_row":focus})
        }

        fn call(&mut self, m: &str, p: Value, _: Duration) -> Result<Value> {
            match m {
                method::UI_AUDIT => Ok(self.audit()),
                method::WAIT => Ok(Value::Null),
                method::UI_PRESS => {
                    let before = self.audit();
                    assert_eq!(p["phase"], "menu");
                    assert_eq!(p["target"], before["target"]);
                    match (self.page, p["action"].as_str()) {
                        (Page::Main, Some("friends")) => self.page = Page::Tab(0),
                        (Page::Tab(t), Some("right")) => self.page = Page::Tab((t + 1) % 3),
                        (Page::Tab(t), Some("left")) => self.page = Page::Tab((t + 2) % 3),
                        (Page::Tab(2), Some("down")) => self.focus = 9,
                        (Page::Tab(2), Some("choose")) => {
                            assert_eq!(p["row"], self.focus);
                            self.opened.push(self.focus);
                            self.page = Page::Card;
                        }
                        (Page::Card, Some("back")) => self.page = Page::Tab(2),
                        (Page::Tab(0), Some("back")) => self.page = Page::Main,
                        other => panic!("unexpected menu action {other:?}"),
                    }
                    Ok(json!({"delivered":true}))
                }
                other => panic!("unexpected method {other}"),
            }
        }
    }

    #[test]
    fn profile_picker_requires_the_bound_nonmutating_action() {
        let mut raw = Game {
            page: Page::Main,
            focus: 0,
            opened: vec![],
        }
        .audit();
        for (id, class, list, action) in [
            (193, 0x45b0b8, 0x45af88, "inspect_profile"),
            (194, 0x45c3f8, 0x45c468, "edit_profile"),
        ] {
            raw["active"]["id"] = json!(id);
            raw["active"]["vtable"] = json!(class);
            let l = &mut raw["active"]["panes"][0]["lists"][0];
            l["vtable"] = json!(list);
            let row = &mut l["rows"][0];
            row["navigation"] = Value::Null;
            row["text"] = json!(action);
            let page: Audit = serde_json::from_value(raw.clone()).unwrap();
            assert!(!page.can_open(page.row(0).unwrap(), false, &[]));
            raw["active"]["panes"][0]["lists"][0]["rows"][0]["navigation"] = json!(action);
            let page: Audit = serde_json::from_value(raw.clone()).unwrap();
            assert!(page.can_open(page.row(0).unwrap(), false, &[]));
        }
    }

    #[test]
    fn scrolling_a_settings_leaf_checks_unmaterialized_rows_without_activating_them() {
        let page = |row: u32| {
            let mut v = Game {
                page: Page::Main,
                focus: row,
                opened: vec![],
            }
            .audit();
            v["active"]["name"] = json!("video");
            v["active"]["id"] = json!(39);
            v["active"]["kit"] = json!(true);
            v["focused_row"] = json!(row);
            let list = &mut v["active"]["panes"][0]["lists"][0];
            list["item_count"] = json!(2);
            list["focused_row"] = json!(row);
            list["rows"] = json!([{"index":row,"text":"untrusted content"}]);
            if row == 7 {
                v["issues"] = json!([{"kind":"empty_row_text","row":7}]);
            }
            v
        };
        let mut presses = 0;
        let mut rpc = |m: &str, p: Value, _: Duration| -> Result<Value> {
            match m {
                method::UI_PRESS => {
                    assert_eq!(p["action"], "down");
                    assert_eq!(p["row"], 1);
                    presses += 1;
                    Ok(json!({"delivered":true}))
                }
                method::WAIT => Ok(Value::Null),
                method::UI_AUDIT => Ok(page(7)),
                other => panic!("unexpected method {other}"),
            }
        };
        let mut crawl = Crawl {
            rpc: &mut rpc,
            end: Instant::now() + Duration::from_secs(5),
            max_depth: 2,
            campaign_picker: false,
            report: Report::default(),
        };
        let result = crawl
            .visit(serde_json::from_value(page(1)).unwrap(), &[], &[])
            .unwrap();
        assert_eq!(result.focused_row, Some(7));
        assert_eq!(crawl.report.findings[0]["issue"]["row"], 7);
        assert_eq!(presses, 1);
    }

    #[test]
    fn opens_each_recent_card_and_reports_wrong_class_and_missing_text() {
        let mut game = Game {
            page: Page::Main,
            focus: 4,
            opened: Vec::new(),
        };
        let root = serde_json::from_value(game.audit()).unwrap();
        let mut rpc = |m: &str, p: Value, left| game.call(m, p, left);
        let mut crawl = Crawl {
            rpc: &mut rpc,
            end: Instant::now() + Duration::from_secs(5),
            max_depth: 6,
            campaign_picker: false,
            report: Report::default(),
        };
        let back = crawl.visit(root, &[], &[]).unwrap();
        assert_eq!(back.active.id, 9);
        assert_eq!(crawl.report.clanker_cards, 1);
        let issues = &crawl.report.findings;
        assert!(
            issues
                .iter()
                .any(|f| f["issue"] == "expected_clanker_player_options")
        );
        assert!(issues.iter().any(|f| f["issue"]["kind"] == "empty_row"));
        assert_eq!(game.opened, [4, 9]);
    }
}
