//! A local film already renders each player's director, weapon and HUD. Select
//! the completed viewport at presentation, after the engine's final draw.
mod redraw;
pub(super) use redraw::PausedRedraw;

use std::sync::{Arc, Mutex};

use super::{Mode, Session};
use engine::GuestMemory;

#[derive(Default)]
struct Views {
    requested: Option<u8>,
    rig: super::ReplayCamera,
    revision: u64,
    guest_view_limit: u32,
    raster_job: Option<redraw::RasterJob>,
    publish: redraw::Publish,
    last_seat: Option<u8>,
    drawing: [Option<[u32; 4]>; 16],
    available: u16,
    host: bool,
}

#[derive(Default, Clone)]
pub struct FilmCamera(Arc<Mutex<Views>>);

impl FilmCamera {
    /// The absolute player slot, independent of pad port or local user index.
    pub fn requested_seat(&self) -> Option<u8> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).requested
    }

    pub fn host_seats(&self, seats: &[u8]) {
        let mut views = self.0.lock().unwrap_or_else(|e| e.into_inner());
        views.host = true;
        views.available = seats
            .iter()
            .filter(|&&seat| seat < 16)
            .fold(0, |mask, &seat| mask | (1u16 << seat));
    }

    pub(super) fn restore(&self, presentation: &super::Presentation) {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        v.requested = presentation.camera_seat.filter(|s| *s < 16);
        v.rig = presentation.camera.clone();
    }

    pub(super) fn save(&self, presentation: &mut super::Presentation) {
        let v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        presentation.camera_seat = v.requested;
        presentation.camera = v.rig.clone();
    }

    pub fn revision(&self) -> u64 {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).revision
    }

    pub fn host_pose(
        &self,
        eyes: &[(u8, [f32; 3], [f32; 3])],
    ) -> Option<(u8, [f32; 3], [f32; 3], bool)> {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let subject = match v.requested {
            Some(seat) => eyes.iter().find(|e| e.0 == seat),
            None => eyes.first(),
        };
        let Some(&(seat, eye, aim)) = subject else {
            let (position, forward) = v.rig.saved_view()?;
            return Some((
                v.requested.or(v.last_seat).unwrap_or(0),
                position,
                forward,
                false,
            ));
        };
        v.last_seat = Some(seat);
        let (position, forward, first_person) = v.rig.host_pose(eye, aim)?;
        Some((seat, position, forward, first_person))
    }

    pub(super) fn control(
        &self,
        request: agent_proto::theater::Camera,
        seat: Option<u8>,
    ) -> Result<(), &'static str> {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if seat.is_some_and(|s| s >= 16 || v.available & (1 << s) == 0) {
            return Err("that seat has no camera view in the current frame");
        }
        v.rig.control(request)?;
        v.requested = seat;
        v.revision = v.revision.wrapping_add(1);
        Ok(())
    }

    fn drawn(&self, seat: u16, rect: [u32; 4]) {
        let mut views = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = views.drawing.get_mut(usize::from(seat)) {
            *slot = Some(rect);
        }
    }

    /// Absolute back-buffer pixels, left/top/width/height. Only views actually
    /// drawn in this frame are eligible: a removed split-screen player must
    /// never inherit another player's image.
    pub fn present_crop(&self) -> Option<[u32; 4]> {
        let mut views = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if views.host {
            return None;
        }
        let presented = std::mem::take(&mut views.drawing);
        views.available = presented.iter().enumerate().fold(0, |mask, (i, rect)| {
            if rect.is_some() {
                mask | (1 << i)
            } else {
                mask
            }
        });
        views
            .requested
            .and_then(|seat| presented[usize::from(seat)])
    }
}

pub(super) fn install(b: &mut hooks::Builder, session: &Session) {
    let state = session.lock();
    if state.header.version < 3 || state.header.meta.source != film::Source::Local {
        return;
    }
    let camera = match &state.mode {
        Mode::Replay(_, replay) => Some(replay.camera.clone()),
        Mode::Record(_) => None,
    };
    // Install the same observer on the recording. The extra dispatch must
    // occur in both schedules even though only playback publishes a viewport.
    b.raw().observe_before(
        &engine::points::RENDER_GAME_VIEW,
        "film player viewport",
        move |o| {
            let Some(camera) = &camera else { return };
            let Some(view) = o.arg_u32("view") else {
                return;
            };
            let guest = o.guest();
            if let Some((seat, rect)) = local_view(&*guest, view) {
                camera.drawn(seat, rect);
            }
        },
    );
}

fn local_view(m: &dyn GuestMemory, view: u32) -> Option<(u16, [u32; 4])> {
    if m.u32(view).ok()? != 0 {
        return None;
    }
    let user = m.u32(view.checked_add(8)?).ok()?;
    let player = crate::coop_look::view::subject(m, user as usize)
        .ok()?
        .player
        .index();
    // render_game_view's first camera is at +0xc. Its viewport is four
    // signed shorts, top/left/bottom/right (render_loop::occlusion::guest).
    let mut raw = [0; 8];
    m.read(view.checked_add(0x0c + 0x30)?, &mut raw).ok()?;
    let edge = |at: usize| u32::try_from(i16::from_le_bytes([raw[at], raw[at + 1]])).ok();
    let (top, left, bottom, right) = (edge(0)?, edge(2)?, edge(4)?, edge(6)?);
    (right > left && bottom > top).then(|| (player, [left, top, right - left, bottom - top]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::theater::{Camera, CameraMode};
    fn request(mode: CameraMode, seat: Option<u8>) -> Camera {
        Camera {
            mode,
            seat,
            player: None,
            look: [0.; 2],
            movement: [0.; 3],
            zoom: 0.,
        }
    }
    #[test]
    fn selection_follows_a_player_only_while_their_view_is_presented() {
        let camera = FilmCamera::default();
        let rect = [0, 240, 640, 240];
        camera.drawn(7, rect);
        assert_eq!(camera.present_crop(), None);
        assert!(
            camera
                .control(request(CameraMode::FirstPerson, Some(0)), Some(0))
                .is_err()
        );
        camera
            .control(request(CameraMode::FirstPerson, Some(7)), Some(7))
            .unwrap();
        camera.drawn(7, rect);
        assert_eq!(camera.present_crop(), Some(rect));
        assert_eq!(camera.present_crop(), None);
        assert!(
            camera
                .control(request(CameraMode::FirstPerson, Some(7)), Some(7))
                .is_err()
        );
        camera.host_seats(&[3, 5]);
        camera
            .control(request(CameraMode::FirstPerson, Some(3)), Some(3))
            .unwrap();
        assert_eq!(camera.present_crop(), None);
        assert_eq!(camera.requested_seat(), Some(3));
    }
    #[test]
    fn host_seek_restores_free_pose_and_first_person_requires_a_live_eye() {
        let camera = FilmCamera::default();
        let eyes = [(5, [12., 5., 1.5], [1., 0., 0.])];
        camera.host_seats(&[5]);
        camera
            .control(request(CameraMode::FirstPerson, Some(5)), Some(5))
            .unwrap();
        assert_eq!(
            camera.host_pose(&eyes),
            Some((5, eyes[0].1, eyes[0].2, true))
        );
        assert!(camera.host_pose(&[]).is_none());
        let mut fly = request(CameraMode::Free, Some(5));
        fly.movement[1] = 2.;
        camera.control(fly, Some(5)).unwrap();
        let view = camera.host_pose(&eyes).unwrap();
        assert_eq!(view, (5, [14., 5., 1.5], [1., 0., 0.], false));
        let mut saved = super::super::Presentation::default();
        camera.save(&mut saved);
        let reopened = FilmCamera::default();
        reopened.restore(&saved);
        assert_eq!(reopened.host_pose(&[]), Some(view));
    }
}
