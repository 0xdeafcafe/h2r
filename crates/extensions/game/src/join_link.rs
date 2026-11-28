//! Web joins enter through the same accepted-invite record and player join
//! action as the friends screen. UI_UPDATE also runs while a game is paused.
mod dialog;
mod outcome;

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};
use std::time::{Duration, Instant};

use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::{memory::GuestMemory, points};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use live_hle::{
    Hle,
    join_link::{self, Phase, Request, Source},
    join_resolver::{JoinLookup, ResolvedJoin},
};
use runtime::guest::Permissions;

use crate::{
    coop_probe, layout_glue::ReadGm, level_boot, list_class, ui_kit::dialog_tag, ui_watch,
};
use dialog::Dialog;
use outcome::Outcome;

/// The Xbox Live action on the main menu, including its ordinary account
/// picker and sign-in path. Its second argument identifies the controller.
const OPEN_LIVE: u32 = 0x0023_08e0;
/// The accepted-invite getter, the main-menu delegate's selected-player
/// setter, and the players screen's accepted-invite Join Game action
/// (mode 4). See docs/live/live-coop.md, sections 12.1 and 12.5.
const SIGNED_IN: u32 = 0x0006_c7e0;
const READ_INVITE: u32 = 0x0006_d210;
const SELECT_TARGET: u32 = 0x0014_8893;
const JOIN_SELECTED: u32 = 0x0023_8f3f;
// Non-overlapping parts of one feature-owned scratch page.
const SELECTION: u32 = 0x100;
const CONTROLLER: u32 = 0x200;
const DEADLINE: Duration = Duration::from_secs(60);
const SIGNIN_LIMIT: Duration = Duration::from_secs(180);

enum Step {
    Start,
    SignIn { opened: bool },
    Lookup(JoinLookup),
    Confirm { place: Place, for_sign_in: bool },
    Returning,
    WaitingForLobby,
    ShowError { reason: String, retry: Instant },
    DismissError,
}

struct Job {
    request: Request,
    step: Step,
    until: Instant,
    /// Consent covers this departure only. A new match needs a new question.
    confirmed: Option<Place>,
}

impl Job {
    fn new(request: Request) -> Self {
        Self {
            request,
            step: Step::Start,
            until: Instant::now() + DEADLINE,
            confirmed: None,
        }
    }

    fn phase(&self, phase: Phase) {
        join_link::set_phase(self.request.id, phase);
    }

    fn fail(&mut self, reason: String) {
        eprintln!("live: join link: {reason}");
        self.phase(Phase::Failed {
            reason: reason.clone(),
        });
        self.step = Step::ShowError {
            reason,
            retry: Instant::now(),
        };
    }

    fn resolve(&mut self, hle: &Hle) -> Result<(), String> {
        self.step = Step::Lookup(hle.resolve_join_link(self.request.link.session())?);
        self.phase(Phase::Resolving);
        self.until = Instant::now() + DEADLINE;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Squad {
    addr: u32,
    incarnation: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Menu,
    Lobby(Squad),
    Game(Option<Squad>),
}

fn place(m: &dyn GuestMemory) -> Option<Place> {
    let mem = ReadGm(m);
    let mode = level_boot::active_mode(&mem)?;
    // Retail can replace a squad at the same address while the prompt is open.
    let squad = match coop_probe::read_session(&mem).filter(coop_probe::Session::live) {
        Some(s) => Some(Squad {
            addr: s.addr,
            incarnation: m.u32(s.addr.checked_add(bots::phantom::session::at::INCARNATION)?).ok()?,
        }),
        None => None,
    };
    match mode {
        level_boot::MODE_MAINMENU => Some(squad.map_or(Place::Menu, Place::Lobby)),
        level_boot::MODE_CAMPAIGN | level_boot::MODE_MULTIPLAYER => Some(Place::Game(squad)),
        _ => None,
    }
}

fn main_menu(m: &dyn GuestMemory) -> Option<u32> {
    let memory = ReadGm(m);
    if ui_watch::dialog(&memory).is_some() {
        return None;
    }
    // The root is sometimes only in the channel's list. The normal input
    // dispatcher can then report no top screen despite the root being up.
    for slot in [ui_watch::SHARED, 0] {
        let Some(stack) = ui_watch::stack(&memory, 5, slot) else {
            continue;
        };
        if let Some((screen, _)) = stack
            .list
            .into_iter()
            .find(|(_, id)| *id == Some(ui_watch::MAIN_MENU))
        {
            return Some(screen);
        }
    }
    None
}

async fn sign_in(cx: &Cx, root: u32, scratch: u32) -> Result<(), EntryError> {
    // The menu action dereferences its context once to read controller +4.
    // Passing the actual root lets its account picker return to that menu.
    cx.write(|m| {
        m.set_u32(scratch, scratch + 4)?;
        m.set_u32(scratch + 8, 0)
    })?;
    cx.call(GuestCall::new(OPEN_LIVE).args([root, scratch]))
        .await?;
    Ok(())
}

/// The title checks its own logon before entering Join Game. The HLE may
/// have an account before the title has finished consuming the logon task.
async fn ready(cx: &Cx, hle: &Hle) -> Result<bool, String> {
    Ok(hle.join_link_ready() && guest(cx, GuestCall::new(SIGNED_IN)).await? as u8 != 0)
}

async fn open_dialog(cx: &Cx) -> Result<(), String> {
    let d = list_class::loader_of(dialog::SCREEN)
        .and_then(dialog_tag::by_loader)
        .ok_or("The Xbox Live dialog is unavailable.")?;
    let at = d.page + dialog_tag::REQUEST_AT;
    cx.write(|m| {
        m.write(
            at,
            &dialog_tag::request(dialog_tag::DIALOG_WINDOW, 1, d.loader),
        )
    })
    .map_err(|e| EntryError::from(e).to_string())?;
    let screen = guest(cx, GuestCall::new(d.loader).args([at])).await?;
    if screen == 0 {
        return Err("The Xbox Live dialog could not open.".into());
    }
    Ok(())
}

async fn guest(cx: &Cx, call: GuestCall) -> Result<u32, String> {
    cx.call(call)
        .await
        .map(|r| r.eax)
        .map_err(|e| e.to_string())
}

/// Translate the HLE's accepted-invite ABI into the selected friend consumed
/// by the title. Same-title invitations have kind 2, and keep the inviting
/// friend and the acceptance time. The remaining selected fields start empty.
fn selected(invite: &[u8; 156]) -> [u8; 120] {
    let mut target = [0; 120];
    target[..4].copy_from_slice(&2u32.to_le_bytes());
    target[4..90].copy_from_slice(&invite[..86]);
    // This selection came from a link. The social join action answers an
    // inbox message only for the received-invite bit (friend flags +28).
    // Keep online/joinable and the title ID, never answer another invite.
    let flags = u32::from_le_bytes(invite[28..32].try_into().unwrap()) & !0x0800_0000;
    target[32..36].copy_from_slice(&flags.to_le_bytes());
    target[36..44].copy_from_slice(&invite[100..108]);
    target
}

async fn dispatch(
    cx: &Cx,
    scratch: u32,
    hle: &Hle,
    target: ResolvedJoin,
    outcome: &Outcome,
) -> Result<(), String> {
    let controller = target.controller();
    let receipt = hle.accept_join_link(target)?;
    let taken = guest(cx, GuestCall::new(READ_INVITE).eax(scratch)).await?;
    if taken as u8 == 0 || !receipt.consumed() {
        return Err("Halo 2 could not accept this invitation.".into());
    }
    dispatch_record(cx, scratch, controller, outcome).await
}

async fn dispatch_record(
    cx: &Cx,
    scratch: u32,
    controller: u32,
    outcome: &Outcome,
) -> Result<(), String> {
    let selection = scratch + SELECTION;
    let requested = cx
        .write(|m| {
            let mut record = [0; 156];
            m.read(scratch, &mut record)?;
            m.write(selection, &selected(&record))?;
            Ok::<[u8; 8], guest_layout::MemFault>(record[40..48].try_into().unwrap())
        })
        .map_err(|e| EntryError::from(e).to_string())?;
    outcome.begin(requested);
    guest(cx, GuestCall::new(SELECT_TARGET).args([selection, 1])).await?;
    guest(
        cx,
        GuestCall::new(JOIN_SELECTED).args([controller, 0, 0, 0]),
    )
    .await?;
    Ok(())
}

async fn question(
    cx: &Cx,
    dialog: &Dialog,
    job: &mut Job,
    location: Place,
    host: Option<&str>,
) -> Result<(), String> {
    let mut body = match host {
        Some(host) => format!("Join {host}'s game?"),
        None => "Sign in to Xbox Live to join this game?".into(),
    };
    match location {
        Place::Menu => {}
        Place::Lobby(_) => body.push_str(" You'll leave your lobby."),
        Place::Game(_) => body.push_str(" You'll leave your current game."),
    }
    dialog.prepare(true, &body);
    open_dialog(cx).await?;
    job.phase(Phase::Confirm {
        host: host.map(str::to_owned),
    });
    job.step = Step::Confirm {
        place: location,
        for_sign_in: host.is_none(),
    };
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum ReadyAction {
    Join,
    Ask,
    Leave,
}

fn join_action(location: Place, source: Source, confirmed: Option<Place>) -> ReadyAction {
    match location {
        Place::Menu if source == Source::CommandLine || confirmed.is_some() => ReadyAction::Join,
        Place::Lobby(_) | Place::Game(_) if confirmed == Some(location) => ReadyAction::Leave,
        _ => ReadyAction::Ask,
    }
}

/// Handles one boundary. Waits for network and user responses by returning to
/// the retail UI loop. Guest memory and host locks are released at each call.
async fn advance(
    cx: &Cx,
    scratch: u32,
    hle: &Hle,
    dialog: &Dialog,
    job: &mut Job,
    outcome: &Outcome,
) -> Result<bool, String> {
    use Step::*;
    let location = cx.read(place);
    if let ShowError { reason, retry } = &mut job.step {
        if job.confirmed.is_none() && matches!(location, Some(Place::Game(_) | Place::Lobby(_))) {
            // fail() has logged the reason and published the terminal agent status.
            return Ok(true);
        }
        if location.is_some() && Instant::now() >= *retry {
            dialog.prepare(false, reason);
            if let Err(e) = open_dialog(cx).await {
                // A map may be unloading; retry once its interface is active.
                eprintln!("live: join dialog: {e}");
                *retry = Instant::now() + Duration::from_secs(2);
            } else {
                job.step = DismissError;
            }
        }
        return Ok(false);
    }
    if matches!(job.step, DismissError) {
        return Ok(dialog.answer().is_some());
    }
    let server = hle
        .config()
        .server_url
        .as_deref()
        .ok_or("Choose a Live server and sign in first.")?;
    if !job.request.link.matches_server(server) {
        return Err(format!(
            "This game is on {}. Choose that Live server and open the link again.",
            job.request.link.server()
        ));
    }
    if !matches!(job.step, Confirm { .. }) && Instant::now() >= job.until {
        return Err(match job.step {
            SignIn { .. } => "Xbox Live sign-in did not finish. Sign in and open the link again.",
            Lookup(_) => "Xbox Live did not answer the session lookup. Try again.",
            WaitingForLobby => "Halo 2 could not join the host's lobby. The game may have closed.",
            _ => "Halo 2 could not finish the join. Open the link again.",
        }
        .into());
    }
    match &mut job.step {
        Start | SignIn { .. } => {
            let Some(location) = location else {
                return Ok(false);
            };
            if ready(cx, hle).await? {
                job.resolve(hle)?;
            } else {
                match join_action(location, job.request.source, job.confirmed) {
                    ReadyAction::Ask => question(cx, dialog, job, location, None).await?,
                    ReadyAction::Leave => return_to_menu(cx, job).await?,
                    ReadyAction::Join => match &mut job.step {
                        Start => {
                            job.step = SignIn { opened: false };
                            job.until = Instant::now() + SIGNIN_LIMIT;
                            job.phase(Phase::SigningIn);
                        }
                        SignIn { opened } if !*opened => {
                            if let Some(menu) = cx.read(main_menu) {
                                *opened = true;
                                sign_in(cx, menu, scratch + CONTROLLER)
                                    .await
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                        _ => {}
                    },
                }
            }
        }
        Lookup(lookup) => {
            if let Some(location) = location
                && let Some(target) = lookup.poll()
            {
                let target = target?;
                match join_action(location, job.request.source, job.confirmed) {
                    ReadyAction::Ask => {
                        question(cx, dialog, job, location, Some(target.host())).await?
                    }
                    ReadyAction::Leave => return_to_menu(cx, job).await?,
                    ReadyAction::Join => {
                        dispatch(cx, scratch, hle, target, outcome).await?;
                        job.phase(Phase::Joining);
                        job.step = WaitingForLobby;
                        job.until = Instant::now() + DEADLINE;
                    }
                }
            }
        }
        Confirm { place, for_sign_in } => match dialog.answer() {
            None => {}
            Some(false) => {
                job.phase(Phase::Cancelled);
                return Ok(true);
            }
            Some(true) => {
                job.confirmed = Some(*place);
                // Capacity, privacy and our own identity may have changed
                // while the prompt was up. Recheck before leaving.
                if ready(cx, hle).await? {
                    job.resolve(hle)?;
                } else if *for_sign_in {
                    job.step = Start;
                    job.until = Instant::now() + DEADLINE;
                } else {
                    return Err("Your Xbox Live sign-in changed. Open the link again.".into());
                }
            }
        },
        Returning if matches!(location, Some(Place::Menu)) => {
            // The request is confirmed, but leaving another squad needs fresh consent.
            job.confirmed = Some(Place::Menu);
            job.step = Start;
        }
        WaitingForLobby => {
            if let Some(result) = outcome.take() {
                result?;
                job.phase(Phase::Joined);
                return Ok(true);
            }
        }
        Returning | ShowError { .. } | DismissError => {}
    }
    Ok(false)
}

async fn return_to_menu(cx: &Cx, job: &mut Job) -> Result<(), String> {
    // The title's normal departure retires its network sessions before the
    // main-menu map is requested, as in host_lobby::retail.
    guest(cx, GuestCall::new(crate::host_lobby::LEAVE_SESSIONS)).await?;
    guest(cx, GuestCall::new(level_boot::REQUEST_MAINMENU).args([0])).await?;
    job.step = Step::Returning;
    job.phase(Phase::Joining);
    job.until = Instant::now() + DEADLINE;
    Ok(())
}

struct Driver {
    live: Arc<Hle>,
    dialog: Arc<Dialog>,
    before_join: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Feature for Driver {
    fn manifest(&self) -> Manifest {
        Manifest::new("live-join-link", "Join a Live session from a URL")
            .doc("docs/live/live-custom-games.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let outcome = Arc::new(Outcome::default());
        outcome.observe(b);
        let scratch = b.guest_alloc(0x1000, Permissions::RW)?;
        let texts = b.guest_alloc(0x1000, Permissions::RW)?;
        self.dialog.texts.base.store(texts, Ordering::SeqCst);
        let dialog = self.dialog.clone();
        b.locale()
            .source("live.join", move |id| dialog.texts.address(id));
        let pending = Arc::new(Mutex::new(None::<Job>));
        const IDLE: u8 = 0;
        const WAITING: u8 = 1;
        const IN_UI_CALL: u8 = 2;
        let stage = Arc::new(AtomicU8::new(IDLE));
        let (active, dialog) = (stage.clone(), self.dialog.clone());
        b.snapshot(SnapshotPolicy::section_forbid_while(
            move || {
                (active.load(Ordering::Acquire) != IDLE)
                    .then(|| "Xbox Live join in progress".into())
            },
            Vec::new,
            move |_, _| {
                dialog.texts.rewrite_all();
                Ok(())
            },
        ));
        let (live, dialog) = (self.live.clone(), self.dialog.clone());
        let before_join = self.before_join.clone();
        b.raw()
            .before(&points::UI_UPDATE, "process a Live join link", move |c| {
                let job = match stage.load(Ordering::Acquire) {
                    IDLE => join_link::take_pending().map(|request| {
                        if let Some(before) = &before_join {
                            before();
                        }
                        Job::new(request)
                    }),
                    WAITING => pending.lock().unwrap_or_else(|e| e.into_inner()).take(),
                    _ => None,
                };
                let Some(mut job) = job else {
                    return Ok(Flow::Continue);
                };
                stage.store(IN_UI_CALL, Ordering::Release);
                let (pending, stage, dialog, live) =
                    (pending.clone(), stage.clone(), dialog.clone(), live.clone());
                let outcome = outcome.clone();
                c.answer_async(move |cx| {
                    Box::pin(async move {
                        let elapsed = cx.arg(0)?;
                        let done =
                            match advance(&cx, scratch, &live, &dialog, &mut job, &outcome).await {
                                Ok(done) => done,
                                Err(error) => {
                                    outcome.clear();
                                    job.fail(error);
                                    job.until = Instant::now() + DEADLINE;
                                    false
                                }
                            };
                        if done {
                            outcome.clear();
                        } else {
                            *pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(job);
                        }
                        let out = cx
                            .call(GuestCall::new(points::UI_UPDATE.base).args([elapsed]))
                            .await;
                        stage.store(if done { IDLE } else { WAITING }, Ordering::Release);
                        Ok(Ret::new(out?.eax, 4))
                    })
                })
            })
            .optional();
        Ok(())
    }
}

/// `before_join` runs as a link's join starts: a default film recording
/// cannot replay a join that came from outside the game, so it stops
/// (`theater::autorecord::stopper`).
pub fn install(
    features: &mut hooks::Registry,
    live: Option<Arc<Hle>>,
    unsupported: bool,
    before_join: Option<Arc<dyn Fn() + Send + Sync>>,
) -> anyhow::Result<()> {
    let Some(live) =
        live.filter(|h| !unsupported && h.config().enabled && h.config().server_url.is_some())
    else {
        join_link::set_enabled(false);
        return Ok(());
    };
    let dialog = Arc::new(Dialog::default());
    let has_dialog = features
        .add(&dialog.class())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let has_driver = features
        .add(&Driver { live, dialog, before_join })
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    join_link::set_enabled(has_dialog && has_driver);
    Ok(())
}

#[cfg(test)]
mod tests;
