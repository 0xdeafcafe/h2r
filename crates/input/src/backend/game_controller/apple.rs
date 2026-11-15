//! `objc2-game-controller` implementation of [`ControllerSource`].
//!
//! Reads `GCController.controllers()` on every poll and the extended gamepad
//! profile's live values. Rumble goes through CoreHaptics engines created from
//! the controller's `GCDeviceHaptics` (left handle for the heavy motor, right
//! handle for the light one), each playing one long looping continuous event
//! whose intensity is driven by a dynamic parameter.
//!
//! Not exercised by CI (needs a controller). The mapping above it is.
//!
//! The framework delivers controller connect/disconnect on the main run loop;
//! a host that never runs one may see the list update late. The windowed
//! front end (winit/AppKit) runs it already.

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
use objc2_foundation::{NSArray, NSString};
use objc2_game_controller::{
    GCController, GCControllerButtonInput, GCDevice, GCDeviceHaptics, GCExtendedGamepad,
    GCHapticsLocalityLeftHandle, GCHapticsLocalityRightHandle,
};

use super::{ControllerSource, RawPad, Snapshot};

/// Longest a CoreHaptics continuous event may last; the player loops it.
const EVENT_SECONDS: f64 = 30.0;

struct Motor {
    engine: Retained<CHHapticEngine>,
    player: Retained<ProtocolObject<dyn CHHapticAdvancedPatternPlayer>>,
    playing: bool,
}

impl Motor {
    fn new(haptics: &GCDeviceHaptics, right: bool) -> Option<Motor> {
        // SAFETY: plain Objective-C calls with valid arguments. The binding
        // for this method is cfg-gated to iOS/tvOS but the API exists on macOS.
        unsafe {
            let locality = if right {
                GCHapticsLocalityRightHandle
            } else {
                GCHapticsLocalityLeftHandle
            };
            let engine: Option<Retained<CHHapticEngine>> =
                msg_send![haptics, createEngineWithLocality: locality];
            let engine = engine?;
            engine.startAndReturnError().ok()?;
            let intensity = CHHapticEventParameter::initWithParameterID_value(
                CHHapticEventParameter::alloc(),
                CHHapticEventParameterIDHapticIntensity,
                1.0,
            );
            // Heavy motor: dull; light motor: sharp.
            let sharpness = CHHapticEventParameter::initWithParameterID_value(
                CHHapticEventParameter::alloc(),
                CHHapticEventParameterIDHapticSharpness,
                if right { 0.8 } else { 0.2 },
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

struct Entry {
    key: u64,
    controller: Retained<GCController>,
    motors: Option<(Option<Motor>, Option<Motor>)>,
}

/// GameController-backed source. See the module docs.
pub struct AppleSource {
    entries: Vec<Entry>,
    next_key: u64,
}

// SAFETY: GameController and CoreHaptics objects are reference-counted and
// documented as usable off the main thread. The source is owned by one
// backend, moved to a thread once and never shared (`&mut self` everywhere).
unsafe impl Send for AppleSource {}

impl Default for AppleSource {
    fn default() -> Self {
        Self::new()
    }
}

impl AppleSource {
    pub fn new() -> Self {
        // SAFETY: a plain class method. Without it a windowless host stops
        // receiving input as soon as it is not the frontmost app.
        unsafe { GCController::setShouldMonitorBackgroundEvents(true) };
        Self {
            entries: Vec::new(),
            next_key: 1,
        }
    }

    /// Reconcile our list with the framework's: keep known controllers (by
    /// object identity) in order, drop vanished ones, append new ones with a
    /// fresh key.
    fn refresh(&mut self) {
        // SAFETY: plain class method returning a retained array.
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
                self.entries.push(Entry {
                    key: self.next_key,
                    controller: c,
                    motors: None,
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
            .filter_map(|e| {
                // SAFETY: property reads on live objects. Pads without the
                // extended profile (micro gamepads, remotes) are skipped.
                unsafe {
                    let gp = e.controller.extendedGamepad()?;
                    let name = e.controller.vendorName().map_or_else(
                        || "Game controller".to_string(),
                        |n: Retained<NSString>| n.to_string(),
                    );
                    Some(RawPad {
                        key: e.key,
                        name,
                        snapshot: read(&gp),
                        supports_rumble: e.controller.haptics().is_some(),
                    })
                }
            })
            .collect()
    }

    fn set_rumble(&mut self, key: u64, left: f32, right: f32) {
        let Some(e) = self.entries.iter_mut().find(|e| e.key == key) else {
            return;
        };
        if e.motors.is_none() {
            if left <= 0.0 && right <= 0.0 {
                return;
            }
            // SAFETY: property read on a live object.
            let haptics = unsafe { e.controller.haptics() };
            e.motors = Some(match haptics {
                Some(h) => (Motor::new(&h, false), Motor::new(&h, true)),
                None => (None, None),
            });
        }
        if let Some((l, r)) = &mut e.motors {
            if let Some(m) = l {
                m.set(left);
            }
            if let Some(m) = r {
                m.set(right);
            }
        }
    }
}
