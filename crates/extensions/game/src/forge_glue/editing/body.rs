//! The Forge body swap (docs/game/forge-design-slices.md, "The monitor
//! body"): D-pad Up makes the player's chosen body (`forge::body`) their
//! unit, and back, as Halo 3's monitor does. The player keeps their own
//! view, HUD and seat; only the unit changes.
//!
//! Each step is a [`Call::ForgeBody`] on the calls queue, made at the next
//! tick with the session guard checked: Enter makes the body where the
//! player's eye is and gives it to the player, Fly moves it to the Objects
//! tool's camera each tick the camera moves, Leave gives the kept biped
//! back where the body is and deletes the body. The biped is left in the
//! world while the player flies, undamageable and not drawn
//! (`forge_controls::render::park`).
use agent::sim::calls::{self, BodyStep, Call, PendingCall};
use guest_layout::GuestMemory;
use guest_layout::view::{objects, players};
use forge::camera::Camera;
use forge_doc::bodies::Body;
use serde_json::Value;

use super::Editor;

/// The local player's unit as the swap sees it.
pub(crate) enum Swap {
    /// In their own biped.
    Biped,
    /// Enter is queued.
    Entering {
        body: Body,
        camera: Camera,
        call: PendingCall,
    },
    In(Flying),
    /// Leave is queued.
    Leaving { flying: Flying, call: PendingCall },
}

/// A body the player is in.
pub(crate) struct Flying {
    pub body: Body,
    /// The body's unit.
    pub unit: u32,
    /// The player's own biped, kept to come back to.
    pub biped: u32,
    /// The camera the last Fly sent: an unmoved camera sends none.
    sent: Option<Camera>,
    fly: Option<PendingCall>,
}

impl Swap {
    /// The kept biped while the player is in a body, for the park.
    pub(crate) fn parked(&self) -> Option<u32> {
        match self {
            Swap::In(f) | Swap::Leaving { flying: f, .. } => Some(f.biped),
            Swap::Biped | Swap::Entering { .. } => None,
        }
    }

    /// The body the player is in, with its unit.
    pub(crate) fn flying(&self) -> Option<(Body, u32)> {
        match self {
            Swap::In(f) | Swap::Leaving { flying: f, .. } => Some((f.body, f.unit)),
            Swap::Biped | Swap::Entering { .. } => None,
        }
    }
}

/// The local player (local user 0): its handle, its unit and the unit's
/// position and forward.
pub(crate) fn local_player(m: &dyn GuestMemory) -> Option<(u32, u32, [f32; 3], [f32; 3])> {
    let array = players::array(m).ok()??;
    let (handle, player) = array
        .live(m, 16)
        .ok()?
        .into_iter()
        .find(|(_, p)| p.local_user(m).ok() == Some(0))?;
    let unit = player.unit(m).ok()??;
    let (_, body) = objects::get(m, unit).ok()??;
    Some((handle.raw(), unit.raw(), body.position(m).ok()?, body.forward(m).ok()?))
}

/// A camera looking along `forward` from `position`.
pub(crate) fn camera_along(position: [f32; 3], forward: [f32; 3]) -> Camera {
    let [x, y, z] = forward;
    Camera {
        position,
        yaw: y.atan2(x),
        pitch: z.clamp(-1., 1.).asin(),
    }
}

/// How far above a biped's origin its eye is, for an Enter with no drawn
/// view yet (`agent_spawn_view`'s standing eye).
const EYE: f32 = 0.64;

/// The handle an answer names under `key`, as the calls write it (eight
/// hex digits).
fn handle(v: &Value, key: &str) -> Option<u32> {
    u32::from_str_radix(v.get(key)?.as_str()?, 16).ok()
}

impl Editor {
    /// D-pad Up: into the chosen body, or back into the biped. A body the
    /// map's kit lacks is refused by name, and with it Forge, when it is
    /// the Monitor; nothing is swapped for another body.
    pub(super) fn toggle_body(&mut self, m: &dyn GuestMemory) -> Result<(), String> {
        match std::mem::replace(&mut self.swap, Swap::Biped) {
            Swap::Biped => self.enter_body(m),
            Swap::In(flying) => {
                let (player, ..) = local_player(m).ok_or("You have no unit to swap.")?;
                let call = calls::submit(
                    &self.calls,
                    Call::ForgeBody {
                        guard: self.guard,
                        player,
                        step: BodyStep::Leave {
                            body: flying.unit,
                            biped: flying.biped,
                        },
                    },
                )?;
                self.tool.clear_selection();
                self.swap = Swap::Leaving { flying, call };
                Ok(())
            }
            // A press while a swap is under way waits for it.
            other => {
                self.swap = other;
                Ok(())
            }
        }
    }

    fn enter_body(&mut self, m: &dyn GuestMemory) -> Result<(), String> {
        let tag = self.bodies.unit(self.body)?;
        let (player, _, position, forward) =
            local_player(m).ok_or("You have no unit to swap: wait for your spawn.")?;
        let camera = crate::forge_controls::eye().unwrap_or_else(|| {
            let [x, y, z] = position;
            camera_along([x, y, z + EYE], forward)
        });
        let call = calls::submit(
            &self.calls,
            Call::ForgeBody {
                guard: self.guard,
                player,
                step: BodyStep::Enter {
                    tag,
                    pos: camera.position,
                    forward: camera.forward(),
                },
            },
        )?;
        self.swap = Swap::Entering {
            body: self.body,
            camera,
            call,
        };
        Ok(())
    }

    /// How the queued step came out. Entering puts the tool in the
    /// monitor at the body's camera; leaving takes it out.
    pub(super) fn poll_body(&mut self, tick: u32) {
        match std::mem::replace(&mut self.swap, Swap::Biped) {
            Swap::Biped => {}
            Swap::Entering { body, camera, call } => match call.outcome() {
                None => self.swap = Swap::Entering { body, camera, call },
                Some(Ok(v)) => match (handle(&v, "body"), handle(&v, "biped")) {
                    (Some(unit), Some(biped)) => {
                        self.tool.clear_selection();
                        self.tool.monitor = true;
                        self.tool.camera = Some(camera);
                        self.swap = Swap::In(Flying {
                            body,
                            unit,
                            biped,
                            sent: Some(camera),
                            fly: None,
                        });
                    }
                    _ => self.refuse("The body swap gave no body.".into(), tick),
                },
                Some(Err(e)) => self.refuse(e.message, tick),
            },
            Swap::In(mut flying) => {
                if let Some(call) = &flying.fly {
                    match call.outcome() {
                        None => {}
                        Some(Ok(_)) => flying.fly = None,
                        Some(Err(e)) => {
                            // The body is gone (or the player is out of
                            // it): they are back in what the game gives.
                            self.out_of_body();
                            self.refuse(e.message, tick);
                            return;
                        }
                    }
                }
                self.swap = Swap::In(flying);
            }
            Swap::Leaving { flying, call } => match call.outcome() {
                None => self.swap = Swap::Leaving { flying, call },
                Some(Ok(v)) => {
                    self.out_of_body();
                    if v.get("placed") == Some(&Value::Bool(false)) {
                        self.say(
                            "No room for your biped here: it stands where you left it.".into(),
                            tick,
                        );
                    }
                }
                Some(Err(e)) => {
                    self.swap = Swap::In(flying);
                    self.refuse(e.message, tick);
                }
            },
        }
    }

    fn out_of_body(&mut self) {
        self.swap = Swap::Biped;
        self.tool.clear_selection();
        self.tool.monitor = false;
        self.tool.camera = None;
    }

    /// The body follows the tool's camera: one Fly at a time, and only for
    /// a camera that moved.
    pub(super) fn fly_body(&mut self) {
        let Swap::In(flying) = &mut self.swap else { return };
        let Some(camera) = self.tool.camera else { return };
        if flying.fly.is_some() || flying.sent == Some(camera) {
            return;
        }
        let Some(player) = self.player else { return };
        let step = BodyStep::Fly {
            body: flying.unit,
            pos: camera.position,
            forward: camera.forward(),
        };
        if let Ok(call) = calls::submit(
            &self.calls,
            Call::ForgeBody {
                guard: self.guard,
                player,
                step,
            },
        ) {
            flying.fly = Some(call);
            flying.sent = Some(camera);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_along_a_forward_points_that_way() {
        let c = camera_along([1., 2., 3.], [0., 0.6, 0.8]);
        let f = c.forward();
        for (a, b) in f.iter().zip([0., 0.6, 0.8]) {
            assert!((a - b).abs() < 1e-5, "{f:?}");
        }
        assert_eq!(c.position, [1., 2., 3.]);
    }

    #[test]
    fn answers_name_their_handles_in_hex() {
        let v = serde_json::json!({ "body": "e1230004", "biped": "x" });
        assert_eq!(handle(&v, "body"), Some(0xe123_0004));
        assert_eq!(handle(&v, "biped"), None);
        assert_eq!(handle(&v, "gone"), None);
    }
}
