//! The weather plates' host, guest side (docs/graphics/retire-d3d-r2.md,
//! "R2's decals and weather"). On with the native renderer
//! (`impl.render.native`), built by [`crate::native_decals`].
//!
//! At each call of the plate draw `0x00037a60` (`stdcall`, the `weat`
//! element at `[esp+4]`) Rust makes the plate's draw from the element and
//! the view ([`plan`]): the placement `0x0003a5c0` (kind 3, flags
//! [`FLAGS`]), its kind 3 definition with the setup skipped and the depth
//! result wanted, and the quad strip from that result. The game's
//! admission (`game/src/native_weather_glue.rs`) takes the plate's
//! resolved strip while the call runs ([`take_draw`]).
//!
//! Rust draws every plate; there is no guest comparison draw. A plate
//! Rust cannot draw is a named refusal ([`Arm::refuse`]): the guest's
//! draw stands for that call. `render.scene` carries
//! `weather_plate_admitted` and `weather_plate_refused_<reason>`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Mutex;

use guest_layout::memory::GuestMemory;
use guest_layout::points::{Point, Sig};
use frame_scene::effect::WeatherPlate;
use hooks::Builder;
use native_render::{weather_place, weather_quad};

const DOC: &str = "docs/graphics/retire-d3d-r2.md";
/// The plate draw, called per `weat` element by the weather pass at
/// `0x00036eca`.
pub const PLATE: Point = Point::new("WEATHER_PLATE", 0x0003_7a60, Sig::Opaque, DOC);

/// `[0x0048565c]`, `[0x00485658]`, `[0x004858b8]`: the placement's scale,
/// the result's limit and the target id.
pub const SCALE: u32 = 0x0048_565c;
pub const LIMIT: u32 = 0x0048_5658;
pub const TARGET: u32 = 0x0048_58b8;
/// The view's near and far (`[0x00485ad4]`, `[0x00485ad8]`).
pub const VIEW_NEAR: u32 = 0x0048_5ad4;
pub const VIEW_FAR: u32 = 0x0048_5ad8;
/// The placement's three flags as `0x00037a60` pushes them (`ebp`, `ebp`,
/// `ebx` at `0x00037da4..0x00037da7`: 0, 0, 1). The fourth push, also
/// `ebx`, skips the setup.
pub const FLAGS: [bool; 3] = [false, false, true];

/// What the plate draw reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Inputs {
    pub plate: WeatherPlate,
    pub globals: weather_place::Globals,
    pub near: f32,
    pub far: f32,
}

/// Rust's draw of one plate: the strip, and the definition words the
/// placement writes (`written`; the others are the buffer's).
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub quad: weather_quad::Quad,
    pub define: [u32; 60],
    pub written: [bool; 60],
}

/// The plate's draw, `None` when the placement refuses it (the guest draws
/// nothing then).
pub fn plan(i: &Inputs) -> Option<Draw> {
    let p = &i.plate;
    let range = weather_place::place(weather_place::Input {
        kind: 3,
        near: p.extent[0],
        far: p.extent[1],
        flags: FLAGS,
        scale: i.globals.scale,
    })?;
    let def = weather_place::kind3(&range, &weather_quad::colours(p), true, true, i.globals);
    let quad = weather_quad::quad(p, def.result?, i.near, i.far);
    Some(Draw {
        quad,
        define: def.define,
        written: def.written,
    })
}

/// The host's tally.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arm {
    pub admitted: u64,
    pub refused: BTreeMap<&'static str, u64>,
}

impl Arm {
    const fn new() -> Self {
        Self {
            admitted: 0,
            refused: BTreeMap::new(),
        }
    }

    /// A draw Rust could not make: named and counted; the guest's stands.
    pub fn refuse(&mut self, why: &'static str) {
        *self.refused.entry(why).or_default() += 1;
    }

    fn counters(&self) -> Vec<(String, u64)> {
        let mut out = vec![("weather_plate_admitted".to_owned(), self.admitted)];
        out.extend(
            self.refused
                .iter()
                .map(|(r, n)| (format!("weather_plate_refused_{r}"), *n)),
        );
        out
    }
}

static ARM: Mutex<Arm> = Mutex::new(Arm::new());

fn with<T>(f: impl FnOnce(&mut Arm) -> T) -> T {
    f(&mut ARM.lock().unwrap_or_else(|e| e.into_inner()))
}

/// [`Arm::refuse`].
pub fn refuse(why: &'static str) {
    with(|a| a.refuse(why));
}

/// Rust's draw was handed to the device.
pub fn admitted() {
    with(|a| a.admitted += 1);
}

/// `render.scene` rows.
pub fn counters() -> Vec<(String, u64)> {
    with(|a| a.counters())
}

/// [`counters`] with static names (each distinct name is leaked once).
pub fn rows() -> Vec<(&'static str, u64)> {
    static NAMES: Mutex<BTreeMap<String, &'static str>> = Mutex::new(BTreeMap::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    counters()
        .into_iter()
        .map(|(n, v)| {
            (
                *names
                    .entry(n.clone())
                    .or_insert_with(|| Box::leak(n.into_boxed_str())),
                v,
            )
        })
        .collect()
}

/// One call of the plate draw: Rust's plan, or why it has none, and
/// whether its draw was seen.
struct Call {
    plan: Result<Option<Draw>, &'static str>,
    seen: bool,
}

thread_local! {
    /// The plate call running on this thread.
    static CALL: RefCell<Option<Call>> = const { RefCell::new(None) };
}

/// At a draw: Rust's plan for the plate call running on this thread, or
/// why it has none. `None` outside a call, and for a second draw of one
/// call (counted `second_draw`; the guest's stands).
pub fn take_draw() -> Option<Result<Option<Draw>, &'static str>> {
    let plan = CALL.with(|c| {
        let mut c = c.borrow_mut();
        let call = c.as_mut()?;
        let first = !std::mem::replace(&mut call.seen, true);
        Some(first.then(|| call.plan.clone()))
    })?;
    if plan.is_none() {
        refuse("second_draw");
    }
    plan
}

struct Ro<'a>(&'a dyn GuestMemory);

impl scene_extract::GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The plate draw's inputs for the element at `element`.
pub fn inputs(mem: &dyn GuestMemory, element: u32) -> Result<Inputs, &'static str> {
    let ro = Ro(mem);
    let (player, window) = scene_extract::weather::view(&ro).ok_or("view_unread")?;
    let plate =
        scene_extract::weather::plate(&ro, element, 0, player, window).ok_or("element_unread")?;
    let f = |a: u32| mem.u32(a).map(f32::from_bits).map_err(|_| "globals_unread");
    Ok(Inputs {
        plate,
        globals: weather_place::Globals {
            scale: f(SCALE)?,
            limit: f(LIMIT)?,
            target: mem.u16(TARGET).map_err(|_| "globals_unread")?,
        },
        near: f(VIEW_NEAR)?,
        far: f(VIEW_FAR)?,
    })
}

fn begin(mem: &dyn GuestMemory, esp: u32) {
    let plan = esp
        .checked_add(4)
        .and_then(|a| mem.u32(a).ok())
        .ok_or("element_unread")
        .and_then(|e| inputs(mem, e))
        .map(|i| plan(&i));
    CALL.with(|c| *c.borrow_mut() = Some(Call { plan, seen: false }));
}

/// The call returned. A plan that expected a draw no admission saw is
/// counted `unseen`.
fn end() {
    if let Some(Call {
        plan: Ok(Some(_)),
        seen: false,
    }) = CALL.with(|c| c.borrow_mut().take())
    {
        refuse("unseen");
    }
}

/// The two observers at the plate draw.
pub(crate) fn build(b: &mut Builder) {
    b.raw()
        .observe_before(&PLATE, "native_weather_plate_begin", |o| {
            let mem = o.guest();
            begin(&*mem, o.cpu.esp);
        })
        .context_only()
        .optional();
    b.raw()
        .observe_after(&PLATE, "native_weather_plate_end", |_| end())
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(extent: [f32; 2]) -> Inputs {
        Inputs {
            plate: WeatherPlate {
                element: 0,
                player: 0,
                window: [0, 0, 640, 480],
                bitmaps: [1, 2, 3],
                layer_scale: [1.0; 3],
                scroll: [[0.25, 0.5], [1.0, 2.0], [3.0, 4.0]],
                extent,
                size: [2.0, 3.0, 4.0],
                tint: [1.0, 0.5, 0.25],
                fill: [0.1, 0.2, 0.3],
            },
            globals: weather_place::Globals {
                scale: 1.0,
                limit: 0.25,
                target: 2,
            },
            near: 0.5,
            far: 100.0,
        }
    }

    #[test]
    fn a_plate_is_drawn_at_its_placements_depth_result() {
        let i = inputs([1.0, 4.0]);
        let d = plan(&i).unwrap();
        // Near 1 times scale 1 is over the limit of 0.25: the result is 1.
        assert_eq!(d.quad, weather_quad::quad(&i.plate, 1.0, 0.5, 100.0));
        assert_eq!(d.written.iter().filter(|&&b| b).count(), 42);
        // A range the placement refuses draws nothing.
        assert_eq!(plan(&inputs([0.5, 0.5])), None);
    }

    #[test]
    fn every_plate_is_rusts_and_a_gap_is_a_named_refusal() {
        let mut a = Arm::new();
        a.admitted += 3;
        a.refuse("vertex_declaration");
        a.refuse("placement");
        let rows = a.counters();
        for (name, n) in [
            ("weather_plate_admitted", 3),
            ("weather_plate_refused_vertex_declaration", 1),
            ("weather_plate_refused_placement", 1),
        ] {
            assert!(rows.contains(&(name.to_owned(), n)), "{name}");
        }
        assert!(
            rows.iter().all(|(n, _)| !n.contains("compared") && !n.contains("fallback")),
            "no comparison arm"
        );
    }

    #[test]
    fn a_call_hands_its_plan_to_its_first_draw_only() {
        assert_eq!(take_draw(), None, "outside a call");
        CALL.with(|c| {
            *c.borrow_mut() = Some(Call {
                plan: Err("view_unread"),
                seen: false,
            })
        });
        assert_eq!(take_draw(), Some(Err("view_unread")));
        assert_eq!(take_draw(), None, "a second draw is the guest's");
        end();
        assert_eq!(take_draw(), None, "the call returned");
    }
}
