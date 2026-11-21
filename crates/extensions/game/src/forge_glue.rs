//! Local Forge session lifetime. The player and agent share one document and
//! apply queue. A map activation invalidates handles, outstanding calls and
//! any map file still being read by the background worker.
mod editing;
pub mod gizmo;

use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};

use agent::forge::local::{self, LocalApi, ProposalReply};
use agent::sim::calls;
use forge_doc::{Entry, ForgeDoc, Op, Sha256};
use guest_layout::game::{GameGlobals, GameTime};
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use serde_json::{Value, json};

use crate::forge_controls as controls;
use crate::forge_map::{self, Loaded, Opened};
use crate::ui_kit::spec::ScreenSpec;
use editing::Editor;

type Shared = Arc<Mutex<Driver>>;
type MapResult = Result<Opened, String>;

static CURRENT: Mutex<Weak<Mutex<Driver>>> = Mutex::new(Weak::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn requested() -> Result<bool, String> {
    Ok(hooks::Switches::from_env()?.feature("forge", false))
}

fn current() -> Option<Shared> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).upgrade()
}

/// The window draws this page over the current world frame.
pub fn page() -> Option<ScreenSpec> {
    let driver = current()?;
    let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
    driver.editor.as_ref()?.page()
}

/// World-space axes projected through the render frame's camera.
pub fn gizmo() -> Option<gizmo::Axes> {
    let frame = {
        let driver = current()?;
        let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
        driver.editor.as_ref()?.ghost()?
    };
    gizmo::project(frame)
}

/// A mouse click is delivered at the same game boundary as a controller press.
pub fn click(row: usize) -> bool {
    let Some(driver) = current() else {
        return false;
    };
    let mut driver = driver.lock().unwrap_or_else(|e| e.into_inner());
    driver.editor.as_mut().is_some_and(|e| e.click(row))
}

fn admitted(m: &dyn engine::GuestMemory, generation: u64) -> bool {
    GENERATION.load(Ordering::Acquire) == generation && forge_map::admitted(m)
}

struct Driver {
    calls: calls::Shared,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    data: PathBuf,
    loaded: Option<Loaded>,
    opening: Option<Receiver<MapResult>>,
    editor: Option<Editor>,
    reason: String,
}

impl Driver {
    fn reset(&mut self) {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        self.editor = None;
        self.opening = None;
        self.loaded = None;
        self.reason = "Waiting for a local multiplayer map.".into();
        controls::publish(false, false, false, None);
    }

    fn open(&mut self, loaded: &Loaded) -> Result<(), String> {
        let vfs = self
            .vfs
            .clone()
            .ok_or("Forge needs the mounted map files.")?;
        let (send, recv) = mpsc::sync_channel(1);
        let source = loaded.clone();
        background::spawn("forge-map", move || {
            let _ = send.send(forge_map::open(&vfs, &source));
        })
        .map_err(|e| e.to_string())?;
        self.opening = Some(recv);
        self.reason = "Reading the Forge palette.".into();
        Ok(())
    }

    fn tick(&mut self, m: &dyn engine::GuestMemory) {
        if self
            .loaded
            .as_ref()
            .is_some_and(|loaded| !loaded.matches(m))
        {
            self.reset();
        }
        if self.loaded.is_none() {
            if !forge_map::admitted(m) {
                return;
            }
            let loaded = match Loaded::read(m, false) {
                Ok(loaded) => loaded,
                Err(reason) => {
                    self.reason = reason;
                    return;
                }
            };
            let result = self.open(&loaded);
            self.loaded = Some(loaded);
            if let Err(e) = result {
                self.reason = e;
            }
        }
        let loaded = self.loaded.as_ref().expect("read above");
        if let Some(recv) = &self.opening {
            let result = match recv.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Err("The Forge map reader stopped.".into()),
            };
            self.opening = None;
            match result {
                Ok(opened) => {
                    self.editor = Some(Editor::new(
                        opened,
                        &self.data,
                        loaded.scenario,
                        self.calls.clone(),
                    ));
                }
                Err(error) => {
                    eprintln!("forge: {error}");
                    self.reason = error;
                    return;
                }
            }
        }
        if let Some(e) = self.editor.as_mut()
            && let Some(tick) = GameTime::current(m)
                .ok()
                .flatten()
                .and_then(|t| t.tick(m).ok())
        {
            let bsp = GameGlobals::current(m)
                .ok()
                .flatten()
                .and_then(|g| g.active_bsp(m).ok())
                .and_then(|i| usize::try_from(i).ok());
            e.tick(tick, bsp);
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.reset();
        controls::forget_park();
    }
}

struct Hooks(Shared);

impl Feature for Hooks {
    fn manifest(&self) -> Manifest {
        Manifest::new("forge", "Local Forge monitor and Objects tool")
            .off_by_default()
            .doc("docs/game/forge-design.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        controls::camera_hooks(b)?;
        let driver = self.0.clone();
        b.game().on_start("reset the Forge document", move |_| {
            controls::forget_park();
            driver.lock().unwrap_or_else(|e| e.into_inner()).reset();
        });
        let driver = self.0.clone();
        b.game().on_end("release the Forge monitor", move |o| {
            let m = o.guest();
            if GameGlobals::current(&*m)
                .ok()
                .flatten()
                .and_then(|g| g.game_ended(&*m).ok())
                .is_some_and(|v| v != 0)
            {
                driver.lock().unwrap_or_else(|e| e.into_inner()).reset();
            }
        });
        let driver = self.0.clone();
        b.frame().on_tick("apply Forge input", move |o| {
            driver
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .tick(&*o.guest());
        });
        let driver = self.0.clone();
        b.snapshot(SnapshotPolicy::forbid_while(move || {
            let driver = driver.lock().unwrap_or_else(|e| e.into_inner());
            (driver.opening.is_some() || driver.editor.is_some())
                .then(|| "The local Forge session owns a live edit log.".into())
        }));
        Ok(())
    }
}

pub fn install(
    registry: &mut hooks::Registry,
    agent: Option<&crate::agent_glue::AgentGlue>,
    vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    data: &Path,
) -> anyhow::Result<()> {
    let calls = if let Some(sim) = agent.and_then(|a| a.sim.as_ref()) {
        sim.calls().unwrap_or_else(calls::shared)
    } else {
        let calls = calls::shared();
        registry
            .add(&calls::CallsFeature(calls.clone()))
            .map_err(anyhow::Error::msg)?;
        calls
    };
    let driver = Arc::new(Mutex::new(Driver {
        calls,
        vfs,
        data: data.into(),
        loaded: None,
        opening: None,
        editor: None,
        reason: "Waiting for a local multiplayer map.".into(),
    }));
    if registry
        .add(&Hooks(driver.clone()))
        .map_err(anyhow::Error::msg)?
    {
        controls::publish(false, false, false, None);
        *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Arc::downgrade(&driver);
        if let Some(a) = agent {
            local::install(&a.agent, Backend(driver));
        }
    }
    Ok(())
}

struct Backend(Shared);

impl Backend {
    fn read<T>(&self, f: impl FnOnce(&Editor) -> T) -> Result<T, String> {
        let driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        driver
            .editor
            .as_ref()
            .map(f)
            .ok_or_else(|| driver.reason.clone())
    }
}

impl LocalApi for Backend {
    fn status(&self) -> Result<Value, String> {
        Ok(self
            .read(Editor::status)
            .unwrap_or_else(|reason| json!({"session":"inactive","reason":reason})))
    }

    fn document(&self) -> Result<ForgeDoc, String> {
        self.read(|e| e.session.document().clone())
    }

    fn log(&self, since: u32) -> Result<Vec<Entry>, String> {
        self.read(|e| {
            e.session
                .log()
                .iter()
                .filter(|entry| entry.seq > since)
                .cloned()
                .collect()
        })
    }

    fn input(&self, m: &dyn engine::GuestMemory, input: forge::tool::Input) -> Result<(), String> {
        let driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if driver.editor.is_none()
            || !driver
                .loaded
                .as_ref()
                .is_some_and(|loaded| loaded.matches(m))
        {
            return Err("Forge input requires the active local session.".into());
        }
        controls::enqueue(input)
    }

    fn propose(&self, tick: u32, op: Op, was: Option<Sha256>) -> Result<ProposalReply, String> {
        let mut driver = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = driver.editor.as_mut() else {
            return Err(driver.reason.clone());
        };
        e.propose(tick, op, was)
    }
}
