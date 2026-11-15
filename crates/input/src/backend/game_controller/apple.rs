//! `objc2-game-controller` implementation of [`ControllerSource`].
//!
//! Threading follows Apple's GameController guidance. A profile's element
//! values are only meant to be read on the queue its callbacks run on, so we
//! never read them from the polling thread. Instead each extended gamepad gets
//! a private serial queue (the controller's `handlerQueue`) and a
//! `valueDidChangeHandler`; the handler (on that queue) reads the whole
//! profile into a plain [`Snapshot`] behind a mutex, and [`poll`] only copies
//! that snapshot. An initial read is queued on the same queue when a pad
//! appears. Because the queue is ours, input does not depend on the main run
//! loop.
//!
//! Connect/disconnect and wireless discovery are different: the framework
//! delivers those on the main run loop, so `GCController.controllers()` only
//! changes while something services it. A windowed front end (winit/AppKit)
//! already does; a command-line host should call [`service_main_loop`]
//! regularly (the `pads` example does). We also ask for wireless discovery at
//! start-up.
//!
//! Rumble goes through CoreHaptics engines created from the controller's
//! `GCDeviceHaptics`. Capability comes from `supportedLocalities`: separate
//! left/right handle engines when both exist, otherwise one engine on the
//! handles / all / default locality ([`plan_rumble`]). Each engine plays one
//! long looping continuous event whose intensity is a dynamic parameter.
//!
//! Not exercised by CI (needs a controller). The mapping above it is.
//!
//! [`poll`]: ControllerSource::poll

use std::ptr::NonNull;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, msg_send};
use objc2_core_haptics::{
    CHHapticAdvancedPatternPlayer, CHHapticDynamicParameter,
    CHHapticDynamicParameterIDHapticIntensityControl, CHHapticEngine, CHHapticEvent,
    CHHapticEventParameter, CHHapticEventParameterIDHapticIntensity,
    CHHapticEventParameterIDHapticSharpness, CHHapticEventTypeHapticContinuous, CHHapticPattern,
    CHHapticPatternPlayer,
};
use objc2_foundation::{NSArray, NSDate, NSDefaultRunLoopMode, NSRunLoop, NSString};
use objc2_game_controller::{
    GCController, GCControllerButtonInput, GCControllerElement, GCDevice, GCDeviceHaptics,
    GCExtendedGamepad, GCHapticsLocality, GCHapticsLocalityAll, GCHapticsLocalityDefault,
    GCHapticsLocalityHandles, GCHapticsLocalityLeftHandle, GCHapticsLocalityRightHandle,
    GCPhysicalInputProfile,
};

use super::{ControllerSource, Locality, RawPad, RumblePlan, Snapshot, plan_rumble};

/// Longest a CoreHaptics continuous event may last; the player loops it.
const EVENT_SECONDS: f64 = 30.0;

struct Motor {
    engine: Retained<CHHapticEngine>,
    player: Retained<ProtocolObject<dyn CHHapticAdvancedPatternPlayer>>,
    playing: bool,
}

impl Motor {
    /// `sharpness`: low for the heavy motor, high for the light one.
    fn new(haptics: &GCDeviceHaptics, locality: &GCHapticsLocality, sharpness: f32) -> Option<Motor> {
        // SAFETY: plain Objective-C calls with valid arguments. The binding
        // for this method is cfg-gated to iOS/tvOS but the API exists on macOS.
        unsafe {
            let engine: Option<Retained<CHHapticEngine>> =
                msg_send![haptics, createEngineWithLocality: locality];
            let engine = engine?;
            engine.startAndReturnError().ok()?;
            let intensity = CHHapticEventParameter::initWithParameterID_value(
                CHHapticEventParameter::alloc(),
                CHHapticEventParameterIDHapticIntensity,
                1.0,
            );
            let sharpness = CHHapticEventParameter::initWithParameterID_value(
                CHHapticEventParameter::alloc(),
                CHHapticEventParameterIDHapticSharpness,
                sharpness,
            );
            let event = CHHapticEvent::initWithEventType_parameters_relativeTime_duration(
                CHHapticEvent::alloc(),
                CHHapticEventTypeHapticContinuous,
                &NSArray::from_retained_slice(&[intensity, sharpness]),
                0.0,
                EVENT_SECONDS,
            );
            let pattern = CHHapticPattern::initWithEvents_parameters_error(
                CHHapticPattern::alloc(),
                &NSArray::from_retained_slice(&[event]),
                &NSArray::new(),
            )
            .ok()?;
            let player = engine
                .createAdvancedPlayerWithPattern_error(&pattern)
                .ok()?;
            player.setLoopEnabled(true);
            Some(Motor {
                engine,
                player,
                playing: false,
            })
        }
    }

    fn set(&mut self, speed: f32) {
        // SAFETY: valid objects created in `new`; errors are ignored because a
        // lost haptic engine just means no rumble.
        unsafe {
            if speed <= 0.0 {
                if self.playing {
                    let _ = self.player.stopAtTime_error(0.0);
                    self.playing = false;
                }
                return;
            }
            let param = CHHapticDynamicParameter::initWithParameterID_value_relativeTime(
                CHHapticDynamicParameter::alloc(),
                CHHapticDynamicParameterIDHapticIntensityControl,
                speed.clamp(0.0, 1.0),
                0.0,
            );
            let params = NSArray::from_retained_slice(&[param]);
            if !self.playing {
                // The engine stops itself when idle; (re)start is cheap.
                let _ = self.engine.startAndReturnError();
                if self.player.startAtTime_error(0.0).is_err() {
                    return;
                }
                self.playing = true;
            }
            let _ = self.player.sendParameters_atTime_error(&params, 0.0);
        }
    }
}

enum Rumbler {
    Split(Option<Motor>, Option<Motor>),
    Combined(Option<Motor>),
}

struct Entry {
    key: u64,
    controller: Retained<GCController>,
    /// The extended profile our handler is installed on, if the pad has one.
    profile: Option<Retained<GCExtendedGamepad>>,
    /// Written on the private handler queue, copied by `poll`.
    snapshot: Arc<Mutex<Snapshot>>,
    plan: Option<RumblePlan>,
    rumbler: Option<Rumbler>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(p) = &self.profile {
            // SAFETY: clearing a handler on a live profile.
            unsafe { p.setValueDidChangeHandler(None) };
        }
    }
}

fn lock(m: &Mutex<Snapshot>) -> MutexGuard<'_, Snapshot> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wrapper to move a retained profile into a block queued on its own handler
/// queue, the one place it is allowed to be read.
struct OnQueue(Retained<GCExtendedGamepad>);
// SAFETY: only dereferenced on the profile's handler queue.
unsafe impl Send for OnQueue {}
impl OnQueue {
    fn get(&self) -> &GCExtendedGamepad {
        &self.0
    }
}

/// GameController-backed source. See the module docs.
pub struct AppleSource {
    entries: Vec<Entry>,
    next_key: u64,
    /// Serial queue every profile's callbacks run on.
    queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: the Objective-C objects held are reference counted and only used
// for thread-safe property reads and haptics calls from `&mut self` methods;
// profile element values are touched only on `queue` (see module docs). The
// source is owned by one backend and never shared.
unsafe impl Send for AppleSource {}

impl Default for AppleSource {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs the main run loop for up to `max` (it returns early when it handled
/// a source). Controller connect/disconnect and wireless discovery are
/// delivered there, so a host with no window of its own must call this
/// regularly from the main thread. Returns immediately if the run loop has
/// nothing to wait on, so callers that poll should sleep out the remainder.
pub fn service_main_loop(max: Duration) {
    // SAFETY: plain Foundation calls; the mode constant is a valid static.
    unsafe {
        let until = NSDate::dateWithTimeIntervalSinceNow(max.as_secs_f64());
        NSRunLoop::currentRunLoop().runMode_beforeDate(NSDefaultRunLoopMode, &until);
    }
}

impl AppleSource {
    pub fn new() -> Self {
        // SAFETY: plain class methods. Without the first a windowless host
        // stops receiving input when it is not frontmost; the second makes
        // wireless pads show up in `controllers()` (needs the main run loop).
        unsafe {
            GCController::setShouldMonitorBackgroundEvents(true);
            GCController::startWirelessControllerDiscoveryWithCompletionHandler(None);
        }
        Self {
            entries: Vec::new(),
            next_key: 1,
            queue: DispatchQueue::new("h2recomp.input.gamecontroller", DispatchQueueAttr::SERIAL),
        }
    }

    fn attach(&self, controller: Retained<GCController>) -> (Option<Retained<GCExtendedGamepad>>, Arc<Mutex<Snapshot>>, Option<RumblePlan>) {
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        // SAFETY: property reads and handler installation on live objects;
        // the handler queue is set before the handler, as Apple asks.
        let profile = unsafe { controller.extendedGamepad() };
        if let Some(p) = &profile {
            let shared = Arc::clone(&snapshot);
            let handler = RcBlock::new(
                move |profile: NonNull<GCPhysicalInputProfile>, _element: NonNull<GCControllerElement>| {
                    // SAFETY: the handler is only installed on an extended
                    // gamepad, and the framework passes that same object,
                    // alive for the duration of the call, on our queue.
                    let gp = unsafe { profile.cast::<GCExtendedGamepad>().as_ref() };
                    *lock(&shared) = read(gp);
                },
            );
            // SAFETY: see above.
            unsafe {
                controller.setHandlerQueue(&self.queue);
                p.setValueDidChangeHandler(Some(&handler));
            }
            // Pick up buttons already held when the pad appears.
            let shared = Arc::clone(&snapshot);
            let on_queue = OnQueue(p.clone());
            self.queue.exec_async(move || {
                *lock(&shared) = read(on_queue.get());
            });
        }
        // SAFETY: property reads on a live object.
        let plan = unsafe {
            controller.haptics().and_then(|h| {
                let set = h.supportedLocalities();
                let all = [
                    (Locality::LeftHandle, GCHapticsLocalityLeftHandle),
                    (Locality::RightHandle, GCHapticsLocalityRightHandle),
                    (Locality::Handles, GCHapticsLocalityHandles),
                    (Locality::All, GCHapticsLocalityAll),
                    (Locality::Default, GCHapticsLocalityDefault),
                ];
                let supported: Vec<Locality> =
                    all.iter().filter(|(_, n)| set.containsObject(n)).map(|(l, _)| *l).collect();
                plan_rumble(&supported)
            })
        };
        (profile, snapshot, plan)
    }

    /// Reconcile our list with the framework's: keep known controllers (by
    /// object identity) in order, drop vanished ones, append new ones with a
    /// fresh key.
    fn refresh(&mut self) {
        // SAFETY: plain class method returning a retained array snapshot.
        let current = unsafe { GCController::controllers() };
        self.entries.retain(|e| {
            current
                .iter()
                .any(|c| Retained::as_ptr(&c) == Retained::as_ptr(&e.controller))
        });
        for c in current.iter() {
            if !self
                .entries
                .iter()
                .any(|e| Retained::as_ptr(&e.controller) == Retained::as_ptr(&c))
            {
                let (profile, snapshot, plan) = self.attach(c.clone());
                self.entries.push(Entry {
                    key: self.next_key,
                    controller: c,
                    profile,
                    snapshot,
                    plan,
                    rumbler: None,
                });
                self.next_key += 1;
            }
        }
    }
}

fn button(b: &GCControllerButtonInput) -> f32 {
    // SAFETY: property read on a live object.
    unsafe { b.value() }
}

fn read(gp: &GCExtendedGamepad) -> Snapshot {
    // SAFETY: property reads on live objects.
    unsafe {
        let ls = gp.leftThumbstick();
        let rs = gp.rightThumbstick();
        let dpad = gp.dpad();
        let pressed =
            |b: Option<Retained<GCControllerButtonInput>>| b.is_some_and(|b| b.isPressed());
        Snapshot {
            a: button(&gp.buttonA()),
            b: button(&gp.buttonB()),
            x: button(&gp.buttonX()),
            y: button(&gp.buttonY()),
            left_shoulder: button(&gp.leftShoulder()),
            right_shoulder: button(&gp.rightShoulder()),
            left_trigger: button(&gp.leftTrigger()),
            right_trigger: button(&gp.rightTrigger()),
            left_stick: (ls.xAxis().value(), ls.yAxis().value()),
            right_stick: (rs.xAxis().value(), rs.yAxis().value()),
            dpad_up: dpad.up().isPressed(),
            dpad_down: dpad.down().isPressed(),
            dpad_left: dpad.left().isPressed(),
            dpad_right: dpad.right().isPressed(),
            menu: gp.buttonMenu().isPressed(),
            options: pressed(gp.buttonOptions()),
            left_stick_press: pressed(gp.leftThumbstickButton()),
            right_stick_press: pressed(gp.rightThumbstickButton()),
        }
    }
}

impl ControllerSource for AppleSource {
    fn poll(&mut self) -> Vec<RawPad> {
        self.refresh();
        self.entries
            .iter()
            .filter(|e| e.profile.is_some()) // micro gamepads, remotes: skipped
            .map(|e| {
                // SAFETY: property read on a live object.
                let name = unsafe { e.controller.vendorName() }.map_or_else(
                    || "Game controller".to_string(),
                    |n: Retained<NSString>| n.to_string(),
                );
                RawPad {
                    key: e.key,
                    name,
                    snapshot: *lock(&e.snapshot),
                    supports_rumble: e.plan.is_some(),
                }
            })
            .collect()
    }

    fn set_rumble(&mut self, key: u64, left: f32, right: f32) {
        let Some(e) = self.entries.iter_mut().find(|e| e.key == key) else {
            return;
        };
        let Some(plan) = e.plan else { return };
        if e.rumbler.is_none() {
            if left <= 0.0 && right <= 0.0 {
                return;
            }
            // SAFETY: property read on a live object.
            let Some(h) = (unsafe { e.controller.haptics() }) else { return };
            // SAFETY: the localities come from the framework's own constants.
            let make = |loc: Locality, sharp: f32| unsafe {
                let name = match loc {
                    Locality::LeftHandle => GCHapticsLocalityLeftHandle,
                    Locality::RightHandle => GCHapticsLocalityRightHandle,
                    Locality::Handles => GCHapticsLocalityHandles,
                    Locality::All => GCHapticsLocalityAll,
                    Locality::Default => GCHapticsLocalityDefault,
                };
                Motor::new(&h, name, sharp)
            };
            e.rumbler = Some(match plan {
                // Heavy motor: dull; light motor: sharp.
                RumblePlan::Split => Rumbler::Split(
                    make(Locality::LeftHandle, 0.2),
                    make(Locality::RightHandle, 0.8),
                ),
                RumblePlan::Combined(l) => Rumbler::Combined(make(l, 0.5)),
            });
        }
        match &mut e.rumbler {
            Some(Rumbler::Split(l, r)) => {
                if let Some(m) = l {
                    m.set(left);
                }
                if let Some(m) = r {
                    m.set(right);
                }
            }
            Some(Rumbler::Combined(Some(m))) => m.set(left.max(right)),
            _ => {}
        }
    }
}
