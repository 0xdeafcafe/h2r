//! A native targets host that records the device calls, for the ocean's
//! tests.

use std::sync::Mutex;

use engine::memory::GuestMemory;

use crate::native_targets::{Bind, Host};

/// One device call, with the words it read from guest memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Guest(u32),
    Clear { flags: u32, colour: u32 },
    Texture { stage: u32, texture: u32 },
    Constants { register: u32, words: Vec<u32> },
    Registers { register: u32, words: Vec<u32> },
    StageState { stage: u32, state: u32, value: u32 },
    VertexProgram { function: u32, address: u32 },
    VertexInput { handle: u32, count: u32, inputs: u32 },
    SelectProgram { vaf: u32, address: u32 },
    Visible,
    RenderState { slot: u32, value: u32 },
    MakeSpace,
    PushViewport { put: u32 },
    Draw { indices: u32, count: u32 },
}

#[derive(Default)]
pub struct Fake {
    pub events: Mutex<Vec<Event>>,
}

impl Fake {
    pub fn push(&self, e: Event) {
        self.events.lock().unwrap().push(e);
    }
    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}

fn read(mem: &dyn GuestMemory, at: u32, n: u32) -> Vec<u32> {
    (0..n).map(|i| mem.u32(at + 4 * i).unwrap()).collect()
}

/// The put [`Fake::push_viewport`] advances by.
pub const PUSH: u32 = 0x28;

impl Host for Fake {
    fn bind(&self, _mem: &mut dyn GuestMemory, _bind: Bind, _colour: u32, _depth: u32) -> bool {
        false
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
    fn make_space(&self, _mem: &mut dyn GuestMemory, _minimum: u32, _requested: u32) -> Option<u32> {
        self.push(Event::MakeSpace);
        Some(0)
    }
    fn push_viewport(&self, _mem: &mut dyn GuestMemory, put: u32, _device: u32) -> Option<u32> {
        self.push(Event::PushViewport { put });
        Some(put + PUSH)
    }
}
