//! One camera rig drives the independent observer and the guest's render view.
use std::sync::{Arc, Mutex};

use agent_proto::theater::{self as api, CameraMode};
use guest_layout::{GuestMemory, points};
use engine::render_loop::views::VIEW_STRIDE;
use hooks::{Builder, Flow};

use super::{Mode, Presentation, Session, forge_film, rig::ReplayCamera};
use crate::spectator_view::{Pose, draw_world_view, put_fov, retail_view, vertical_fov};

#[derive(Default)]
struct Views {
    rig: ReplayCamera,
    revision: u64,
    subject: Option<u8>,
    crop_seat: Option<u8>,
    drawing: [Option<[u32; 4]>; 16],
    available: u16,
    playerless: bool,
    hide_hud: bool,
}

impl Views {
    fn has_viewport(&self) -> bool {
        // A monitor seat has no guest viewport: the film's own records give
        // its first-person view.
        self.rig.seat.map_or(self.available != 0, |seat| {
            self.available & (1 << seat) != 0 || forge_film::is_monitor(seat)
        })
    }

    fn mode(&self) -> CameraMode {
        self.rig.effective(self.playerless, self.has_viewport())
    }
}

#[derive(Default, Clone)]
pub struct FilmCamera(Arc<Mutex<Views>>);

#[derive(Debug, PartialEq)]
pub(super) struct CameraState {
    pub mode: CameraMode,
    pub seat: Option<u8>,
    pub player: Option<String>,
    pub pose: Option<api::CameraPose>,
    pub fallback: Option<api::CameraFallback>,
}

impl FilmCamera {
    fn lock(&self) -> std::sync::MutexGuard<'_, Views> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn playerless(&self) -> bool {
        self.lock().playerless
    }

    #[cfg(feature = "gpu")]
    pub(super) fn set_playerless(&self) {
        self.lock().playerless = true;
    }

    pub(super) fn state(&self) -> CameraState {
        let v = self.lock();
        CameraState {
            mode: v.mode(),
            seat: v.rig.seat.or(v.subject),
            player: v.rig.player.clone(),
            pose: v.rig.reported_pose(),
            fallback: v.rig.fallback(v.playerless, v.has_viewport()),
        }
    }

    /// Whether the picture is a local seat's own first-person view, which
    /// reads the look state; free, follow and orbit replace the view.
    pub(super) fn reads_look(&self) -> bool {
        matches!(
            self.lock().mode(),
            CameraMode::Default | CameraMode::FirstPerson
        )
    }

    pub(super) fn restore(&self, presentation: &Presentation) {
        let mut v = self.lock();
        v.rig = presentation.camera.clone();
        v.rig.seat = v.rig.seat.filter(|s| *s < 16);
        v.hide_hud = !presentation.hud;
    }

    pub(super) fn save(&self, presentation: &mut Presentation) {
        let v = self.lock();
        presentation.camera = v.rig.clone();
        presentation.hud = !v.hide_hud;
    }

    pub(super) fn set_hud(&self, visible: bool) {
        let mut v = self.lock();
        v.hide_hud = !visible;
        v.revision = v.revision.wrapping_add(1);
    }

    pub fn revision(&self) -> u64 {
        self.lock().revision
    }

    pub(super) fn has_seat(&self, seat: u8) -> bool {
        seat < 16 && self.lock().available & (1 << seat) != 0
    }

    pub(super) fn control(
        &self,
        request: api::Camera,
        seat: Option<u8>,
    ) -> Result<(), &'static str> {
        let mut v = self.lock();
        v.rig.control(request, seat)?;
        v.subject = None;
        v.revision = v.revision.wrapping_add(1);
        Ok(())
    }

    #[cfg(any(test, feature = "gpu"))]
    /// Publish one playerless camera from the same rig used for guest draws.
    pub(super) fn host_pose(
        &self,
        eyes: impl Iterator<Item = ObserverEye> + Clone,
    ) -> Option<ObserverView> {
        let mut v = self.lock();
        v.playerless = true;
        v.available = eyes.clone().fold(0, |mask, eye| mask | (1 << eye.seat));
        let subject = match v.rig.seat {
            Some(seat) => eyes.clone().find(|e| e.seat == seat),
            None => eyes.clone().next(),
        };
        if let Some((seat, pose)) = monitor_subject(&v.rig) {
            v.subject = Some(seat);
            let requested = v.rig.mode;
            let pose = v.rig.resolve_monitor(requested, pose);
            return Some(ObserverView {
                position: pose.position,
                forward: pose.axes().0,
                fov: vertical_fov(v.rig.reported_pose()?.fov),
                hide_subject: None,
            });
        }
        v.subject = subject.map(|e| e.seat);
        let eye = subject.and_then(|eye| Some((Pose::looking(eye.position, eye.aim)?, eye.fov)));
        let body = subject.and_then(|eye| Pose::looking(eye.position, eye.facing));
        let mode = v.rig.mode;
        let first_person = matches!(mode, CameraMode::Default | CameraMode::FirstPerson);
        let lens = first_person.then_some(eye).flatten();
        let pose = v.rig.resolve(mode, body, lens)?;
        Some(ObserverView {
            position: pose.position,
            forward: subject
                .filter(|_| first_person)
                .map_or_else(|| pose.axes().0, |eye| eye.aim),
            fov: vertical_fov(v.rig.reported_pose()?.fov),
            hide_subject: v.subject.filter(|_| first_person),
        })
    }

    fn guest_pose(&self, guest: &runtime::Guest, views: &[u32]) -> Option<(Pose, Option<f32>)> {
        let mut v = self.lock();
        v.drawing.fill(None);
        let viewports = views.iter().filter_map(|&view| {
            let (seat, _) = local_view(guest, view)?;
            Some((seat, view))
        });
        v.available = viewports
            .clone()
            .fold(0, |mask, (seat, _)| mask | (1 << seat));
        let selected = match v.rig.seat {
            Some(wanted) => viewports.clone().find(|&(seat, _)| seat == wanted),
            None => viewports.clone().next(),
        };
        // A camera on a monitor seat: the guest has no biped there, and the
        // film's own monitor records give the pose.
        if let Some((seat, pose)) = monitor_subject(&v.rig) {
            v.subject = Some(seat);
            v.crop_seat = None;
            let requested = v.rig.mode;
            let pose = v.rig.resolve_monitor(requested, pose);
            return Some((pose, Some(v.rig.reported_pose()?.fov)));
        }
        let mode = v.mode();
        v.crop_seat = selected
            .filter(|_| mode == CameraMode::FirstPerson)
            .map(|(seat, _)| seat);
        let primary = views.first().copied();
        let template = selected.map(|(_, view)| view).or(primary);
        let retail = template.and_then(|view| retail_view(guest, view));
        if matches!(mode, CameraMode::Default | CameraMode::FirstPerson) {
            v.subject = selected.map(|(seat, _)| seat);
            if let Some(retail) = retail {
                v.rig.use_view(retail);
            }
            return None;
        }
        let body = if mode == CameraMode::Free && v.rig.pose.is_some() {
            None
        } else {
            let subject = subject(guest, v.rig.seat, v.rig.player.as_deref());
            v.subject = subject.map(|(seat, _)| seat);
            subject.map(|(_, pose)| pose)
        };
        let pose = v.rig.resolve(mode, body, retail)?;
        Some((pose, Some(v.rig.reported_pose()?.fov)))
    }

    fn drawn(&self, seat: u8, rect: [u32; 4]) {
        self.lock().drawing[usize::from(seat)] = Some(rect);
    }

    /// Absolute back-buffer pixels, left/top/width/height. A removed
    /// split-screen player must never inherit another player's image.
    pub fn present_crop(&self) -> Option<[u32; 4]> {
        let mut v = self.lock();
        let presented = std::mem::take(&mut v.drawing);
        v.crop_seat.and_then(|seat| presented[usize::from(seat)])
    }

    fn viewport_fov(&self, seat: Option<u8>) -> Option<f32> {
        let v = self.lock();
        match v.mode() {
            CameraMode::Default => v.rig.fov,
            CameraMode::FirstPerson if seat.is_some() && seat == v.crop_seat => v.rig.fov,
            _ => None,
        }
    }

    fn alters_render(&self) -> bool {
        let v = self.lock();
        v.hide_hud
            || v.rig.fov.is_some()
            || matches!(
                v.mode(),
                CameraMode::Free | CameraMode::Follow | CameraMode::Orbit
            )
    }
}

#[cfg(any(test, feature = "gpu"))]
#[derive(Clone, Copy)]
pub(super) struct ObserverEye {
    pub seat: u8,
    pub position: [f32; 3],
    pub aim: [f32; 3],
    pub facing: [f32; 3],
    pub fov: f32,
}

#[cfg(any(test, feature = "gpu"))]
pub(super) struct ObserverView {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub fov: f32,
    pub hide_subject: Option<u8>,
}

/// The seat and pose a camera on a monitor seat follows or looks from now,
/// when the rig names such a seat in follow, orbit or first person
/// ([`forge_film::subject`]). A seat that is not a monitor at this moment
/// is the guest's to give; no monitor seat hides any body.
fn monitor_subject(rig: &ReplayCamera) -> Option<(u8, Pose)> {
    let found = rig.seat.zip(forge_film::view_of(rig.mode)).and_then(|(seat, view)| {
        forge_film::subject(seat, view)
    });
    let Some(subject) = found else {
        forge_film::release();
        return None;
    };
    let pose = Pose {
        position: subject.pose.position,
        yaw: subject.pose.yaw,
        pitch: subject.pose.pitch,
    };
    Some((subject.seat, pose))
}

fn subject(guest: &runtime::Guest, seat: Option<u8>, name: Option<&str>) -> Option<(u8, Pose)> {
    let players = live_telemetry::players(guest).ok()?;
    players.iter().find_map(|player| {
        let slot = u8::try_from(player.datum & 0xffff).ok()?;
        if seat.is_some_and(|seat| seat != slot)
            || seat.is_none() && name.is_some_and(|name| name != player.player.name)
        {
            return None;
        }
        let unit = live_telemetry::object(guest, player.unit).ok()??;
        if unit.vitality.is_some_and(|(body, _)| body <= 0.0) {
            return None;
        }
        let mut position = unit.pos.map(|v| v as f32);
        let crouch = unit.motion.crouch.unwrap_or_default() as f32;
        position[2] += if unit.vehicle.is_some() {
            0.85
        } else {
            0.64 - 0.22 * crouch
        };
        Some((
            slot,
            Pose::looking(position, unit.forward.map(|v| v as f32))?,
        ))
    })
}

pub(super) fn install(b: &mut Builder, session: &Session) {
    let state = session.lock();
    if state.header.version < 3 || state.header.meta.source != film::Source::Local {
        return;
    }
    let camera = match &state.mode {
        Mode::Replay(replay) => Some(replay.camera.clone()),
        Mode::Record(_) => None,
    };
    // Keep the recorded dispatch plan intact for existing films.
    b.raw().observe_before(
        &points::RENDER_GAME_VIEW,
        "film player viewport",
        move |o| {
            let Some(camera) = &camera else { return };
            let Some(view) = o.arg_u32("view") else {
                return;
            };
            if let Some((seat, rect)) = local_view(&*o.guest(), view) {
                camera.drawn(seat, rect);
            }
        },
    );
}

fn local_view(m: &dyn GuestMemory, view: u32) -> Option<(u8, [u32; 4])> {
    if m.u32(view).ok()? != 0 {
        return None;
    }
    let user = m.u32(view.checked_add(8)?).ok()?;
    let player = crate::coop_look::view::subject(m, user as usize)
        .ok()?
        .player
        .index();
    let seat = u8::try_from(player).ok().filter(|&seat| seat < 16)?;
    // The view's first camera has signed short edges in top/left/bottom/right order.
    let mut raw = [0; 8];
    m.read(view.checked_add(0x0c + 0x30)?, &mut raw).ok()?;
    let edge = |at: usize| u32::try_from(i16::from_le_bytes([raw[at], raw[at + 1]])).ok();
    let (top, left, bottom, right) = (edge(0)?, edge(2)?, edge(4)?, edge(6)?);
    (right > left && bottom > top).then(|| (seat, [left, top, right - left, bottom - top]))
}

/// Appended after simulation features so its private allocation cannot move
/// any addresses carried by a film. This feature is presentation-only.
pub(super) const FEATURE: &str = "theater-camera";

pub struct RenderHooks(pub(super) FilmCamera);

impl Session {
    pub fn camera_render_hooks(&self) -> Option<RenderHooks> {
        let state = self.lock();
        let Mode::Replay(replay) = &state.mode else {
            return None;
        };
        (replay.render_every != 0 && !replay.camera.playerless())
            .then(|| RenderHooks(replay.camera.clone()))
    }
}

impl hooks::Feature for RenderHooks {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Theater presentation")
            .doc("docs/game/theater.md")
            .presentation_only()
    }

    fn build(&self, b: &mut Builder) -> Result<(), hooks::BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let scratch = b.guest_alloc(2 * VIEW_STRIDE, runtime::guest::Permissions::RW)?;
        let camera = self.0.clone();
        b.raw()
            .before(&points::RENDER_VIEWS, "draw Theater camera", move |call| {
                draw_world_view(call, scratch, |guest, views| {
                    camera.guest_pose(guest, views)
                });
                Ok(Flow::Continue)
            })
            .context_only();

        let camera = self.0.clone();
        b.raw()
            .before(
                &points::RENDER_GAME_VIEW,
                "Theater viewport lens",
                move |call| {
                    let arg = call.cpu.esp.wrapping_add(4);
                    let mut guest = call.ctx.memory();
                    if let Ok(view) = guest.u32(arg)
                        && guest.u32(view).ok() == Some(0)
                    {
                        let slot = local_view(&*guest, view);
                        if let Some((seat, rect)) = slot {
                            camera.drawn(seat, rect);
                        }
                        if let Some(fov) = camera.viewport_fov(slot.map(|(seat, _)| seat)) {
                            let mut copy = [0; VIEW_STRIDE as usize];
                            if GuestMemory::read(&*guest, view, &mut copy).is_ok() {
                                put_fov(&mut copy, vertical_fov(fov));
                                let private = scratch + VIEW_STRIDE;
                                if GuestMemory::write(&mut *guest, private, &copy).is_ok() {
                                    let _ = guest.set_u32(arg, private);
                                }
                            }
                        }
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only()
            .after_feature("coop-look-prediction");

        let camera = self.0.clone();
        // Hold the game's HUD flag only for the duration of its draw.
        let hud_flag = Arc::new(Mutex::new(None));
        let saved_flag = hud_flag.clone();
        b.raw()
            .before(&points::UI_HUD_DRAW, "hide Theater HUD", move |call| {
                if camera.lock().hide_hud {
                    let mut guest = call.ctx.memory();
                    if let Some(addr) = guest
                        .u32(crate::cheats::HUD_GLOBALS)
                        .ok()
                        .filter(|&base| base != 0)
                        .and_then(|base| base.checked_add(crate::cheats::HUD_SHOWN))
                        && let Ok(shown) = guest.u8(addr)
                        && guest.set_u8(addr, 0).is_ok()
                    {
                        *saved_flag.lock().unwrap_or_else(|e| e.into_inner()) = Some((addr, shown));
                    }
                }
                Ok(Flow::Continue)
            })
            .after_feature("render-aspect")
            .after_feature("native-hud")
            .optional();
        b.raw()
            .after(&points::UI_HUD_DRAW, "restore game HUD flag", move |call| {
                if let Some((addr, shown)) =
                    hud_flag.lock().unwrap_or_else(|e| e.into_inner()).take()
                {
                    let _ = call.ctx.memory().set_u8(addr, shown);
                }
                Ok(())
            })
            .optional();

        // The render's private random stream, shared with `render_rng`'s
        // feature: whichever enters first enters the frame.
        let seeds = crate::render_rng::shared();
        let frame_seeds = seeds.clone();
        let camera = self.0.clone();
        b.raw()
            .before(
                &points::RENDER_FRAME,
                "enter Theater render RNG",
                move |call| {
                    let mut guest = call.ctx.memory();
                    if camera.alters_render()
                        && sim_bridge::sim_frame::frame_kind(
                            &sim_bridge::sim_frame::Addrs::HALO2,
                            &*guest,
                        ) == sim_bridge::sim_frame::FrameKind::World
                    {
                        frame_seeds
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .enter(&mut *guest);
                    }
                    Ok(Flow::Continue)
                },
            )
            .after_feature(crate::bot_glue::ID);
        b.raw()
            .after(
                &points::RENDER_FRAME,
                "restore Theater sim RNG",
                move |call| {
                    let mut guest = call.ctx.memory();
                    seeds
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .leave(&mut *guest);
                    Ok(())
                },
            )
            .after_feature(crate::bot_glue::ID);
        Ok(())
    }
}

#[cfg(test)]
pub(super) fn with_render_seeds(
    guest: &mut runtime::Guest,
    draw: impl FnOnce(&mut runtime::Guest),
) {
    let mut seeds = crate::render_rng::Seeds::default();
    seeds.enter(guest);
    draw(guest);
    seeds.leave(guest);
}
