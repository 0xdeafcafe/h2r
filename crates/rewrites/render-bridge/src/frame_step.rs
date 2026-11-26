//! The frame step's tick-visible writes (docs/graphics/framescene.md, 5.2).
//!
//! A cut producer's guest body never runs ([`crate::cut`]), so whatever the
//! tick reads from what that body wrote is made here, in the order the
//! guest made it. Each [`Row`] is one row of the 5.2 table: the writes it
//! makes, who owns it, and the test in `tests/cut.rs` that proves it. A row
//! whose writer is a later slice's body (the seed, the effects word, the
//! stamps, residency) is a no-op until that slice fills it; the row and its
//! place in the order exist now so the slice only adds the body.

use engine::memory::{GuestMemory, MemFault};

/// The guest words the frame step writes.
#[derive(Clone, Copy, Debug)]
pub struct Addrs {
    /// The 64-bit present counter `rasterizer_present` (`0x00013f10`) adds to.
    pub present_counter: u32,
    /// The render frame counter `render_views` adds to.
    pub frame_counter: u32,
    /// The one-shot byte `render_views` (`0x00223080`) clears per view.
    pub one_shot: u32,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        present_counter: 0x0048_5aa0,
        frame_counter: 0x004b_a034,
        one_shot: 0x004e_6098,
    };
}

/// Who runs a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The frame owner, once per frame ([`Step::Frame`]).
    Frame,
    /// Before each of these cut producers' bodies would have run, named by
    /// `residual_census::REGISTRY` row ([`Step::Producer`]).
    Producers(&'static [&'static str]),
}

/// One step: the frame, or one cut producer's turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step<'a> {
    Frame,
    Producer(&'a str),
}

/// One row of framescene.md 5.2.
pub struct Row {
    pub name: &'static str,
    pub owner: Owner,
    /// The test in `tests/cut.rs` that proves the row.
    pub test: &'static str,
    make: fn(&Addrs, &mut dyn GuestMemory) -> Result<(), MemFault>,
}

fn nothing(_: &Addrs, _: &mut dyn GuestMemory) -> Result<(), MemFault> {
    Ok(())
}

fn frame_counter(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let f = g.u32(a.frame_counter)?;
    g.set_u32(a.frame_counter, f.wrapping_add(1))
}

fn one_shot(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    g.set_u8(a.one_shot, 0)
}

fn present_counter(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let lo = g.u32(a.present_counter)?;
    let hi = g.u32(a.present_counter.wrapping_add(4))?;
    let v = (u64::from(hi) << 32 | u64::from(lo)).wrapping_add(1);
    g.write(a.present_counter, &v.to_le_bytes())
}

/// The residency requests are the object setup's: its own producer
/// (`native_scene_lists`, [`crate::cut::OWN_PRODUCER`]) stages them with the
/// records it writes, in the guest's visit order, and publishes both in one
/// turn, so the row's `make` is empty.
///
/// The rows, in the order the guest's frame makes them: object code
/// (seed, stamps, residency) and the particle draws inside the views, then
/// `render_views`' counter and one-shot byte, then the present's counter.
/// The rows 5.2 marks "guest code keeps running" and the frame wait, the
/// game-time bracket and frame timing are the frame owner's own calls and
/// have no row.
pub const ROWS: &[Row] = &[
    Row {
        name: "shared_seed",
        owner: Owner::Producers(&[]),
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: nothing,
    },
    Row {
        name: "effects_word",
        owner: Owner::Producers(&[]),
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: nothing,
    },
    Row {
        name: "particle_stamps",
        owner: Owner::Producers(&[]),
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: nothing,
    },
    Row {
        name: "animation_stamps",
        owner: Owner::Producers(&[]),
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: nothing,
    },
    Row {
        name: "residency",
        owner: Owner::Producers(&["object_setup"]),
        test: "object_setup_makes_the_guests_residency_requests",
        make: nothing,
    },
    Row {
        name: "render_frame_counter",
        owner: Owner::Frame,
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: frame_counter,
    },
    Row {
        name: "one_shot_byte",
        owner: Owner::Frame,
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: one_shot,
    },
    Row {
        name: "present_counter",
        owner: Owner::Frame,
        test: "the_frame_step_makes_the_ticks_writes_in_guest_order",
        make: present_counter,
    },
];

/// Makes `step`'s rows' writes in [`ROWS`] order and returns their names.
pub fn writes_at(
    a: &Addrs,
    mem: &mut dyn GuestMemory,
    step: Step,
) -> Result<Vec<&'static str>, MemFault> {
    let mut made = Vec::new();
    for row in ROWS {
        let mine = match (row.owner, step) {
            (Owner::Frame, Step::Frame) => true,
            (Owner::Producers(names), Step::Producer(p)) => names.contains(&p),
            _ => false,
        };
        if mine {
            (row.make)(a, mem)?;
            made.push(row.name);
        }
    }
    Ok(made)
}

/// [`writes_at`] the retail image's words.
pub fn writes(mem: &mut dyn GuestMemory, step: Step) -> Result<Vec<&'static str>, MemFault> {
    writes_at(&Addrs::HALO2, mem, step)
}

/// What a cut producer's hook runs before it answers: its turn's rows.
pub fn before(mem: &mut dyn GuestMemory, name: &str) -> Result<(), MemFault> {
    writes(mem, Step::Producer(name)).map(drop)
}
