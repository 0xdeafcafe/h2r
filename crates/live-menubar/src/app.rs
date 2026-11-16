//! The tray itself: a winit event loop with no windows and no Dock icon
//! (an accessory app on macOS), a tray icon, and its menu drawn from
//! [`crate::menu::model`].
//!
//! One worker thread does everything that can block: it polls a
//! [`Snapshot`] every few seconds and runs the actions the menu asks for,
//! one at a time, then polls again at once. It sends snapshots to the UI
//! through the event loop; the UI only redraws.
//!
//! With `[update]`, a second thread runs update checks
//! ([`crate::update`]), one at a time, on its timer or Update Now; the
//! poller shows its progress, and it asks the UI to re-execute the menu
//! bar when it replaced the `h2` this is.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::WindowId;

use crate::actions::{Ctx, Desktop, Outcome, busy_label};
use crate::config::Config;
use crate::launchd::Services;
use crate::menu::{Entry, ItemId, Overall, model, overall, shape};
use crate::status::{Backend, Snapshot, collect, unix_now};
use crate::update::{self, State, UpdateView, Updater};

pub const POLL: Duration = Duration::from_secs(4);

#[derive(Debug)]
enum UserEvent {
    Snapshot(Box<Snapshot>),
    Menu(ItemId),
    Exit,
    /// `h2` was replaced: run the new one.
    Reexec,
}

/// What the menu asks of the update thread.
#[derive(Debug)]
enum UpdateCmd {
    Now,
    TogglePause,
}

/// The menu's toolkit items, by id, for updates in place.
enum Widget {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

struct Tray {
    icon: TrayIcon,
    widgets: HashMap<ItemId, Widget>,
    shape: Vec<Option<ItemId>>,
    state: Overall,
}

struct App {
    cfg: Config,
    tray: Option<Tray>,
    snapshot: Snapshot,
    actions: mpsc::Sender<ItemId>,
    updates: Option<mpsc::Sender<UpdateCmd>>,
}

fn icon(state: Overall) -> Result<Icon> {
    Icon::from_rgba(
        crate::icon::rgba(state),
        crate::icon::SIZE,
        crate::icon::SIZE,
    )
    .context("building the tray icon")
}

fn build_menu(entries: &[Entry]) -> Result<(Menu, HashMap<ItemId, Widget>)> {
    let menu = Menu::new();
    let mut widgets = HashMap::new();
    for e in entries {
        match e {
            Entry::Separator => menu.append(&PredefinedMenuItem::separator())?,
            Entry::Item(i) => {
                let w = match i.checked {
                    Some(on) => Widget::Check(CheckMenuItem::with_id(
                        i.id.key(),
                        &i.label,
                        i.enabled,
                        on,
                        None,
                    )),
                    None => Widget::Plain(MenuItem::with_id(i.id.key(), &i.label, i.enabled, None)),
                };
                let item: &dyn IsMenuItem = match &w {
                    Widget::Plain(m) => m,
                    Widget::Check(m) => m,
                };
                menu.append(item)?;
                widgets.insert(i.id, w);
            }
        }
    }
    Ok((menu, widgets))
}

impl App {
    fn create_tray(&mut self) -> Result<()> {
        let entries = model(&self.snapshot, &self.cfg);
        let (menu, widgets) = build_menu(&entries)?;
        let state = overall(&self.snapshot);
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon_templated(icon(state)?)
            .with_tooltip("h2 Live")
            .build()
            .context("creating the tray icon")?;
        self.tray = Some(Tray {
            icon,
            widgets,
            shape: shape(&entries),
            state,
        });
        Ok(())
    }

    fn redraw(&mut self) -> Result<()> {
        let Some(tray) = &mut self.tray else {
            return Ok(());
        };
        let entries = model(&self.snapshot, &self.cfg);
        let state = overall(&self.snapshot);
        if state != tray.state {
            tray.icon.set_icon_templated(Some(icon(state)?))?;
            tray.state = state;
        }
        let new_shape = shape(&entries);
        if new_shape != tray.shape {
            let (menu, widgets) = build_menu(&entries)?;
            tray.icon.set_menu(Some(Box::new(menu)));
            tray.widgets = widgets;
            tray.shape = new_shape;
            return Ok(());
        }
        for e in &entries {
            let Entry::Item(i) = e else { continue };
            match tray.widgets.get(&i.id) {
                Some(Widget::Plain(m)) => {
                    if m.text() != i.label {
                        m.set_text(&i.label);
                    }
                    if m.is_enabled() != i.enabled {
                        m.set_enabled(i.enabled);
                    }
                }
                Some(Widget::Check(m)) => {
                    if m.text() != i.label {
                        m.set_text(&i.label);
                    }
                    if m.is_enabled() != i.enabled {
                        m.set_enabled(i.enabled);
                    }
                    let on = i.checked.unwrap_or(false);
                    if m.is_checked() != on {
                        m.set_checked(on);
                    }
                }
                None => {}
            }
        }
        Ok(())
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, _: &ActiveEventLoop, cause: StartCause) {
        // tray-icon wants the event loop running before the icon exists
        // on macOS.
        if cause == StartCause::Init
            && let Err(e) = self.create_tray()
        {
            eprintln!("h2 live menubar: {e:#}");
        }
    }

    fn resumed(&mut self, _: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Snapshot(s) => {
                self.snapshot = *s;
                if let Err(e) = self.redraw() {
                    eprintln!("h2 live menubar: {e:#}");
                }
            }
            UserEvent::Menu(ItemId::Quit) => el.exit(),
            UserEvent::Menu(id) => {
                // The checkbox flips itself on click; put it back until the
                // worker says what launchd did.
                let _ = self.redraw();
                if id.is_update() {
                    if let Some(tx) = &self.updates {
                        let _ = tx.send(match id {
                            ItemId::UpdateNow => UpdateCmd::Now,
                            _ => UpdateCmd::TogglePause,
                        });
                    }
                } else {
                    let _ = self.actions.send(id);
                }
            }
            UserEvent::Exit => el.exit(),
            UserEvent::Reexec => reexec(el),
        }
    }
}

/// Replaces this process with the (new) program at the same path.
fn reexec(el: &ActiveEventLoop) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        if let Ok(exe) = std::env::current_exe() {
            let err = std::process::Command::new(exe)
                .args(std::env::args_os().skip(1))
                .exec();
            eprintln!("h2 live menubar: re-executing: {err}");
        }
    }
    // Could not: quit, and launchd or the user starts the new one.
    el.exit();
}

/// Whether `a` and `b` are the same file.
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The update thread: a check now, then one every interval or on Update
/// Now; never two at once.
fn updater(
    cfg: Config,
    services: Arc<dyn Services>,
    view: Arc<Mutex<UpdateView>>,
    commands: mpsc::Receiver<UpdateCmd>,
    ui: EventLoopProxy<UserEvent>,
) {
    let Some(ucfg) = cfg.update.clone() else {
        return;
    };
    let log = cfg.logs_dir.join("update.log");
    let git = update::SystemGit {
        repo: ucfg.repo.clone(),
        log: log.clone(),
    };
    let builder = update::SystemBuilder {
        repo: ucfg.repo.clone(),
        target_dir: ucfg.target_dir(),
        log: log.clone(),
        cargo: ucfg.cargo.clone(),
        bun: ucfg.bun.clone(),
    };
    let interval = Duration::from_secs(ucfg.interval_minutes.max(1) * 60);
    let me: Option<PathBuf> = std::env::current_exe().ok();
    let mut next = Instant::now();
    loop {
        let wait = next.saturating_duration_since(Instant::now());
        let force = match commands.recv_timeout(wait) {
            Ok(UpdateCmd::Now) => true,
            Ok(UpdateCmd::TogglePause) => {
                let path = ucfg.state_file();
                let mut state = State::load(&path);
                state.paused = !state.paused;
                let _ = state.save(&path);
                view.lock().unwrap().paused = state.paused;
                continue;
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        update::log_line(
            &log,
            &format!("== check at {} (force: {force})", unix_now()),
        );
        let progress = |what: &str| {
            view.lock().unwrap().working = Some(what.into());
            update::log_line(&log, &format!("-- {what}"));
        };
        let report = Updater {
            cfg: &ucfg,
            git: &git,
            builder: &builder,
            services: services.as_ref(),
            server: &cfg.server,
            data_dir: &cfg.data_dir,
            progress: &progress,
            now: unix_now(),
        }
        .check(force);
        update::log_line(&log, &format!("== {:?}", report.outcome));
        {
            let state = State::load(&ucfg.state_file());
            let mut v = view.lock().unwrap();
            *v = UpdateView::from_state(&state);
            v.outcome = Some(report.outcome);
        }
        let installed_h2 = ucfg.bin_dir().join("h2");
        if report.h2_changed && me.as_deref().is_some_and(|me| same_file(me, &installed_h2)) {
            let _ = ui.send_event(UserEvent::Reexec);
        }
        next = Instant::now() + interval;
    }
}

/// The worker: polls, and runs actions between polls.
#[allow(clippy::too_many_arguments)]
fn worker(
    cfg: Config,
    services: Arc<dyn Services>,
    backend: Arc<dyn Backend>,
    desktop: Arc<dyn Desktop>,
    exe: std::path::PathBuf,
    actions: mpsc::Receiver<ItemId>,
    ui: EventLoopProxy<UserEvent>,
    updates: Option<Arc<Mutex<UpdateView>>>,
) {
    let mut message: Option<String> = None;
    let send = |s: Snapshot| ui.send_event(UserEvent::Snapshot(Box::new(s))).is_ok();
    loop {
        let mut snap = collect(&cfg, services.as_ref(), backend.as_ref());
        snap.message = message.clone();
        snap.update = updates.as_ref().map(|v| v.lock().unwrap().clone());
        if !send(snap.clone()) {
            return;
        }
        let id = match actions.recv_timeout(POLL) {
            Ok(id) => id,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if let Some(label) = busy_label(id) {
            snap.busy = Some(label.into());
            snap.message = None;
            send(snap);
        }
        let ctx = Ctx {
            cfg: &cfg,
            services: services.as_ref(),
            backend: backend.as_ref(),
            desktop: desktop.as_ref(),
            exe: exe.clone(),
        };
        message = match ctx.perform(id) {
            Ok(Outcome::Done(m)) => m,
            Ok(Outcome::Quit) => {
                let _ = ui.send_event(UserEvent::Exit);
                return;
            }
            Err(e) => Some(format!("Failed: {}", one_line(&e))),
        };
    }
}

fn one_line(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    s.lines().next().unwrap_or("").chars().take(120).collect()
}

/// Runs the menu bar until Quit.
pub fn run(
    cfg: Config,
    services: Arc<dyn Services>,
    backend: Arc<dyn Backend>,
    desktop: Arc<dyn Desktop>,
) -> Result<()> {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false);
    }
    let event_loop = builder.build().context("starting the event loop")?;

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if let Some(id) = ItemId::from_key(e.id().0.as_str()) {
            let _ = proxy.send_event(UserEvent::Menu(id));
        }
    }));

    let (tx, rx) = mpsc::channel();
    let exe = std::env::current_exe().context("finding this program")?;

    let mut update_tx = None;
    let view = cfg.update.as_ref().map(|u| {
        Arc::new(Mutex::new(UpdateView::from_state(&State::load(
            &u.state_file(),
        ))))
    });
    if let Some(view) = &view {
        let (utx, urx) = mpsc::channel();
        update_tx = Some(utx);
        let (cfg, services, view) = (cfg.clone(), services.clone(), view.clone());
        let proxy = event_loop.create_proxy();
        std::thread::Builder::new()
            .name("menubar-update".into())
            .spawn(move || updater(cfg, services, view, urx, proxy))
            .context("starting the updater")?;
    }

    let proxy = event_loop.create_proxy();
    let worker_cfg = cfg.clone();
    std::thread::Builder::new()
        .name("menubar-poll".into())
        .spawn(move || worker(worker_cfg, services, backend, desktop, exe, rx, proxy, view))
        .context("starting the poller")?;

    let mut app = App {
        cfg,
        tray: None,
        snapshot: Snapshot::initial(),
        actions: tx,
        updates: update_tx,
    };
    event_loop
        .run_app(&mut app)
        .context("running the menu bar")?;
    Ok(())
}
