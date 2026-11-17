//! Which touch layout to show, from player one's state (docs/runtime/ios.md,
//! "Layouts"), and when the touch controls fire by themselves (Touch Auto
//! Fire).
//!
//! The boot reads the local player's state from guest memory once a tick
//! (`boot::touch_glue`) into a [`Context`] and publishes it ([`publish`]).
//! [`plan`] turns a context into a [`Plan`]: a layout id, which buttons show
//! and which are dimmed, and the move stick's radius. The iOS host asks for
//! the plan each frame (`h2host_touch_layout`) and only draws it.
//!
//! [`fire_mode`] and [`AutoFire`] decide auto-fire: hold the trigger for an
//! automatic weapon while the reticle is red, pulse it at the weapon's rate
//! for a semi-automatic one, never for the sword or with nothing to shoot.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::touch::bits;

/// Where player one is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Seat {
    /// No game, no unit (menus, a cutscene, dead): the full on-foot set.
    #[default]
    None,
    OnFoot,
    /// Driving a vehicle.
    Driver,
    /// In a vehicle, not driving: a passenger or a gunner.
    Passenger,
}

/// How the held weapon's trigger fires.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Trigger {
    /// Unknown (unreadable tag): treated as semi-automatic at
    /// [`DEFAULT_RATE`].
    #[default]
    Unknown,
    /// Fires while held (assault rifle, SMG, plasma rifle).
    Automatic,
    /// One shot (or burst) per pull, at up to `rate` pulls a second
    /// (magnum, battle rifle, sniper rifle, carbine).
    SemiAuto { rate: f32 },
    /// Charges while held (plasma pistol): pulsed like semi-automatic, so it
    /// never charges.
    Charge { rate: f32 },
    /// A melee weapon (the energy sword): no auto-fire, its lunge is the
    /// player's call.
    Melee,
}

/// What [`plan`] needs to know, read from guest memory each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Context {
    pub seat: Seat,
    /// The held weapon has zoom levels (a scope).
    pub zoom: bool,
    /// Dual-wielding: the left trigger fires the left-hand weapon.
    pub dual: bool,
    /// The right-hand weapon's trigger.
    pub trigger: Trigger,
    /// The reticle is red: the crosshair is over an enemy.
    pub on_target: bool,
}

/// The layout ids of `H2TouchLayout::layout` (h2host.h).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum Layout {
    #[default]
    OnFoot = 0,
    Driver = 1,
    Turret = 2,
}

impl Layout {
    pub fn name(self) -> &'static str {
        match self {
            Layout::OnFoot => "on foot",
            Layout::Driver => "driver",
            Layout::Turret => "turret",
        }
    }
}

/// `H2TouchLayout::flags` bits.
pub mod flags {
    /// The weapon has a scope: Zoom is drawn large, next to Fire.
    pub const ZOOM_NEAR_FIRE: u32 = 1 << 0;
    /// Dual-wielding: the left trigger is drawn as a second Fire on the
    /// left side, labelled "FIRE L", and Grenade is gone.
    pub const DUAL: u32 = 1 << 1;
    /// The reticle is red (the host may tint Fire).
    pub const ON_TARGET: u32 = 1 << 2;
}

/// What the host draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub layout: Layout,
    /// [`bits`] of the buttons to draw.
    pub visible: u32,
    /// Of those, the ones that do nothing now (drawn faint; still pressable).
    pub dimmed: u32,
    /// [`flags`].
    pub flags: u32,
    /// Thumb travel for a full move stick, points.
    pub stick_radius: f32,
}

/// The move stick's radius on foot (points).
pub const STICK_RADIUS: f32 = 56.0;
/// And while driving: a wider throw for finer steering and throttle.
pub const DRIVE_STICK_RADIUS: f32 = 80.0;

/// Every button the on-foot set draws (no d-pad: Halo 2 does not use one).
pub const ON_FOOT: u32 = bits::A
    | bits::B
    | bits::X
    | bits::Y
    | bits::BLACK
    | bits::WHITE
    | bits::LEFT_TRIGGER
    | bits::RIGHT_TRIGGER
    | bits::START
    | bits::BACK
    | bits::LEFT_THUMB
    | bits::RIGHT_THUMB;
/// Driving: Fire (the vehicle's guns), Boost (the left trigger), Exit (hold
/// X), Zoom (the tank's), and the menu buttons. No Jump, Crouch, Melee,
/// Grenade or weapon switch.
pub const DRIVER: u32 = bits::RIGHT_TRIGGER
    | bits::LEFT_TRIGGER
    | bits::X
    | bits::RIGHT_THUMB
    | bits::START
    | bits::BACK;
/// A passenger or gunner: look and Fire, Exit (hold X; a tap reloads a
/// passenger's own weapon), Zoom, and the menu buttons.
pub const TURRET: u32 =
    bits::RIGHT_TRIGGER | bits::X | bits::RIGHT_THUMB | bits::START | bits::BACK;

/// The layout for `ctx`.
pub fn plan(ctx: &Context) -> Plan {
    let target = if ctx.on_target { flags::ON_TARGET } else { 0 };
    match ctx.seat {
        Seat::Driver => Plan {
            layout: Layout::Driver,
            visible: DRIVER,
            dimmed: 0,
            flags: target,
            stick_radius: DRIVE_STICK_RADIUS,
        },
        Seat::Passenger => Plan {
            layout: Layout::Turret,
            visible: TURRET,
            dimmed: 0,
            flags: target,
            stick_radius: STICK_RADIUS,
        },
        Seat::OnFoot | Seat::None => {
            let mut visible = ON_FOOT;
            let mut dimmed = 0;
            let mut flags = target;
            if ctx.dual {
                // The left trigger fires the left gun; Halo 2 throws no
                // grenades and has no zoom while dual-wielding.
                flags |= flags::DUAL;
                visible &= !bits::RIGHT_THUMB;
                dimmed |= bits::BLACK;
            } else if ctx.zoom {
                flags |= flags::ZOOM_NEAR_FIRE;
            } else if ctx.seat == Seat::OnFoot {
                dimmed |= bits::RIGHT_THUMB;
            }
            if ctx.seat == Seat::None {
                // Menus: everything plain, as round 1 drew it.
                dimmed = 0;
                visible = ON_FOOT;
                flags = 0;
            }
            Plan {
                layout: Layout::OnFoot,
                visible,
                dimmed,
                flags,
                stick_radius: STICK_RADIUS,
            }
        }
    }
}

// ---- auto-fire ----

/// Shots a second assumed for a weapon whose rate is unknown.
pub const DEFAULT_RATE: f32 = 4.0;
/// Guest reads a second (Halo 2 ticks at 30 Hz).
pub const READS_PER_SECOND: f32 = 30.0;

/// What auto-fire does with the trigger now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireMode {
    Off,
    /// Held while on target.
    Hold,
    /// Pulled for one guest read, then released, every `period` reads.
    Pulse {
        period: u32,
    },
}

fn pulse(rate: f32) -> FireMode {
    let rate = if rate.is_finite() && rate > 0.0 {
        rate
    } else {
        DEFAULT_RATE
    };
    // At least one read released between pulls, so each is a new pull.
    let period = (READS_PER_SECOND / rate).round().max(2.0) as u32;
    FireMode::Pulse { period }
}

/// Whether and how auto-fire pulls the trigger: only with the setting on,
/// on foot or in a passenger seat, with the reticle red, and with a weapon
/// that shoots.
pub fn fire_mode(enabled: bool, ctx: &Context) -> FireMode {
    if !enabled || !ctx.on_target || !matches!(ctx.seat, Seat::OnFoot | Seat::Passenger) {
        return FireMode::Off;
    }
    match ctx.trigger {
        Trigger::Automatic => FireMode::Hold,
        Trigger::SemiAuto { rate } | Trigger::Charge { rate } => pulse(rate),
        Trigger::Unknown => pulse(DEFAULT_RATE),
        Trigger::Melee => FireMode::Off,
    }
}

/// Auto-fire's pulse, counted in guest reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AutoFire {
    /// Reads since the last pull (`None`: never pulled, or reset).
    since: Option<u32>,
}

impl AutoFire {
    /// Whether the trigger is pulled in the state about to be reported.
    pub fn pulled(&self, mode: FireMode) -> bool {
        match mode {
            FireMode::Off => false,
            FireMode::Hold => true,
            FireMode::Pulse { period } => self.since.is_none_or(|n| n >= period.max(2)),
        }
    }

    /// The guest read a state reported under `mode`.
    pub fn read(&mut self, mode: FireMode) {
        self.since = match mode {
            FireMode::Off | FireMode::Hold => None,
            FireMode::Pulse { .. } if self.pulled(mode) => Some(1),
            FireMode::Pulse { .. } => self.since.map(|n| n + 1),
        };
    }
}

// ---- the shared context ----

static CONTEXT: Mutex<Context> = Mutex::new(Context {
    seat: Seat::None,
    zoom: false,
    dual: false,
    trigger: Trigger::Unknown,
    on_target: false,
});
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// The boot's tick hook: player one's state now. Returns the new plan's
/// layout when it differs from the last one published (for the log).
pub fn publish(ctx: Context) -> Option<(Layout, Layout)> {
    let mut c = CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
    if *c == ctx {
        return None;
    }
    let (old, new) = (plan(&c), plan(&ctx));
    *c = ctx;
    if old != new {
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    (old.layout != new.layout).then_some((old.layout, new.layout))
}

/// The last published context.
pub fn context() -> Context {
    *CONTEXT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Bumped whenever the plan changes.
pub fn generation() -> u32 {
    GENERATION.load(Ordering::Relaxed)
}

#[cfg(test)]
#[path = "touch_layout_tests.rs"]
mod tests;
