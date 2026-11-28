//! The lab's questions to the player. Only a player choosing Close and
//! start (`Decision::Allowed`) or Start alongside (`Decision::Alongside`,
//! offered unless the job needs a quiet machine) can grant consent to start
//! a job; the lab then closes this game with `host.quit` for the first and
//! leaves it open for the second. While a job runs, a game the
//! player opens asks what the job should do (`Ask::Running`,
//! docs/runtime/lab.md "When the player opens a game").

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use agent::Access;
use agent_proto::takeover::{self, Ask, Decision, Operation, Request, Source};
use agent_proto::{RpcError, code};
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use runtime::guest::Permissions;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::layout_glue::ReadGm;
use crate::list_class::{self, ListClass, ListLayout, Panel, PanelText, Rows, TextBlock};
use crate::ui::{Chosen, LABEL_SLOT};
use crate::ui_kit::dialog::{self, DialogBase, DialogGeometry, DialogSpec};
use crate::ui_kit::input::{Glyph, Input, Prompt};
use crate::ui_kit::page::ListPage;
use crate::ui_kit::screen::SlotTexts;
use crate::ui_kit::spec::{ListPlace, Press, RowSpec, ScreenSpec};
use crate::ui_kit::view::Opener;
use crate::{coop_probe, level_boot, ui_kit, ui_watch};

pub const SCREEN: &str = "lab.takeover";
/// The start question with a Start alongside row.
pub const ALONGSIDE_SCREEN: &str = "lab.takeover.alongside";
/// The question a game asks while a lab job runs.
pub const RUNNING_SCREEN: &str = "lab.running";
const IDS: ui_kit::ids::Family = ui_kit::ids::family("lab takeover");
const TITLE: &str = "LAB REQUEST";
/// Ids both questions share: the title, the body and the footer.
const TITLE_ID: u32 = 0;
const BODY_ID: u32 = 1;
const FOOTER_ID: u32 = 4;

/// One of the three dialogs: its screen and class, its rows and what each
/// answers. The safe answer is row 0, the initial focus, including when a
/// button is held.
struct Form {
    screen: &'static str,
    class: &'static str,
    labels: &'static [&'static str],
    /// Row `n`'s label is [`IDS`] id `first_id + n`.
    first_id: u32,
    answers: &'static [Decision],
    /// What closing the dialog without a choice answers.
    dismissed: Decision,
    /// The list's anchor height, which puts the rows under the body.
    anchor: i16,
}

// The dialog's list is on skin 0 (`ui_kit::view::Geometry::fallback`):
// a row's cap (item art 1, the focus frame) is 34 high from 1 below the
// row's anchor, so it spans anchor + 33 down to anchor - 1, and rows are
// 22 apart. The body's ink ends 19 units a line below `dialog::BODY_TOP`
// (72), and the footer's top is -83. The layout check holds the body off
// every row's art, and every label to a contrast of 2.5 against what is
// drawn under it, the main menu included.
//
// Both questions have a three-line body, ending at 72 - 3 * 19 = 15. A
// fourth line would end at -4 and need row 0's cap top at -4 or lower, an
// anchor of -37 or lower; on the disc these labels lose their contrast
// from about -36 down (over the menu under the panel), so four lines fit
// no anchor. At -34 the cap's top (-1) is 16 under the body, the labels
// keep their contrast (measured at this anchor), and row 1's cap ends at
// -57, well above the footer.
const START: Form = Form {
    screen: SCREEN,
    class: "lab-takeover-dialog",
    labels: &["Not now", "Close and start"],
    first_id: 2,
    answers: &[Decision::Declined, Decision::Allowed],
    dismissed: Decision::Declined,
    anchor: -34,
};

// The start question with Start alongside has three rows under the same
// three-line body, so it takes the running question's anchor, -35.
const ALONGSIDE: Form = Form {
    screen: ALONGSIDE_SCREEN,
    class: "lab-takeover-alongside-dialog",
    labels: &["Not now", "Start alongside", "Close and start"],
    first_id: 8,
    answers: &[Decision::Declined, Decision::Alongside, Decision::Allowed],
    dismissed: Decision::Declined,
    anchor: -35,
};

// At -35 row 0's cap tops out at -2, 17 under the body, the labels keep
// their contrast, and row 2's cap ends at -80, above the footer's -83.
const RUNNING: Form = Form {
    screen: RUNNING_SCREEN,
    class: "lab-running-dialog",
    labels: &["Finish this one", "Keep running", "Stop and pause"],
    first_id: 5,
    answers: &[
        Decision::FinishThis,
        Decision::KeepRunning,
        Decision::StopAndPause,
    ],
    dismissed: Decision::FinishThis,
    anchor: -35,
};

/// The dialog that asks `request`: the start question has the Start
/// alongside row only when the request offers it.
fn form(request: &Request) -> &'static Form {
    match (request.ask, request.alongside) {
        (Ask::Start, false) => &START,
        (Ask::Start, true) => &ALONGSIDE,
        (Ask::Running, _) => &RUNNING,
    }
}

fn prompts() -> [Prompt; 2] {
    [
        Prompt::new(&[Glyph::A], "Select"),
        Prompt::new(&[Glyph::B], "Back"),
    ]
}

fn layout(form: &Form) -> ListLayout {
    let mut l = ui_kit::dialog_tag::base_layout(DialogBase::Large, form.labels.len());
    l.anchor[1] = form.anchor;
    l
}

fn display_field(value: &str, max: usize) -> String {
    let clean: String = value
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_control() && c != '|' {
                c
            } else {
                '?'
            }
        })
        .take(max + 1)
        .collect();
    if clean.len() > max {
        format!("{}...", &clean[..max - 3])
    } else {
        clean
    }
}

fn source_line(source: &Source) -> String {
    let short = |commit: &str| display_field(&commit.chars().take(7).collect::<String>(), 7);
    match source {
        Source::Branch { name, commit } => {
            format!("lab/{} at {}", display_field(name, 12), short(commit))
        }
        Source::Rev { rev, commit } => {
            format!("{} at {}", display_field(rev, 12), short(commit))
        }
        Source::Current { commit: Some(c) } => format!("installed image at {}", short(c)),
        Source::Current { commit: None } => "installed image".into(),
    }
}

/// The dialog's text, three lines (all the room above the rows, see
/// [`START`]). To start a job: who asks, what it does and how long it
/// takes; the job's name; its source, then the level and data. While a
/// job runs: who runs it, its name and the question. Each line stays one
/// line (the fit test), since the body wraps only at spaces.
fn body(request: &Request) -> String {
    let who = display_field(&request.requester, 12);
    let name = display_field(&request.name, 22);
    if request.ask == Ask::Running {
        return format!("{who} is running\n\"{name}\"\nwhile you play. What now?");
    }
    let action = match (request.operation, request.builds_image) {
        (Operation::Build, _) => "build",
        (Operation::Run, false) => "run",
        (Operation::Run, true) => "build, then run",
        (Operation::Sim, false) => "run a sim",
        (Operation::Sim, true) => "build, then sim",
    };
    let time: String = match request.estimate_seconds {
        0 => "time varies".into(),
        n @ 1..=59 => format!("about {n} sec"),
        n @ 60..=5940 => format!("about {} min", n.div_ceil(60)),
        _ => "over 99 min".into(),
    };
    let mut facts = vec![source_line(&request.source)];
    if let Some(level) = request.level.as_deref().filter(|l| !l.trim().is_empty()) {
        facts.push(format!("level {}", display_field(level, 10)));
    }
    if request.own_data {
        facts.push("own data".into());
    }
    format!("{who}: {action} ({time})\n\"{name}\"\n{}", facts.join(", "))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Queued,
    Opening,
    Open(u32),
    Closing(u32),
    Gone,
}

struct PromptState {
    id: u64,
    form: &'static Form,
    until: Instant,
    body: String,
    screen: Screen,
    decision: Decision,
    pause: Option<CampaignPause>,
}

impl PromptState {
    fn expire(&mut self, now: Instant) {
        if self.decision == Decision::Pending && now >= self.until {
            self.decision = Decision::TimedOut;
        }
    }

    fn finished(&self) -> bool {
        self.decision != Decision::Pending && matches!(self.screen, Screen::Queued | Screen::Gone)
    }
}

struct Dialog {
    texts: SlotTexts,
    prompt: Mutex<Option<PromptState>>,
}

impl Default for Dialog {
    fn default() -> Self {
        let texts = SlotTexts::new(IDS);
        texts.set(TITLE_ID, TITLE);
        for form in [&START, &ALONGSIDE, &RUNNING] {
            for (row, label) in form.labels.iter().enumerate() {
                texts.set(form.first_id + row as u32, label);
            }
        }
        texts.set(FOOTER_ID, &ui_kit::input::legend(&prompts()));
        Self {
            texts,
            prompt: Mutex::default(),
        }
    }
}

impl Dialog {
    fn prompt(&self) -> MutexGuard<'_, Option<PromptState>> {
        self.prompt.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn submit(&self, request: Request, now: Instant) -> Result<u64, RpcError> {
        let source_empty = match &request.source {
            Source::Branch { name, commit } => name.trim().is_empty() || commit.trim().is_empty(),
            Source::Rev { rev, commit } => rev.trim().is_empty() || commit.trim().is_empty(),
            Source::Current { commit } => commit.as_deref().is_some_and(|c| c.trim().is_empty()),
        };
        if request.requester.trim().is_empty()
            || request.name.trim().is_empty()
            || source_empty
            || !(1..=takeover::MAX_TIMEOUT_SECONDS).contains(&request.timeout_seconds)
        {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "expected a requester, a job name, a source and a timeout from 1 to 3600 seconds",
            ));
        }
        let mut prompt = self.prompt();
        if let Some(p) = prompt.as_mut() {
            p.expire(now);
            if !p.finished() {
                return Err(RpcError::new(code::BUSY, "another lab request is open"));
            }
        }
        let id = prompt.as_ref().map_or(1, |p| p.id + 1);
        *prompt = Some(PromptState {
            id,
            form: form(&request),
            until: now + Duration::from_secs(request.timeout_seconds.into()),
            body: body(&request),
            screen: Screen::Queued,
            decision: Decision::Pending,
            pause: None,
        });
        Ok(id)
    }

    fn status(&self, id: u64, now: Instant) -> Result<Decision, RpcError> {
        let mut prompt = self.prompt();
        let p = prompt
            .as_mut()
            .filter(|p| p.id == id)
            .ok_or_else(|| RpcError::new(code::GONE, "lab request not found"))?;
        p.expire(now);
        Ok(p.decision)
    }

    fn close(&self, screen: u32) -> Option<CampaignPause> {
        let mut prompt = self.prompt();
        let p = prompt.as_mut().filter(
            |p| matches!(p.screen, Screen::Open(at) | Screen::Closing(at) if at == screen),
        )?;
        p.expire(Instant::now());
        if p.decision == Decision::Pending {
            p.decision = p.form.dismissed;
        }
        p.screen = Screen::Gone;
        p.pause.take()
    }

    fn class(self: &Arc<Self>, form: &'static Form) -> ListClass {
        let closed = Arc::clone(self);
        let g = DialogGeometry::fallback();
        let rows = Arc::new(FormRows {
            dialog: Arc::clone(self),
            form,
        });
        ListClass::with_rows(form.class, form.screen, rows)
            .dialog(DialogBase::Large)
            .layout(layout(form))
            .panel(Panel {
                art: Vec::new(),
                texts: vec![PanelText {
                    block: TextBlock {
                        rect: [
                            dialog::BODY_TOP,
                            dialog::LEFT,
                            dialog::STATUS.top + 8,
                            dialog::RIGHT,
                        ],
                        font: g.body_font,
                        colour: g.body_colour,
                        animation: 0,
                    },
                    right: false,
                    bias: 0,
                }],
            })
            .footer(IDS.id(FOOTER_ID))
            .action(Arc::new(|cx, screen| {
                Box::pin(async move { ui_kit::dialog_tag::close(&cx, screen).await })
            }))
            .action_buttons(&[])
            .on_free(Arc::new(move |cx, screen| {
                if let Some(pause) = closed.close(screen) {
                    cx.write(|m| pause.release(m));
                }
                Box::pin(async { Ok(()) })
            }))
    }

    fn spec(&self, form: &Form) -> ScreenSpec {
        let body = self
            .prompt()
            .as_ref()
            .map_or_else(String::new, |p| p.body.clone());
        let mut spec = ScreenSpec::new(form.screen, TITLE);
        spec.list = ListPlace::from(Some(layout(form)));
        spec.rows = form
            .labels
            .iter()
            .enumerate()
            .map(|(row, label)| {
                RowSpec::action(label)
                    .press(Press::Act)
                    .ids(&[IDS.id(form.first_id + row as u32)])
            })
            .collect();
        spec.ids = vec![IDS.id(TITLE_ID), IDS.id(BODY_ID), IDS.id(FOOTER_ID)];
        spec.prompts = prompts().into();
        spec.footer = true;
        spec.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
        spec.dialog = Some(DialogSpec {
            rows: spec.rows.clone(),
            ..DialogSpec::new(&body, Opener::MAIN_MENU)
        });
        spec
    }

    fn page(self: &Arc<Self>, form: &'static Form) -> ListPage {
        let dialog = Arc::clone(self);
        ListPage::new(self.class(form), move || dialog.spec(form))
    }

    /// A press on row `row` of `form`'s dialog. Only an open question of
    /// that form takes it.
    fn press(&self, form: &Form, row: i16) -> Chosen {
        let Some(&decision) = usize::try_from(row)
            .ok()
            .and_then(|row| form.answers.get(row))
        else {
            return Chosen::Nothing;
        };
        if let Some(p) = self.prompt().as_mut() {
            p.expire(Instant::now());
            if p.form.screen == form.screen
                && p.decision == Decision::Pending
                && matches!(p.screen, Screen::Open(_))
            {
                p.decision = decision;
            }
        }
        Chosen::Act
    }
}

/// One question's rows over the shared dialog state.
struct FormRows {
    dialog: Arc<Dialog>,
    form: &'static Form,
}

impl Rows for FormRows {
    fn title(&self) -> u32 {
        IDS.id(TITLE_ID)
    }

    fn count(&self) -> usize {
        self.form.labels.len()
    }

    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        if (0..self.form.labels.len() as i16).contains(&row) {
            vec![(LABEL_SLOT, IDS.id(self.form.first_id + row as u32))]
        } else {
            Vec::new()
        }
    }

    fn panel_text(&self, index: usize) -> Option<u32> {
        (index == 0).then(|| IDS.id(BODY_ID))
    }

    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.dialog.texts.write(m)
    }

    fn press(&self, row: i16) -> Chosen {
        self.dialog.press(self.form, row)
    }
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

pub(crate) fn add_info(agent: &agent::Agent, window: bool) {
    agent.add_method(takeover::INFO, Access::Read, move |params| {
        if !params.is_null() && !params.as_object().is_some_and(|fields| fields.is_empty()) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "expected no parameters",
            ));
        }
        Ok(json!(takeover::Info {
            window,
            pid: Some(std::process::id())
        }))
    });
}

fn add_consent(agent: &agent::Agent, dialog: &Arc<Dialog>) {
    let d = Arc::clone(dialog);
    agent.add_method(takeover::REQUEST, Access::Act, move |params| {
        let id = d.submit(parse(params)?, Instant::now())?;
        Ok(json!(takeover::Requested { id }))
    });
    let d = Arc::clone(dialog);
    agent.add_method(takeover::STATUS, Access::Read, move |params| {
        let takeover::StatusParams { id } = parse(params)?;
        let decision = d.status(id, Instant::now())?;
        Ok(json!(takeover::Status { decision }))
    });
}

/// The same pause latch the campaign pause menu uses. Keep ownership only
/// if this dialog set it in an initialized local campaign.
/// See docs/game/settings-menu.md, "What pausing means".
struct CampaignPause(u32);

impl CampaignPause {
    fn clock(m: &dyn GuestMemory) -> Option<u32> {
        let mem = ReadGm(m);
        if level_boot::active_mode(&mem) != Some(level_boot::MODE_CAMPAIGN)
            || !coop_probe::read_sim(&mem).is_some_and(|sim| sim.mode == 1 && sim.ended == 0)
        {
            return None;
        }
        let clock = m.u32(points::GAME_TIME.base).ok().filter(|&at| at != 0)?;
        (m.u8(clock).ok()? != 0).then_some(clock)
    }

    fn acquire(m: &mut dyn GuestMemory) -> Option<Self> {
        let clock = Self::clock(m)?;
        if m.u8(clock + 1).ok()? != 0 {
            return None;
        }
        m.set_u8(clock + 1, 1).ok()?;
        Some(Self(clock))
    }

    fn release(self, m: &mut dyn GuestMemory) {
        if Self::clock(m) == Some(self.0) {
            let _ = m.set_u8(self.0 + 1, 0);
        }
    }
}

async fn open(cx: &Cx, screen: &str) -> Result<u32, EntryError> {
    use ui_kit::dialog_tag;

    let Some(d) = list_class::loader_of(screen).and_then(dialog_tag::by_loader) else {
        return Ok(0);
    };
    dialog_tag::open_over_screen(cx, &d, 1).await
}

enum UiStep {
    Open(&'static str),
    Close(u32),
}

struct Driver(Arc<Dialog>);

impl Feature for Driver {
    fn manifest(&self) -> Manifest {
        Manifest::new("lab-takeover", "Ask the player before a lab job").doc("docs/runtime/lab.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        self.0
            .texts
            .base
            .store(b.guest_alloc(0x1000, Permissions::RW)?, Ordering::SeqCst);
        for form in [&START, &ALONGSIDE, &RUNNING] {
            let dialog = Arc::clone(&self.0);
            b.locale()
                .source(form.screen, move |id| dialog.texts.address(id));
        }
        let (guard, restore) = (Arc::clone(&self.0), Arc::clone(&self.0));
        b.snapshot(SnapshotPolicy::section_forbid_while(
            move || {
                guard
                    .prompt()
                    .as_ref()
                    .is_some_and(|p| !matches!(p.screen, Screen::Queued | Screen::Gone))
                    .then(|| "lab consent dialog is open".into())
            },
            Vec::new,
            move |_, _| {
                restore.texts.rewrite_all();
                Ok(())
            },
        ));

        let dialog = Arc::clone(&self.0);
        b.raw()
            .before(
                &points::UI_UPDATE,
                "open or expire a lab request",
                move |c| {
                    let mut prompt = dialog.prompt();
                    let Some(p) = prompt.as_mut() else {
                        return Ok(Flow::Continue);
                    };
                    p.expire(Instant::now());
                    // Don't replace a dialog that's still being pushed on this channel.
                    let step = match p.screen {
                        Screen::Queued if p.decision == Decision::Pending => {
                            let mem = c.ctx.runtime().memory();
                            let mem = ReadGm(&*mem);
                            if level_boot::active_mode(&mem).is_none()
                                || (0..=ui_watch::SHARED).any(|slot| {
                                    ui_watch::stack(&mem, ui_kit::dialog_tag::DIALOG_WINDOW, slot)
                                        .is_some_and(|stack| stack.pending != 0)
                                })
                            {
                                return Ok(Flow::Continue);
                            }
                            dialog.texts.set(BODY_ID, &p.body.replace('\n', "\r\n"));
                            p.screen = Screen::Opening;
                            UiStep::Open(p.form.screen)
                        }
                        Screen::Open(at) if p.decision == Decision::TimedOut => {
                            p.screen = Screen::Closing(at);
                            UiStep::Close(at)
                        }
                        _ => return Ok(Flow::Continue),
                    };
                    drop(prompt);
                    let dialog = Arc::clone(&dialog);
                    c.answer_async(move |cx| {
                        Box::pin(async move {
                            let elapsed = cx.arg(0)?;
                            match step {
                                UiStep::Open(screen) => {
                                    let result = open(&cx, screen).await;
                                    if let Err(e) = &result {
                                        eprintln!("lab takeover: opening the dialog: {e}");
                                    }
                                    if let Some(p) = dialog.prompt().as_mut() {
                                        match result.unwrap_or(0) {
                                            0 => {
                                                p.screen = Screen::Gone;
                                                if p.decision == Decision::Pending {
                                                    p.decision = Decision::Unavailable;
                                                }
                                            }
                                            screen => {
                                                p.screen = Screen::Open(screen);
                                                p.pause = cx.write(CampaignPause::acquire);
                                            }
                                        }
                                    }
                                }
                                UiStep::Close(screen) => {
                                    if let Err(e) = ui_kit::dialog_tag::close(&cx, screen).await {
                                        eprintln!("lab takeover: closing the dialog: {e}");
                                        if let Some(pause) = dialog.close(screen) {
                                            cx.write(|m| pause.release(m));
                                        }
                                    }
                                }
                            }
                            let out = cx
                                .call(GuestCall::new(points::UI_UPDATE.base).args([elapsed]))
                                .await?;
                            Ok(Ret::new(out.eax, 4))
                        })
                    })
                },
            )
            // The Live join link also runs before UI_UPDATE; a pending join
            // goes first and the lab request waits a frame.
            .after_feature("live-join-link")
            // So does a linked Theater film page, which also goes first.
            .after_feature("theater-link")
            // And a Forge screen opening or closing.
            .after_feature("forge-screens")
            .optional();
        Ok(())
    }
}

/// Plans the dialog's hooks, and with `accept` takes the lab's requests
/// (`session.request_takeover`). Without it no request is ever made, so the
/// hooks never act: a film's replay plans them as its recording did and
/// never opens the dialog.
pub fn install(registry: &mut hooks::Registry, agent: &agent::Agent, accept: bool) -> anyhow::Result<()> {
    let dialog = Arc::new(Dialog::default());
    if registry.add(&dialog.class(&START))? && registry.add(&Driver(Arc::clone(&dialog)))? {
        // With its class switched off, the running question can't open and
        // answers Unavailable; the lab then lets the job finish.
        registry.add(&dialog.class(&ALONGSIDE))?;
        registry.add(&dialog.class(&RUNNING))?;
        if accept {
            add_consent(agent, &dialog);
        }
    }
    Ok(())
}

pub(crate) fn sample_page() -> ListPage {
    sample(sample_request(), &START)
}

/// The start question with Start alongside, for the screen audit.
pub(crate) fn sample_alongside_page() -> ListPage {
    sample(
        Request {
            alongside: true,
            ..sample_request()
        },
        &ALONGSIDE,
    )
}

/// The question a game asks while a lab job runs, for the screen audit.
pub(crate) fn sample_running_page() -> ListPage {
    sample(
        Request {
            ask: Ask::Running,
            ..sample_request()
        },
        &RUNNING,
    )
}

fn sample(request: Request, form: &'static Form) -> ListPage {
    let dialog = Arc::new(Dialog::default());
    dialog
        .submit(request, Instant::now())
        .expect("sample request");
    dialog.page(form)
}

fn sample_request() -> Request {
    Request {
        ask: Ask::Start,
        requester: "Lab driver".into(),
        name: "Menu bands check".into(),
        operation: Operation::Run,
        builds_image: true,
        source: Source::Branch {
            name: "menu-bands".into(),
            commit: "cb9b315".into(),
        },
        level: Some("ascension".into()),
        own_data: false,
        estimate_seconds: 240,
        timeout_seconds: 60,
        alongside: false,
    }
}

#[cfg(test)]
mod tests;
