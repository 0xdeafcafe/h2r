//! The lab's consent prompt. Only a player choosing Allow can grant consent.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use agent::Access;
use agent_proto::takeover::{self, Decision, Operation, Request};
use agent_proto::{RpcError, code};
use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;
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
const CLASS: &str = "lab-takeover-dialog";
const IDS: ui_kit::ids::Family = ui_kit::ids::family("lab takeover");
const TITLE: &str = "Lab request";
// Put the safe answer on the initial focus, including when a button is held.
const LABELS: [&str; 2] = ["Not now", "Allow"];

fn prompts() -> [Prompt; 2] {
    [
        Prompt::new(&[Glyph::A], "Select"),
        Prompt::new(&[Glyph::B], "Back"),
    ]
}

fn layout() -> ListLayout {
    let mut l = ui_kit::dialog_tag::base_layout(DialogBase::Large, LABELS.len());
    l.anchor[1] = -24;
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

fn body(request: &Request) -> String {
    let who = display_field(&request.requester, 16);
    let revision = display_field(&request.revision, 16);
    let operation = match request.operation {
        Operation::Build => "build",
        Operation::Run => "run",
    };
    let time = match request.estimate_seconds {
        0 => "duration varies".into(),
        n @ 1..=59 => format!("about {n} sec"),
        n => format!("about {} min", n.div_ceil(60)),
    };
    format!("{who} requests a lab {operation}.\n{revision}, {time}\nAllow, then quit when ready.")
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
        texts.set(0, TITLE);
        for (row, label) in LABELS.iter().enumerate() {
            texts.set(2 + row as u32, label);
        }
        texts.set(4, &ui_kit::input::legend(&prompts()));
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
        if request.requester.trim().is_empty()
            || request.revision.trim().is_empty()
            || !(1..=takeover::MAX_TIMEOUT_SECONDS).contains(&request.timeout_seconds)
        {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "expected a requester, a revision and a timeout from 1 to 3600 seconds",
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
            p.decision = Decision::Declined;
        }
        p.screen = Screen::Gone;
        p.pause.take()
    }

    fn class(self: &Arc<Self>) -> ListClass {
        let closed = Arc::clone(self);
        let g = DialogGeometry::fallback();
        ListClass::with_rows(CLASS, SCREEN, self.clone())
            .dialog(DialogBase::Large)
            .layout(layout())
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
            .footer(IDS.id(4))
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

    fn spec(&self) -> ScreenSpec {
        let body = self
            .prompt()
            .as_ref()
            .map_or_else(String::new, |p| p.body.clone());
        let mut spec = ScreenSpec::new(SCREEN, TITLE);
        spec.list = ListPlace::from(Some(layout()));
        spec.rows = LABELS
            .iter()
            .enumerate()
            .map(|(row, label)| {
                RowSpec::action(label)
                    .press(Press::Act)
                    .ids(&[IDS.id(2 + row as u32)])
            })
            .collect();
        spec.ids = vec![IDS.id(0), IDS.id(1), IDS.id(4)];
        spec.prompts = prompts().into();
        spec.footer = true;
        spec.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
        spec.dialog = Some(DialogSpec {
            rows: spec.rows.clone(),
            ..DialogSpec::new(&body, Opener::MAIN_MENU)
        });
        spec
    }

    fn page(self: &Arc<Self>) -> ListPage {
        let dialog = Arc::clone(self);
        ListPage::new(self.class(), move || dialog.spec())
    }
}

impl Rows for Dialog {
    fn title(&self) -> u32 {
        IDS.id(0)
    }

    fn count(&self) -> usize {
        LABELS.len()
    }

    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        if (0..LABELS.len() as i16).contains(&row) {
            vec![(LABEL_SLOT, IDS.id(2 + row as u32))]
        } else {
            Vec::new()
        }
    }

    fn panel_text(&self, index: usize) -> Option<u32> {
        (index == 0).then(|| IDS.id(1))
    }

    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.texts.write(m)
    }

    fn press(&self, row: i16) -> Chosen {
        let decision = match row {
            0 => Decision::Declined,
            1 => Decision::Allowed,
            _ => return Chosen::Nothing,
        };
        if let Some(p) = self.prompt().as_mut() {
            p.expire(Instant::now());
            if p.decision == Decision::Pending && matches!(p.screen, Screen::Open(_)) {
                p.decision = decision;
            }
        }
        Chosen::Act
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

async fn open(cx: &Cx) -> Result<u32, EntryError> {
    use ui_kit::dialog_tag;

    let Some(d) = list_class::loader_of(SCREEN).and_then(dialog_tag::by_loader) else {
        return Ok(0);
    };
    dialog_tag::open_over_screen(cx, &d, 1).await
}

enum UiStep {
    Open,
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
        let dialog = Arc::clone(&self.0);
        b.locale()
            .source(SCREEN, move |id| dialog.texts.address(id));
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
                            dialog.texts.set(1, &p.body.replace('\n', "\r\n"));
                            p.screen = Screen::Opening;
                            UiStep::Open
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
                                UiStep::Open => {
                                    let result = open(&cx).await;
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
            .optional();
        Ok(())
    }
}

pub fn install(registry: &mut hooks::Registry, agent: &agent::Agent) -> anyhow::Result<()> {
    let dialog = Arc::new(Dialog::default());
    if registry.add(&dialog.class())? && registry.add(&Driver(Arc::clone(&dialog)))? {
        add_consent(agent, &dialog);
    }
    Ok(())
}

pub(crate) fn sample_page() -> ListPage {
    let dialog = Arc::new(Dialog::default());
    dialog
        .submit(sample_request(), Instant::now())
        .expect("sample request");
    dialog.page()
}

fn sample_request() -> Request {
    Request {
        requester: "Lab driver".into(),
        operation: Operation::Run,
        revision: "cb9b315e1".into(),
        estimate_seconds: 240,
        timeout_seconds: 60,
    }
}

#[cfg(test)]
mod tests;
