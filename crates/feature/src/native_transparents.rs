//! Marks the guest's sorted geometry entries for the resolved indexed-draw
//! path. The guest runs each handler, including its state and texture setup.
//! The two stack frames identify the entry while the indexed call is live.
//! Push-buffer work and other kinds of handlers keep their ordinary path.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::{memory::GuestMemory, points};
use native_render::camera::GuestRead;
use native_render::transparent::{self, Entry, Source};

use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-transparents";
pub const SWITCH: &str = "impl.render.native_transparents";

pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, false)
}

static ACTIVATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MapGeneration {
    activation: u64,
    globals: u32,
    bsp: u32,
    bsp_index: u16,
}

impl MapGeneration {
    fn read(mem: &dyn GuestRead) -> Option<Self> {
        let globals = mem.read_u32(points::GAME_GLOBALS.base)?;
        let bsp = mem.read_u32(crate::native_world::ACTIVE_BSP)?;
        if globals == 0 || bsp == 0 {
            return None;
        }
        let index = (mem.read_u32(globals)? >> 16) as i16;
        Some(Self {
            activation: ACTIVATION.load(Ordering::Acquire),
            globals,
            bsp,
            bsp_index: u16::try_from(index).ok()?,
        })
    }
}

#[derive(Clone, Copy)]
struct Frame {
    esp: u32,
    return_to: u32,
}

impl Frame {
    fn read(mem: &dyn GuestRead, esp: u32) -> Option<Self> {
        let return_to = mem.read_u32(esp)?;
        (return_to != 0).then_some(Self { esp, return_to })
    }

    fn contains(self, mem: &dyn GuestRead, child_esp: u32) -> bool {
        child_esp < self.esp && mem.read_u32(self.esp) == Some(self.return_to)
    }
}

#[derive(Clone, Copy)]
struct Scene {
    esp: u32,
    frame: Option<Frame>,
    map: MapGeneration,
}

#[derive(Clone, Copy)]
struct Handler {
    frame: Frame,
    data: u32,
    entry: Entry,
}

#[derive(Default)]
pub struct Scope {
    scene: Option<Scene>,
    handler: Option<Handler>,
}

impl Scope {
    pub fn begin_view(&mut self, mem: &dyn GuestRead, esp: u32) {
        self.scene = MapGeneration::read(mem).map(|map| Scene { esp, frame: None, map });
        self.handler = None;
    }

    pub fn begin_entry(&mut self, mem: &dyn GuestRead, esp: u32, objects: bool) {
        if let Some(scene) = &mut self.scene
            && scene.frame.is_none()
            && esp < scene.esp
        {
            // The return observer's continuation replaces the original
            // return slot after begin_view, before the body starts.
            scene.frame = Frame::read(mem, scene.esp);
        }
        self.handler = self.read_handler(mem, esp, objects);
    }

    fn read_handler(&self, mem: &dyn GuestRead, esp: u32, objects: bool) -> Option<Handler> {
        let scene = self.scene?;
        let frame = Frame::read(mem, esp)?;
        // The next function after the sorted-list dispatch is the effects
        // allocator. An invocation from another caller has no sorted slot.
        const NEXT_AFTER_SORTED_DISPATCH: u32 = 0x0001_e4e0;
        if !scene.frame?.contains(mem, esp)
            || !(points::SCENE_TRANSPARENTS.base..NEXT_AFTER_SORTED_DISPATCH).contains(&frame.return_to)
        {
            return None;
        }
        let data = mem.read_u32(esp.checked_add(4)?)?;
        let entry = transparent::read_entry(mem, data)?;
        if !objects && matches!(entry.source, Source::Object(_)) {
            return None;
        }
        Some(Handler { frame, data, entry })
    }

    /// Returns the exact sorted part only while its call frame survives.
    /// The dispatch reuses the frame for the next handler and always writes
    /// that handler's own entry pointer to its argument slot.
    pub fn indexed_entry(&self, mem: &dyn GuestRead, esp: u32) -> Option<(MapGeneration, Entry)> {
        let scene = self.scene?;
        let handler = self.handler?;
        if !scene.frame?.contains(mem, handler.frame.esp)
            || !handler.frame.contains(mem, esp)
            || mem.read_u32(handler.frame.esp.checked_add(4)?) != Some(handler.data)
            || MapGeneration::read(mem) != Some(scene.map)
        {
            return None;
        }
        let live = transparent::read_entry(mem, handler.data)?;
        (live == handler.entry).then_some((scene.map, live))
    }
}

thread_local! {
    // D3D resolves the indexed API call on the same guest execution thread
    // as the hook. The GPU worker receives only immutable resolved data.
    static SCOPE: Cell<Scope> = Cell::new(Scope::default());
}

pub fn indexed_entry(mem: &dyn GuestRead, esp: u32) -> Option<(MapGeneration, Entry)> {
    SCOPE.with(|slot| {
        let scope = slot.take();
        let entry = scope.indexed_entry(mem, esp);
        slot.set(scope);
        entry
    })
}

struct ReadMemory<'a>(&'a dyn GuestMemory);

impl GuestRead for ReadMemory<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

pub struct NativeTransparents;

impl Feature for NativeTransparents {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Ordered native vertex bindings for sorted transparents")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Stateless, on or off: what it holds (`OPEN`, the run entries
        // join, and `OBJECTS_DRAWN`) lasts one view and every view's
        // first observer forgets it ([`new_view`]); a save comes between
        // renders, with the pipelined render drained. It changes no guest
        // memory, and the host's materials are native-objects' models,
        // keyed by map name.
        b.snapshot(crate::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        let objects = b.switches().feature(crate::native_objects::SWITCH, false);
        b.raw()
            .observe_before(&points::GAME_ACTIVATE, "native_transparents_generation", |_| {
                ACTIVATION.fetch_add(1, Ordering::AcqRel);
            })
            .context_only();
        b.raw()
            .observe_before(&points::SCENE_TRANSPARENTS, "native_transparents_view", |c| {
                let mem = c.guest();
                let mut scope = Scope::default();
                scope.begin_view(&ReadMemory(&*mem), c.cpu.esp);
                SCOPE.with(|slot| slot.set(scope));
            })
            .context_only();
        b.raw()
            .observe_after(&points::SCENE_TRANSPARENTS, "native_transparents_end", |_| {
                SCOPE.with(|slot| slot.set(Scope::default()));
            })
            .context_only();
        b.raw()
            .observe_before(&points::TRANSPARENT_GEOMETRY, "native_transparents_entry", move |c| {
                let mem = c.guest();
                SCOPE.with(|slot| {
                    let mut scope = slot.take();
                    scope.begin_entry(&ReadMemory(&*mem), c.cpu.esp, objects);
                    slot.set(scope);
                });
            })
            .context_only();
        Ok(())
    }
}
