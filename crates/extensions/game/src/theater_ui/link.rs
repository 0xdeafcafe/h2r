use super::{DETAIL, Entry, FILMS, Location, Theater, adapter};
use crate::{layout_glue::ReadGm, list_class, ui_watch};
use agent_proto::{RpcError, code, theater as api};
use guest_layout::{entry::{GuestCall, Ret}, points};
use hooks::{Feature, Flow};
use live_proto::theater_link::TheaterLink;
use serde_json::Value;
use std::sync::{Mutex, mpsc};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Publish {
    id: String,
    visibility: live_proto::film::sharing::Visibility,
    clip: Option<ShareClip>,
}
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ShareClip {
    Id(String),
    Bounds([u64; 2]),
}

fn defer_reply(ctx: &mut agent::GameCtx<'_>, rx: mpsc::Receiver<Result<Value, String>>) {
    ctx.defer(
        agent::DEFER_TICKS_MAX,
        Box::new(move |_| match rx.try_recv() {
            Ok(result) => Some(result.map_err(|e| RpcError::new(code::UNAVAILABLE, e))),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err(RpcError::new(
                code::UNAVAILABLE,
                "Theater request ended",
            ))),
        }),
    );
}

type Reply = mpsc::Sender<Result<Value, String>>;
static EXTERNAL: Mutex<Option<TheaterLink>> = Mutex::new(None);

/// Launch Services can deliver the event before the front end has installed Theater.
pub fn external_link(link: TheaterLink) {
    *EXTERNAL.lock().unwrap_or_else(|e| e.into_inner()) = Some(link);
}

pub(super) struct Ready {
    entry: Entry,
    view: api::InitialView,
    account: live_proto::Xuid,
    controller: u32,
    generation: u64,
    reply: Option<Reply>,
    /// A film of this machine's own library (the film page after a film
    /// watched in the running game): no account selected it.
    local: bool,
    /// Menu updates a local page has waited for the menu to settle.
    waited: u32,
    /// What a local page has seen of the menu at its last update.
    menu: Menu,
    /// The page's status line once it opens: why the film stopped.
    notice: Option<String>,
}

/// The menu a local page waits on, as its last update saw it: the screen on
/// top, and for how many updates in a row the main menu has been the game
/// with that screen on top.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Menu {
    top: Option<(u32, u32)>,
    settled: u32,
}

impl Menu {
    /// This update's look: `main_menu` says the main menu is the active
    /// game, `top` is the screen on top (its id and address). The count
    /// starts again whenever the screen on top changes, and is zero while
    /// the main menu is not the game or no screen is up.
    fn after(self, main_menu: bool, top: Option<(u32, u32)>) -> Menu {
        let settled = match top {
            Some(_) if main_menu && top == self.top => self.settled + 1,
            Some(_) if main_menu => 1,
            _ => 0,
        };
        Menu { top, settled }
    }
}

/// A film page reopened after a film watched in the running game waits for
/// the main menu: the main menu is the active game and the same screen has
/// been on top for this many updates in a row ([`Menu`]). The page is
/// opened over a menu that is still building its own screens otherwise
/// (after a film stopped at its start, the menu's load runs long past any
/// fixed count of updates), and the check that the page is on top fails. It
/// gives up waiting after [`MENU_WAIT_UPDATES`].
const MENU_SETTLE_UPDATES: u32 = 20;
const MENU_WAIT_UPDATES: u32 = 900;

/// The screens a reopened film page opens, bottom first: the Theater list
/// when it exists, then the film page over it.
fn reopen_screens(list: Option<u32>, detail: u32) -> Vec<(&'static str, u32)> {
    list.map(|loader| (FILMS, loader)).into_iter().chain([(DETAIL, detail)]).collect()
}

/// Whether a local page keeps waiting at this update: the menu has not had
/// one screen on top for [`MENU_SETTLE_UPDATES`] updates in a row, and the
/// wait has not run out.
fn menu_not_settled(waited: u32, menu: Menu) -> bool {
    waited < MENU_WAIT_UPDATES && menu.settled < MENU_SETTLE_UPDATES
}

pub(super) fn share_url(
    archive: &live_hle::films::Archive,
    location: &Location,
    game: &str,
    clip: Option<&str>,
) -> Result<String, String> {
    let row = archive.detail(game)?;
    if row.game_id != game || !row.has_host_film {
        return Err("This game has no archived film".into());
    }
    if let Some(id) = clip {
        let (owner, clip) = archive.clip(id)?;
        if owner != game || clip.id != id {
            return Err("Invalid Live clip".into());
        }
    }
    let base = location
        .live
        .server_url
        .as_deref()
        .ok_or("Missing Live server")?;
    live_proto::theater_link::web_link(base, game, clip).map_err(|e| e.to_string())
}

pub(super) fn copy(link: &str) -> Result<(), String> {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let (program, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("/usr/bin/pbcopy", &[])
    } else if cfg!(windows) {
        ("clip.exe", &[])
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        ("wl-copy", &[])
    } else {
        ("xclip", &["-selection", "clipboard"])
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not open the clipboard: {e}"))?;
    let wrote = child
        .stdin
        .take()
        .ok_or("Clipboard is unavailable")?
        .write_all(link.as_bytes());
    let status = child.wait().map_err(|e| e.to_string())?;
    wrote.map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("Could not copy the film link".into())
    }
}

fn resolve(
    location: &Location,
    account: live_proto::Xuid,
    link: &TheaterLink,
    sync: Option<&live_hle::films::sync::Sync>,
) -> Result<(Entry, api::InitialView), String> {
    let archive = live_hle::films::Archive::connect(&location.live, Some(account))?
        .ok_or("Live unavailable")?;
    let mut film = adapter::FilmMeta::try_from(archive.detail(&link.film)?)?;
    if film.archive_id() != Some(link.film.as_str()) {
        return Err("Invalid film ID in Live response".into());
    }
    let mut entry = Entry {
        film: film.clone(),
        clip: None,
        archive_clip: None,
    };
    if let Some(id) = &link.clip {
        let (game, clip) = archive.clip(id)?;
        if game != link.film || clip.id != *id || clip.from_tick >= clip.to_tick {
            return Err("Invalid clip".into());
        }
        entry.archive_clip = Some((clip.id, clip.title));
        entry.clip = Some(adapter::Clip {
            film: film.id.clone(),
            from_tick: clip.from_tick,
            to_tick: clip.to_tick,
            ..Default::default()
        });
    }
    let mut local_clip = entry.clip.clone();
    let local_library = (location.open)(&location.dir)
        .and_then(|lib| lib.list(&location.installation))
        .unwrap_or_default();
    let local = local_library.into_iter().find(|row| {
        row.live_game_id.as_deref() == Some(&link.film)
            || sync.and_then(|s| s.published(&row.id)).as_deref() == Some(&link.film)
    });
    if let Some(mut local) = local.filter(|_| entry.clip.is_none() || film.shared) {
        local.live_game_id = Some(link.film.clone());
        local.shared = film.shared;
        local.saved = film.saved;
        if let Some(clip) = &mut entry.clip {
            clip.film.clone_from(&local.id);
        }
        film = local;
    }
    // Legacy guest-clock clips need the original time map. A split film's
    // catalogue already contains its absolute tick bounds.
    if film.archive_id().is_some() && !film.shared && (link.tick.is_some() || local_clip.is_some())
    {
        local_clip = adapter::download_film(
            &archive,
            sync,
            &location.dir,
            &link.film,
            entry.clip.as_ref(),
            film.live_start_tick,
        )?;
        let path = adapter::archive_path(&location.dir, &link.film)?;
        let local = adapter::archive_entry(&path, &link.film).map_err(|e| e.to_string())?;
        film.version = Some(local.version);
        film.apply_compatibility(location.installation.compatibility(&local));
        film.length_ticks = local.meta.length_ticks;
    }
    link.check_bounds(
        film.length_ticks,
        film.scoreboard.iter().filter_map(|p| p.seat),
        local_clip.map(|c| [c.from_tick, c.to_tick]),
    )
    .map_err(|e| e.to_string())?;
    entry.film = film;
    let camera = link
        .camera
        .as_ref()
        .map(|s| serde_json::from_value(Value::String(s.clone())))
        .transpose()
        .map_err(|e| e.to_string())?;
    Ok((
        entry,
        api::InitialView {
            tick: link.tick,
            seat: link.seat,
            camera,
        },
    ))
}

impl Theater {
    fn select_link(
        &self,
        entry: Entry,
        view: api::InitialView,
        account: live_proto::Xuid,
        notice: Option<String>,
    ) {
        let mut state = self.state();
        state.account = Some(account);
        state.working = false;
        state.select(entry);
        if let Some(notice) = notice {
            state.status = notice;
        }
        state.view = Some(view);
    }

    fn queue_link(&self, link: TheaterLink, reply: Option<Reply>) -> Result<(), String> {
        let location = self
            .0
            .location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or("Theater is unavailable")?;
        let server = location
            .live
            .server_url
            .as_deref()
            .filter(|_| location.live.enabled)
            .ok_or("Sign in to the Live server")?;
        link.check_server(server).map_err(|e| e.to_string())?;
        let controller = self.state().controller;
        let (account, _) = location
            .signed_in
            .as_ref()
            .and_then(|h| h.signed_in_account(controller))
            .ok_or("Sign in to the Live server")?;
        let generation = {
            let mut state = self.state();
            state.open_generation = state.open_generation.wrapping_add(1);
            state.open_generation
        };
        let this = self.clone();
        std::thread::Builder::new()
            .name("h2-theater-link".into())
            .spawn(move || {
                match resolve(
                    &location,
                    account,
                    &link,
                    this.sync().filter(|s| s.account() == account).as_ref(),
                ) {
                    Ok((entry, view)) => {
                        let mut ready = this.0.link.lock().unwrap_or_else(|e| e.into_inner());
                        if this.state().open_generation == generation {
                            *ready = Some(Ready {
                                entry,
                                view,
                                account,
                                controller,
                                generation,
                                reply,
                                local: false,
                                waited: 0,
                                menu: Menu::default(),
                                notice: None,
                            });
                        } else if let Some(reply) = reply {
                            let _ = reply.send(Err("Theater selection changed".into()));
                        }
                    }
                    Err(error) => {
                        if let Some(reply) = reply {
                            let _ = reply.send(Err(error));
                        } else {
                            eprintln!("theater: {error}");
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The film page of the film watched in the running game, opened from
    /// the menu the viewer returns to after it: left, diverged, or never
    /// started (`why` says which failed, and is the page's status line).
    /// The page opens at the next menu update, as a linked film page does.
    /// The entry the menu's Watch played is the page's (a Live film is not
    /// in the local library); with none, the film is found in the library.
    /// A reason the session recorded (`return_notice`) is used when `why`
    /// gives none.
    pub fn reopen_film(&self, film: &str, why: Option<&str>) {
        let notice = why.map(str::to_owned).or_else(|| self.state().return_notice.take());
        let location = self
            .0
            .location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(location) = location else { return };
        // The film's own id, or a Live film's (its page names the game, the
        // film file its own id).
        let watched = self
            .0
            .watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .filter(|e| e.film.id == film || e.film.archive_id().is_some());
        let entry = if let Some(entry) = watched {
            entry
        } else {
            let found = (location.open)(&location.dir)
                .and_then(|library| library.list(&location.installation))
                .map(|films| films.into_iter().find(|f| f.id == film));
            match found {
                Ok(Some(film)) => Entry {
                    film,
                    clip: None,
                    archive_clip: None,
                },
                Ok(None) => {
                    eprintln!("theater: film {film} is not in the library; no film page to open");
                    return;
                }
                Err(error) => {
                    eprintln!("theater: could not reopen the film page: {error}");
                    return;
                }
            }
        };
        let (controller, account, generation) = {
            let mut state = self.state();
            state.open_generation = state.open_generation.wrapping_add(1);
            (
                state.controller,
                state.account.unwrap_or(live_proto::Xuid(0)),
                state.open_generation,
            )
        };
        *self.0.link.lock().unwrap_or_else(|e| e.into_inner()) = Some(Ready {
            entry,
            view: api::InitialView::default(),
            account,
            controller,
            generation,
            reply: None,
            local: true,
            waited: 0,
            menu: Menu::default(),
            notice,
        });
    }

    pub(super) fn pump_external_link(&self) {
        let mut pending = EXTERNAL.lock().unwrap_or_else(|e| e.into_inner());
        if pending.is_none() {
            return;
        }
        // Finder may also launch the game before its account signs in.
        let controller = self.state().controller;
        if !self
            .0
            .location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|l| l.signed_in.as_ref()?.signed_in_account(controller))
            .is_some()
        {
            return;
        }
        if let Some(link) = pending.take()
            && let Err(error) = self.queue_link(link, None)
        {
            eprintln!("theater: {error}");
        }
    }

    fn publish_link(&self, request: Publish) -> Result<Value, String> {
        let location = self
            .0
            .location
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or("Theater is unavailable")?;
        let controller = self.state().controller;
        let account = location
            .signed_in
            .as_ref()
            .and_then(|hle| hle.signed_in_account(controller))
            .map(|(id, _)| id)
            .ok_or("Sign in to share a film")?;
        let archive = live_hle::films::Archive::connect(&location.live, Some(account))?
            .ok_or("Live unavailable")?;
        let sync = self.sync().filter(|s| s.account() == account);
        let library = film::Library::open(&location.dir).map_err(|e| e.to_string())?;
        let remote = if let Some(id) = request.id.strip_prefix("live:") {
            id.to_owned()
        } else if let Some(id) = sync.as_ref().and_then(|s| s.published(&request.id)) {
            id
        } else if let Ok(entry) = library.get(&request.id) {
            if let Some(game) = entry.game_id {
                game
            } else {
                let sync = sync.as_ref().ok_or("Film library sync is unavailable")?;
                sync.enqueue(&request.id, &entry.path)?;
                sync.share(&request.id, request.visibility)?
            }
        } else {
            request.id
        };
        let detail = archive.detail(&remote)?;
        if detail.shared && detail.visibility != Some(request.visibility) {
            archive.set_visibility(&remote, request.visibility)?;
        }
        let clip = match request.clip {
            Some(ShareClip::Id(id)) => Some(id),
            Some(ShareClip::Bounds([start, end])) => Some(archive.share_clip(&remote, start, end)?),
            None => None,
        };
        let url = share_url(&archive, &location, &remote, clip.as_deref())?;
        Ok(serde_json::json!({"film":remote, "url":url}))
    }

    pub(super) fn link_method(&self, agent: &agent::Agent) {
        let theater = self.clone();
        agent.add_game_method(api::SHARE, agent::Access::Act, move |value, ctx| {
            let request: Publish = serde_json::from_value(value)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
            if !live_proto::game::valid_id(request.id.strip_prefix("live:").unwrap_or(&request.id))
            {
                return Err(RpcError::new(code::INVALID_PARAMS, "Invalid film ID"));
            }
            if request.clip.as_ref().is_some_and(|c| match c {
                ShareClip::Id(id) => !live_proto::game::valid_id(id),
                ShareClip::Bounds([a, b]) => a >= b,
            }) {
                return Err(RpcError::new(code::INVALID_PARAMS, "Invalid clip"));
            }
            let (tx, rx) = mpsc::channel();
            let theater = theater.clone();
            std::thread::Builder::new()
                .name("h2-theater-share".into())
                .spawn(move || {
                    let _ = tx.send(theater.publish_link(request));
                })
                .map_err(|e| RpcError::new(code::UNAVAILABLE, e.to_string()))?;
            defer_reply(ctx, rx);
            Ok(Value::Null)
        });
        let theater = self.clone();
        agent.add_game_method(api::OPEN, agent::Access::Act, move |value, ctx| {
            let request: api::Open = serde_json::from_value(value)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, format!("url: {e}")))?;
            let link = request
                .url
                .parse::<TheaterLink>()
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
            let (tx, rx) = mpsc::channel();
            theater
                .queue_link(link, Some(tx))
                .map_err(|e| RpcError::new(code::UNAVAILABLE, e))?;
            defer_reply(ctx, rx);
            Ok(Value::Null)
        });
    }
}

pub(super) struct Driver(pub Theater);
impl Feature for Driver {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("theater-link", "Open a linked film page")
            .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        let theater = self.0.clone();
        b.raw()
            .before(&points::UI_UPDATE, "open a linked film page", move |c| {
                let ready = theater
                    .0
                    .link
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                let Some(ready) = ready else {
                    return Ok(Flow::Continue);
                };
                let theater = theater.clone();
                c.answer_async(move |cx| {
                    Box::pin(async move {
                        let elapsed = cx.arg(0)?;
                        let account = theater
                            .0
                            .location
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .as_ref()
                            .and_then(|l| {
                                l.signed_in.as_ref()?.signed_in_account(ready.controller)
                            });
                        if ready.local {
                            let last = ready.menu;
                            let menu = cx.read(|m| {
                                let m = ReadGm(m);
                                let main_menu = crate::level_boot::active_mode(&m)
                                    == Some(crate::level_boot::MODE_MAINMENU);
                                let top = ui_watch::read(&m).top.map(|t| (t.id, t.screen));
                                last.after(main_menu, top)
                            });
                            if menu_not_settled(ready.waited, menu) {
                                // The update goes on first, with the page
                                // not yet pending: the call below passes
                                // through this very hook, which would take
                                // a pending page again and call itself
                                // without end.
                                let out = cx
                                    .call(GuestCall::new(points::UI_UPDATE.base).args([elapsed]))
                                    .await?;
                                {
                                    let mut slot =
                                        theater.0.link.lock().unwrap_or_else(|e| e.into_inner());
                                    if slot.is_none() {
                                        *slot = Some(Ready {
                                            waited: ready.waited + 1,
                                            menu,
                                            ..ready
                                        });
                                    }
                                }
                                return Ok(Ret::new(out.eax, 4));
                            }
                        }
                        let result = if (!ready.local && account.map(|(id, _)| id) != Some(ready.account))
                            || theater.state().open_generation != ready.generation
                        {
                            Err("Theater account or selection changed".into())
                        } else if let Some(detail) = list_class::loader_of(DETAIL) {
                            // A film page of this machine's library is
                            // reached through the Theater list, as the
                            // menu's own Theater row reaches it: the list
                            // is under the page, and B goes back to it.
                            let list = if ready.local { list_class::loader_of(FILMS) } else { None };
                            if list.is_some() {
                                let player = theater.state().player.clone();
                                theater.open(ready.controller, player);
                            }
                            theater.select_link(ready.entry, ready.view, ready.account, ready.notice);
                            for (_, loader) in reopen_screens(list, detail) {
                                cx.call(
                                    GuestCall::new(points::OPEN_SCREEN.base)
                                        .args([1 << ready.controller, loader]),
                                )
                                .await?;
                            }
                            let shown = cx.read(|m| ui_watch::read(&ReadGm(m)));
                            if shown.top.is_some_and(|s| s.class_name == Some(DETAIL)) {
                                Ok(ui_watch::screen_json(
                                    std::time::Duration::ZERO,
                                    shown.top,
                                    shown.dialog,
                                ))
                            } else {
                                Err("Could not open the Theater film page".into())
                            }
                        } else {
                            Err("Theater film page is unavailable".into())
                        };
                        if let Some(reply) = ready.reply {
                            let _ = reply.send(result);
                        } else if let Err(e) = result {
                            eprintln!("theater: {e}");
                        }
                        let out = cx
                            .call(GuestCall::new(points::UI_UPDATE.base).args([elapsed]))
                            .await?;
                        Ok(Ret::new(out.eax, 4))
                    })
                })
            })
            // A pending Live join goes first; a linked film page waits a frame.
            .after_feature("live-join-link")
            .optional();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_page_waits_for_a_settled_menu_and_not_for_ever() {
        let (loading, main) = (Some((7, 0x100)), Some((1, 0x200)));
        // Updates of the main menu with one screen on top, from the first.
        let steady = |n: u32, top| (0..n).fold(Menu::default(), |m, _| m.after(true, top));
        assert!(menu_not_settled(0, Menu::default()));
        assert!(menu_not_settled(0, steady(MENU_SETTLE_UPDATES - 1, main)));
        assert!(!menu_not_settled(0, steady(MENU_SETTLE_UPDATES, main)), "settled: open the page");
        // A film stopped at its start: the film's game holds its screen
        // for longer than the settle count while the menu loads. Its
        // updates count for nothing, and the menu's screen settles anew.
        let film = (0..2 * MENU_SETTLE_UPDATES).fold(Menu::default(), |m, _| m.after(false, loading));
        assert_eq!(film.settled, 0, "not the main menu");
        assert!(menu_not_settled(2 * MENU_SETTLE_UPDATES, film));
        let first = film.after(true, main);
        assert_eq!(first.settled, 1);
        assert!(menu_not_settled(2 * MENU_SETTLE_UPDATES + 1, first), "the menu's screen just came up");
        // A screen pushed over the menu's own starts the count again.
        let pushed = steady(MENU_SETTLE_UPDATES - 1, main).after(true, loading);
        assert_eq!(pushed.settled, 1);
        assert_eq!(pushed.after(true, None).settled, 0, "no screen up");
        assert!(!menu_not_settled(MENU_WAIT_UPDATES, Menu::default()), "give up and try");
    }

    #[test]
    fn a_reopened_film_page_has_the_theater_list_under_it() {
        assert_eq!(reopen_screens(Some(1), 2), [(FILMS, 1), (DETAIL, 2)]);
        assert_eq!(reopen_screens(None, 2), [(DETAIL, 2)], "a linked page opens alone");
    }

    #[test]
    fn linked_selection_focuses_watch_and_waits_for_a_press() {
        let theater = Theater::default();
        let entry = Entry {
            film: adapter::FilmMeta {
                id: "live:example".into(),
                map: "lockout".into(),
                source: adapter::Source::Host,
                compatibility: api::CompatibilityState::Playable,
                live_game_id: Some("example".into()),
                ..Default::default()
            },
            clip: None,
            archive_clip: None,
        };
        theater.select_link(
            entry,
            api::InitialView {
                tick: Some(42),
                seat: Some(1),
                camera: Some(api::CameraMode::Orbit),
            },
            live_proto::Xuid(42),
            None,
        );
        let screen = super::super::pages::spec(&theater, false);
        assert_eq!(screen.name, DETAIL);
        assert_eq!(screen.rows[screen.focus].label, "Watch");
        let labels = screen
            .rows
            .iter()
            .map(|r| r.label.as_str())
            .collect::<Vec<_>>();
        assert!(labels.contains(&"Share") && labels.contains(&"Copy Link"));
        assert!(!theater.playing());
        assert_eq!(theater.state().view.unwrap().tick, Some(42));
    }

    /// All three run before UI_UPDATE; with no order between them the plan
    /// refused and every boot with Theater and Live stopped at the plan.
    #[test]
    fn the_film_link_join_link_and_lab_consent_plan_together_at_ui_update() {
        let live = live_hle::Hle::new(live_hle::LiveConfig {
            enabled: true,
            server_url: Some("https://live.example".into()),
            ..Default::default()
        })
        .unwrap();
        let mut features = hooks::Registry::new(hooks::Switches::defaults());
        crate::join_link::install(&mut features, Some(live), false, None).unwrap();
        crate::lab_takeover::install(&mut features, &agent::Agent::new(), true).unwrap();
        features
            .add(&super::Driver(crate::theater_ui::Theater::default()))
            .unwrap();
        let plan = features.build(&hooks::Base).unwrap().text();
        assert!(
            plan.contains("open a linked film page")
                && plan.contains("process a Live join link")
                && plan.contains("open or expire a lab request"),
            "{plan}"
        );
    }
}
