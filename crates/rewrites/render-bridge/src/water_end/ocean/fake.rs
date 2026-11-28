//! A native targets host that records the device calls, for the ocean's
//! tests.

use std::sync::Mutex;

use engine::memory::GuestMemory;

use crate::native_targets::{Bind, Host};

/// One device call, with the words it read from guest memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Clear { flags: u32, colour: u32 },
    Bind { colour: u32, depth: u32 },
    Viewport { words: Vec<u32> },
    Sky { constants: Vec<u32> },
    Texture { stage: u32, texture: u32 },
    Constants { register: u32, words: Vec<u32> },
    Registers { register: u32, words: Vec<u32> },
    StageState { stage: u32, state: u32, value: u32 },
    VertexProgram { function: u32, address: u32 },
    VertexInput { handle: u32, count: u32, inputs: u32 },
    SelectProgram { vaf: u32, address: u32 },
    Visible,
    RenderState { slot: u32, value: u32 },
    Draw(Box<super::Draw>),
    CopyView { target: u32 },
    Restore { writes: usize },
}

#[derive(Default)]
pub struct Fake {
    pub events: Mutex<Vec<Event>>,
    /// The copy into a target is refused.
    pub refuse_copy: std::sync::atomic::AtomicBool,
}

impl Fake {
    pub fn push(&self, e: Event) {
        self.events.lock().unwrap().push(e);
    }
    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}

pub fn read(mem: &dyn GuestMemory, at: u32, n: u32) -> Vec<u32> {
    (0..n).map(|i| mem.u32(at + 4 * i).unwrap()).collect()
}

/// The size every surface has.
pub const SURFACE: (u32, u32) = (512, 256);

impl Host for Fake {
    fn bind(&self, _mem: &mut dyn GuestMemory, _bind: Bind, colour: u32, depth: u32) -> bool {
        self.push(Event::Bind { colour, depth });
        true
    }
    fn surface_size(&self, _mem: &dyn GuestMemory, header: u32) -> Option<(u32, u32)> {
        (header != 0).then_some(SURFACE)
    }
    fn viewport(&self, mem: &mut dyn GuestMemory, at: u32) -> bool {
        self.push(Event::Viewport { words: read(&*mem, at, 6) });
        true
    }
    fn clear(&self, _mem: &mut dyn GuestMemory, flags: u32, colour: u32, _z: u32, _stencil: u32) -> bool {
        self.push(Event::Clear { flags, colour });
        true
    }
    fn bound(&self) -> Option<[u32; 8]> {
        None
    }
    fn texture(&self, _mem: &mut dyn GuestMemory, stage: u32, texture: u32) -> bool {
        self.push(Event::Texture { stage, texture });
        true
    }
    fn render_state(&self, _mem: &mut dyn GuestMemory, slot: u32, value: u32) -> bool {
        self.push(Event::RenderState { slot, value });
        true
    }
    fn constants(&self, mem: &mut dyn GuestMemory, register: u32, data: u32, dwords: u32) -> bool {
        self.push(Event::Constants { register, words: read(&*mem, data, dwords) });
        true
    }
    fn registers(&self, mem: &mut dyn GuestMemory, register: u32, data: u32, count: u32) -> bool {
        self.push(Event::Registers { register, words: read(&*mem, data, 4 * count) });
        true
    }
    fn texture_stage_state(&self, _mem: &mut dyn GuestMemory, stage: u32, state: u32, value: u32) -> bool {
        self.push(Event::StageState { stage, state, value });
        true
    }
    fn vertex_program(&self, _mem: &mut dyn GuestMemory, function: u32, address: u32) -> bool {
        self.push(Event::VertexProgram { function, address });
        true
    }
    fn vertex_input(&self, _mem: &mut dyn GuestMemory, handle: u32, count: u32, inputs: u32) -> bool {
        self.push(Event::VertexInput { handle, count, inputs });
        true
    }
    fn select_vertex_program(&self, _mem: &mut dyn GuestMemory, vaf: u32, address: u32) -> bool {
        self.push(Event::SelectProgram { vaf, address });
        true
    }
    fn copy_view(&self, _mem: &mut dyn GuestMemory, target: u32) -> bool {
        self.push(Event::CopyView { target });
        !self.refuse_copy.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn restore(&self, _mem: &mut dyn GuestMemory, writes: &[crate::open_shader::Write]) -> bool {
        self.push(Event::Restore { writes: writes.len() });
        true
    }
}
