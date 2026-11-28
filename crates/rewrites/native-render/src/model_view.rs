//! A menu model widget's view as the object stage draws it
//! (docs/graphics/native-menus-overlay.md, "Model views"): the render
//! camera the guest's view `0x0002c560` fills for it, the view constants
//! its objects' vertex programs take, and the scissor of its viewport in
//! the overlay's target. Pure but [`view_inputs`]; the overlay plan keeps
//! one [`PlanModelView`] per view, in list order.
//!
//! `0x0002c560` builds the record at its ESP `+0x38` and copies it whole
//! (`0x1d` words) to the render camera `0x004b9da0` (`0x0002c703`):
//!
//! | Field | From |
//! |---|---|
//! | position `+0`, forward `+0xc`, up `+0x18` | the call's EAX, fifth argument and ECX: the widget's centre and axes |
//! | vertical FOV `+0x28` | the sixth argument |
//! | viewport `+0x30` (top, left, bottom, right) | the first four arguments (left, top, right, bottom) |
//! | near `+0x40`, far `+0x44` | `0x00467014`, `0x00467018` (`engine::ui_model::VIEW_NEAR`) |
//!
//! Its paired projection (`0x0002fd90` into `0x004b9e14`) is the view's
//! `OverlayCamera::projection`, `0x0002fb70`'s scales and offsets with the
//! game's widescreen and display factors in them
//! (`scene_extract::ui::UiModel::with_lens`): the camera's
//! `projection_xy`, so the lens is the guest's, not
//! [`RenderCamera::projection`]'s from the FOV and the viewport alone.

use frame_scene::{ClipRect, OverlayModelView};

use crate::camera::RenderCamera;
use crate::overlay::{Scissor, scissor};

/// Why a model view cannot reach the object stage: the overlay plan stops
/// on it by name (`unimplemented!`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The record would not be a camera: a FOV, plane or axis that does not
    /// make one ([`RenderCamera::is_usable`]).
    Camera,
    /// The viewport does not fit the record's `i16` edges, or leaves no
    /// pixel of the target.
    Clip,
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Refusal::Camera => "model_view_camera",
            Refusal::Clip => "model_view_clip",
        }
    }
}

/// The render camera record `0x0002c560` fills for `view` drawn into
/// `viewport` (canvas pixels), as `scene_extract::layout::camera::read`
/// reads it back with no paired projection.
pub fn camera(view: &OverlayModelView, viewport: &ClipRect) -> Result<RenderCamera, Refusal> {
    let edge = |v: Option<u32>| v.and_then(|v| i16::try_from(v).ok()).ok_or(Refusal::Clip);
    let left = edge(Some(viewport.x))?;
    let top = edge(Some(viewport.y))?;
    let right = edge(viewport.x.checked_add(viewport.width))?;
    let bottom = edge(viewport.y.checked_add(viewport.height))?;
    let c = &view.camera;
    let camera = RenderCamera {
        position: c.position,
        forward: c.forward,
        up: c.up,
        fov: c.fov_y,
        projection_xy: c.projection,
        viewport: [top, left, bottom, right],
        near: c.planes[0],
        far: c.planes[1],
    };
    if camera.is_usable() { Ok(camera) } else { Err(Refusal::Camera) }
}

/// A model view between the overlay's batches, for the object stage.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanModelView {
    /// The batches drawn before it (an index into
    /// `OverlayPlan::batches`).
    pub before: u32,
    /// The blurs before it in the list (an index into
    /// `OverlayPlan::blurs`): a blur and a view after the same batch keep
    /// their list order.
    pub blurs: u32,
    /// The canvas the camera's viewport is in, guest pixels.
    pub guest: [u32; 2],
    /// The viewport in the target's pixels.
    pub scissor: Scissor,
    pub camera: RenderCamera,
    /// The objects' datum handles, in the widget's order.
    pub objects: Vec<u32>,
}

/// The view of an overlay item: `view` into `clip` (pixels of a `guest`
/// canvas) drawn into a `target` of the renderer's size, after `before`
/// batches and `blurs` blurs.
pub fn plan(
    view: &OverlayModelView,
    clip: &ClipRect,
    guest: [u32; 2],
    target: [u32; 2],
    before: u32,
    blurs: u32,
) -> Result<PlanModelView, Refusal> {
    let camera = camera(view, clip)?;
    let scissor = scissor(clip, guest, target).ok_or(Refusal::Clip)?;
    Ok(PlanModelView { before, blurs, guest, scissor, camera, objects: view.objects.clone() })
}

/// The view constants a model view's object draws take on a `guest`
/// canvas drawn into a `target` of the renderer's size: the camera's
/// (`ViewInputs::from_camera`, the view-projection rows and the clip
/// range from its planes) with the viewport transform `c[10]`, `c[11]`
/// of the viewport rather than the whole canvas: scale `(w / 2, -h / 2)`,
/// offset `(left + w / 2, top + h / 2)`, guest pixels, y down.
#[cfg(feature = "gpu")]
pub fn view_inputs(camera: &RenderCamera, guest: [u32; 2], target: [u32; 2]) -> crate::program::ViewInputs {
    let mut inputs = crate::program::ViewInputs::from_camera(camera, (guest[0], guest[1]));
    let [top, left, bottom, right] = camera.viewport.map(f32::from);
    let (w, h) = (right - left, bottom - top);
    inputs.constants[10] = [w * 0.5, -h * 0.5, 0.0, 0.0];
    inputs.constants[11] = [left + w * 0.5, top + h * 0.5, 0.0, 0.0];
    inputs.size = (target[0], target[1]);
    inputs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{RENDER_CAMERA, read};
    use frame_scene::OverlayCamera;
    use scene_extract::testing::Mem;

    /// The widget of `render_bridge::native_hud::model`'s tests: its camera
    /// call's arguments, left, top, right, bottom `310, 228, 330, 252`.
    fn widget() -> (OverlayModelView, ClipRect) {
        let view = OverlayModelView {
            camera: OverlayCamera {
                position: [0.0, 1.0, 2.0],
                forward: [0.0, 1.0, 0.0],
                up: [0.0, 0.0, 1.0],
                fov_y: 0.75,
                planes: [0.0625, 200.0],
                projection: None,
            },
            objects: vec![42, 17],
        };
        (view, ClipRect { x: 310, y: 228, width: 20, height: 24 })
    }

    /// The record as `0x0002c560` writes it from the call's arguments
    /// (`0x0002c57e` to `0x0002c5fb`), copied to the render camera.
    fn guest_record(view: &OverlayModelView, [left, top, right, bottom]: [u16; 4]) -> Mem {
        let mut m = Mem::default();
        let c = &view.camera;
        for (i, v) in c.position.iter().chain(&c.forward).chain(&c.up).enumerate() {
            m.f(RENDER_CAMERA + 4 * i as u32, *v);
        }
        m.f(RENDER_CAMERA + 0x28, c.fov_y);
        m.f(RENDER_CAMERA + 0x2c, 1.0);
        let pair = |lo: u16, hi: u16| u32::from(lo) | u32::from(hi) << 16;
        for at in [0x30, 0x38] {
            m.w(RENDER_CAMERA + at, pair(top, left));
            m.w(RENDER_CAMERA + at + 4, pair(bottom, right));
        }
        m.f(RENDER_CAMERA + 0x40, c.planes[0]);
        m.f(RENDER_CAMERA + 0x44, c.planes[1]);
        // `0x0002fd90`'s scales and offsets beside it, at `0x004b9e14`.
        if let Some(p) = c.projection {
            for (offset, v) in [0x78, 0x8c, 0x98, 0x9c].into_iter().zip(p) {
                m.f(0x004b_9e14 + offset, v);
            }
        }
        m
    }

    #[test]
    fn a_widgets_camera_is_the_record_the_guest_fills() {
        let (view, clip) = widget();
        let guest = read(&guest_record(&view, [310, 228, 330, 252]), RENDER_CAMERA).unwrap();
        let native = camera(&view, &clip).unwrap();
        assert_eq!(native, guest);
        assert_eq!(native.view_projection(), guest.view_projection(), "the same matrices");
        assert_eq!(native.viewport, [228, 310, 252, 330]);
    }

    /// A view carrying the projection its call builds draws with that lens,
    /// as the render camera reads with its paired projection; it differs
    /// from the FOV's own when the widescreen factor is in it.
    #[test]
    fn a_widgets_lens_is_the_projection_its_call_builds() {
        let (mut view, clip) = widget();
        let own = camera(&view, &clip).unwrap().projection();
        view.camera.projection = Some([own[0][0] * 0.75, own[1][1], 0.0, 0.0]);
        let guest = read(&guest_record(&view, [310, 228, 330, 252]), RENDER_CAMERA).unwrap();
        let native = camera(&view, &clip).unwrap();
        assert_eq!(native, guest);
        assert_eq!(native.projection_xy, view.camera.projection);
        assert_eq!(native.projection()[0][0], own[0][0] * 0.75, "the guest's x scale, not the FOV's");
        assert_eq!(native.view_projection(), guest.view_projection());
    }

    #[test]
    fn the_scissor_is_the_records_viewport_in_the_targets_pixels() {
        let (view, clip) = widget();
        let guest = read(&guest_record(&view, [310, 228, 330, 252]), RENDER_CAMERA).unwrap();
        let [top, left, bottom, right] = guest.viewport.map(|e| e as u32);
        let at_canvas = plan(&view, &clip, [640, 480], [640, 480], 3, 1).unwrap();
        assert_eq!((at_canvas.blurs, at_canvas.guest), (1, [640, 480]));
        assert_eq!(at_canvas.scissor, Scissor { x: left, y: top, width: right - left, height: bottom - top });
        let doubled = plan(&view, &clip, [640, 480], [1280, 960], 3, 0).unwrap();
        assert_eq!(doubled.scissor, Scissor { x: 2 * left, y: 2 * top, width: 2 * (right - left), height: 2 * (bottom - top) });
        assert_eq!((doubled.before, doubled.objects.as_slice(), doubled.camera), (3, &[42, 17][..], guest));
    }

    /// A point straight down the widget's forward axis lands on the
    /// viewport's centre in guest pixels, through the view constants.
    #[cfg(feature = "gpu")]
    #[test]
    fn the_view_constants_put_the_axis_on_the_viewports_centre() {
        let (view, clip) = widget();
        let camera = camera(&view, &clip).unwrap();
        let inputs = view_inputs(&camera, [640, 480], [1280, 960]);
        let whole = crate::program::ViewInputs::from_camera(&camera, (640, 480));
        assert_eq!(inputs.constants[..4], whole.constants[..4], "the view-projection rows are the camera's");
        assert_eq!((inputs.guest_size, inputs.size, inputs.w_clip), ((640, 480), (1280, 960), whole.w_clip));
        let c = &inputs.constants;
        let p = [0.0, 11.0, 2.0, 1.0];
        let dot = |row: [f32; 4]| row.iter().zip(p).map(|(a, b)| a * b).sum::<f32>();
        let w = dot(c[3]);
        let pixel = [c[10][0] * dot(c[0]) / w + c[11][0], c[10][1] * dot(c[1]) / w + c[11][1]];
        assert!((pixel[0] - 320.0).abs() < 1e-3 && (pixel[1] - 240.0).abs() < 1e-3, "{pixel:?}");
        let clip_space = crate::camera::transform(&camera.view_projection(), p);
        assert!((clip_space[3] - 10.0).abs() < 1e-4, "ten units down the axis: {clip_space:?}");
    }

    #[test]
    fn a_view_that_is_not_a_camera_or_does_not_fit_is_refused_by_name() {
        let (view, clip) = widget();
        let mut flat = view.clone();
        flat.camera.fov_y = 0.0;
        assert_eq!(camera(&flat, &clip), Err(Refusal::Camera));
        let mut inverted = view.clone();
        inverted.camera.planes = [200.0, 0.0625];
        assert_eq!(camera(&inverted, &clip), Err(Refusal::Camera));
        let wide = ClipRect { x: 32_760, y: 0, width: 16, height: 16 };
        assert_eq!(camera(&view, &wide), Err(Refusal::Clip));
        assert_eq!(plan(&view, &clip, [0, 0], [640, 480], 0, 0), Err(Refusal::Clip), "no canvas, no scissor");
        assert_eq!((Refusal::Camera.name(), Refusal::Clip.name()), ("model_view_camera", "model_view_clip"));
    }
}
