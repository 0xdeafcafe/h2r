//! Host metadata stays with swap-ring images and queued presents.
use std::any::Any;
use std::cell::RefCell;
use std::sync::Arc;

pub use frame_types::RenderCamera as Camera;

pub type Payload = Arc<dyn Any + Send + Sync>;

#[derive(Clone, Copy, Debug)]
pub struct View {
    pub camera: Camera,
    pub horizontal_factor: f32,
}

pub struct Image {
    pub views: [Option<View>; 4],
    pub payload: Payload,
}

/// How a made (in-between) frame drew one view, against the camera of the
/// genuine image its marks come from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewShift {
    /// The view on the target: top, left, bottom and right as fractions of
    /// it (the order of a mark camera's viewport).
    pub viewport: [f32; 4],
    /// World to world, four rows acting on column vectors: the source
    /// image's camera for this view times this is the camera the frame drew
    /// the view with (`C⁻¹ B`). The identity when it drew the source's.
    pub world: [[f64; 4]; 4],
}

impl ViewShift {
    /// `view_projection` (column-major, `view_projection[column][row]`, as
    /// [`Camera::view_projection`] gives it) times [`Self::world`]: a
    /// projection of the source's tick moved onto the camera the frame was
    /// drawn with.
    pub fn apply(&self, view_projection: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let m = |row: usize, column: usize| f64::from(view_projection[column][row]);
        std::array::from_fn(|column| {
            std::array::from_fn(|row| {
                (0..4).map(|k| m(row, k) * self.world[k][column]).sum::<f64>() as f32
            })
        })
    }

    /// The shift that moves `source`'s view-projection onto `shown`'s
    /// (`C⁻¹ B`), for the view at `source`'s viewport. `None` when
    /// `source`'s cannot be inverted.
    pub fn between(source: &Camera, shown: &Camera) -> Option<Self> {
        let c = invert(row_major(source.view_projection()))?;
        let b = row_major(shown.view_projection());
        let world: [[f64; 4]; 4] = std::array::from_fn(|row| {
            std::array::from_fn(|column| (0..4).map(|k| c[row][k] * b[k][column]).sum())
        });
        Some(Self { viewport: image_fraction(source.viewport), world })
    }

    /// Whether the frame drew the view with the source's own camera.
    pub fn is_identity(&self) -> bool {
        (0..4).all(|i| (0..4).all(|j| self.world[i][j] == if i == j { 1.0 } else { 0.0 }))
    }

    /// The shift for the view at `viewport` (top, left, bottom, right as
    /// fractions of the image) among `shifts`: the one within a hundredth
    /// of it on every side. With a single view on both sides, that one.
    pub fn find(shifts: &[ViewShift], viewport: [f32; 4], views: usize) -> Option<&ViewShift> {
        let near = |s: &&ViewShift| (0..4).all(|i| (s.viewport[i] - viewport[i]).abs() <= 0.01);
        shifts
            .iter()
            .find(near)
            .or_else(|| (shifts.len() == 1 && views == 1).then(|| &shifts[0]))
    }
}

/// The guest image is 640 by 480 guest pixels: what a camera's viewport
/// is in, and what a mark camera's viewport is a fraction of.
const GUEST_IMAGE: [f32; 2] = [640.0, 480.0];

/// Top, left, bottom, right in guest pixels as fractions of the image.
fn image_fraction([top, left, bottom, right]: [i16; 4]) -> [f32; 4] {
    let [w, h] = GUEST_IMAGE;
    [
        f32::from(top) / h,
        f32::from(left) / w,
        f32::from(bottom) / h,
        f32::from(right) / w,
    ]
}

/// A column-major matrix (`m[column][row]`) as rows.
fn row_major(m: [[f32; 4]; 4]) -> [[f64; 4]; 4] {
    std::array::from_fn(|row| std::array::from_fn(|column| f64::from(m[column][row])))
}

/// Gauss-Jordan with partial pivoting.
fn invert(m: [[f64; 4]; 4]) -> Option<[[f64; 4]; 4]> {
    let mut a = m;
    let mut out: [[f64; 4]; 4] =
        std::array::from_fn(|i| std::array::from_fn(|j| if i == j { 1.0 } else { 0.0 }));
    for col in 0..4 {
        let pivot = (col..4).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if !a[pivot][col].is_finite() || a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        out.swap(col, pivot);
        let p = a[col][col];
        for j in 0..4 {
            a[col][j] /= p;
            out[col][j] /= p;
        }
        for row in (0..4).filter(|&r| r != col) {
            let f = a[row][col];
            for j in 0..4 {
                a[row][j] -= f * a[col][j];
                out[row][j] -= f * out[col][j];
            }
        }
    }
    out.iter().flatten().all(|v| v.is_finite()).then_some(out)
}

/// How a display frame's marks were placed (`render.stats`'
/// `display_rate.marks_moved` and `marks_held`), counted per view of the
/// marks' image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkShifts {
    /// Views whose marks were moved onto the camera the frame drew them
    /// with.
    pub moved: u32,
    /// Views whose marks keep the image's camera although the frame may
    /// have drawn another: no drawn view at its viewport, or a camera that
    /// cannot be inverted.
    pub held: u32,
}

#[derive(Clone)]
pub enum Present {
    Guest {
        image: Option<Arc<Image>>,
        reference: bool,
    },
    /// An in-between frame the host made (docs/game/high-framerate.md,
    /// "World marks on made frames").
    Synthetic {
        /// The genuine image the marks come from: the newest, or while a
        /// blended replay is nearer the frame before, that one.
        source: Option<Arc<Image>>,
        /// For each view the frame drew with another camera than the
        /// source's, how to move the source's camera onto it. A view with
        /// none drew the source's own.
        shifts: Vec<ViewShift>,
        /// The blend fraction: 0 is the frame before, 1 the newest.
        fraction: f32,
        reference: bool,
    },
}

impl Present {
    /// A display frame's present (docs/game/high-framerate.md, "World
    /// marks on made frames"): marks from `source`, each of its views
    /// moved onto the camera in `shown` at the same viewport (the frame's
    /// game views, as drawn). A view drawn with the source's own camera
    /// needs no shift. With one view on each side they pair whatever
    /// their viewports.
    pub fn display_frame(source: Option<Arc<Image>>, shown: &[Camera], fraction: f32) -> (Self, MarkShifts) {
        let mut shifts = Vec::new();
        let mut counts = MarkShifts::default();
        if let Some(image) = &source {
            let views: Vec<&View> = image.views.iter().flatten().collect();
            for view in &views {
                let drawn = shown
                    .iter()
                    .find(|c| c.viewport == view.camera.viewport)
                    .or_else(|| (views.len() == 1 && shown.len() == 1).then(|| &shown[0]));
                match drawn {
                    Some(drawn) if *drawn == view.camera => {}
                    Some(drawn) => match ViewShift::between(&view.camera, drawn) {
                        Some(shift) => {
                            shifts.push(shift);
                            counts.moved += 1;
                        }
                        None => counts.held += 1,
                    },
                    None => counts.held += 1,
                }
            }
        }
        let present = Self::Synthetic { source, shifts, fraction, reference: false };
        (present, counts)
    }
}

/// The image of the guest present this thread is making (inside
/// [`with`]): what a frame owner's display frames take their marks from.
pub fn guest_image() -> Option<Arc<Image>> {
    CURRENT.with_borrow(|present| match present {
        Some(Present::Guest { image, .. }) => image.clone(),
        _ => None,
    })
}

pub(crate) struct Images<S> {
    source: Option<fn() -> Option<Image>>,
    building: Option<(S, Image)>,
    displayed: Vec<(S, Arc<Image>)>,
}

impl<S: Copy + Eq> Images<S> {
    pub(crate) fn new() -> Self {
        Self {
            source: None,
            building: None,
            displayed: Vec::new(),
        }
    }

    pub(crate) fn set_source(&mut self, source: fn() -> Option<Image>) {
        self.source = Some(source);
    }

    pub(crate) fn capture(&mut self, back: S) {
        if let Some(image) = self.source.and_then(|source| source()) {
            self.building = Some((back, image));
        }
    }

    pub(crate) fn finish(&mut self, surface: S) {
        if let Some((back, image)) = self.building.take()
            && back == surface
        {
            self.invalidate(surface);
            self.displayed.push((surface, Arc::new(image)));
        }
    }

    pub(crate) fn color_written(&mut self, surface: S) {
        self.invalidate(surface);
        // A previous BYPASSCOPY did not consume the back image. Another
        // write to the same surface abandons that sample.
        if self.building.as_ref().is_some_and(|(back, _)| *back == surface) {
            self.building = None;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.building = None;
        self.displayed.clear();
        if let Some(source) = self.source {
            let _ = source();
        }
    }

    pub(crate) fn get(&self, surface: S) -> Option<Arc<Image>> {
        self.displayed
            .iter()
            .find(|(s, _)| *s == surface)
            .map(|(_, image)| image.clone())
    }

    pub(crate) fn copy(&mut self, from: S, to: S) {
        let image = self.get(from);
        self.invalidate(to);
        if let Some(image) = image {
            self.displayed.push((to, image));
        }
    }

    pub(crate) fn invalidate(&mut self, surface: S) {
        self.displayed.retain(|(s, _)| *s != surface);
    }
}

thread_local! {
    static CURRENT: RefCell<Option<Present>> = const { RefCell::new(None) };
}

pub fn current() -> Option<Present> {
    CURRENT.with_borrow(Clone::clone)
}

/// Runs `present` with `metadata` as this thread's [`current`] present,
/// restoring the one before after it. A present hook called inside reads
/// it.
pub fn with<R>(metadata: Option<Present>, present: impl FnOnce() -> R) -> R {
    struct Restore(Option<Present>);

    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = CURRENT.replace(self.0.take());
        }
    }

    let _restore = Restore(CURRENT.replace(metadata));
    present()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift(viewport: [f32; 4], world: [[f64; 4]; 4]) -> ViewShift {
        ViewShift { viewport, world }
    }

    #[test]
    fn a_shift_multiplies_the_projection_on_the_world_side() {
        // Columns: x scaled by 2, then a world move of +3 in x.
        let mut vp = [[0.0f32; 4]; 4];
        vp[0][0] = 2.0;
        vp[1][1] = 1.0;
        vp[2][2] = 1.0;
        vp[3][3] = 1.0;
        let mut world = [[0.0; 4]; 4];
        for (i, row) in world.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        world[0][3] = 3.0;
        let out = shift([0.0, 0.0, 1.0, 1.0], world).apply(vp);
        // The world point (1, 0, 0) is at x = 2 * (1 + 3).
        let x: f32 = (0..4).map(|k| out[k][0] * [1.0, 0.0, 0.0, 1.0][k]).sum();
        assert_eq!(x, 8.0);
        assert_eq!(out[3][3], 1.0);
    }

    #[test]
    fn shifts_are_found_by_their_view() {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let top = shift([0.0, 0.0, 0.5, 1.0], id);
        let bottom = shift([0.5, 0.0, 1.0, 1.0], id);
        let both = [top, bottom];
        assert_eq!(
            ViewShift::find(&both, [0.502, 0.0, 1.0, 0.999], 2),
            Some(&bottom)
        );
        assert_eq!(ViewShift::find(&both, [0.0, 0.0, 1.0, 1.0], 2), None);
        // One view each side pairs even when the rectangles disagree.
        assert_eq!(ViewShift::find(&[top], [0.0, 0.0, 1.0, 1.0], 1), Some(&top));
        assert_eq!(ViewShift::find(&[top], [0.0, 0.0, 1.0, 1.0], 2), None);
    }

    fn camera(viewport: [i16; 4], yaw: f32, x: f32) -> Camera {
        Camera {
            position: [x, 2.0, 1.5],
            forward: [yaw.cos(), yaw.sin(), 0.0],
            up: [0.0, 0.0, 1.0],
            fov: 1.2,
            projection_xy: None,
            viewport,
            near: 0.05,
            far: 500.0,
        }
    }

    const FULL: [i16; 4] = [0, 0, 480, 640];
    const TOP: [i16; 4] = [0, 0, 240, 640];
    const BOTTOM: [i16; 4] = [240, 0, 480, 640];

    fn image(cameras: &[Camera]) -> Arc<Image> {
        let mut views = [None; 4];
        for (slot, camera) in views.iter_mut().zip(cameras) {
            *slot = Some(View { camera: *camera, horizontal_factor: 1.0 });
        }
        Arc::new(Image { views, payload: Arc::new(()) })
    }

    fn clip(m: [[f32; 4]; 4], p: [f32; 3]) -> [f32; 2] {
        let v = [p[0], p[1], p[2], 1.0];
        let row = |r: usize| (0..4).map(|c| m[c][r] * v[c]).sum::<f32>();
        [row(0) / row(3), row(1) / row(3)]
    }

    #[test]
    fn a_shift_between_two_cameras_projects_the_source_as_the_shown_one() {
        let source = camera(FULL, 0.3, 0.0);
        let shown = camera(FULL, 0.34, 0.2);
        let shift = ViewShift::between(&source, &shown).unwrap();
        assert_eq!(shift.viewport, [0.0, 0.0, 1.0, 1.0]);
        let moved = shift.apply(source.view_projection());
        for p in [[10.0, 6.0, 2.0], [7.0, 1.0, 0.5], [30.0, 15.0, 4.0]] {
            let (a, b) = (clip(moved, p), clip(shown.view_projection(), p));
            assert!((a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4, "{a:?} {b:?}");
            // The source alone puts it elsewhere.
            assert!((clip(source.view_projection(), p)[0] - b[0]).abs() > 1e-3);
        }
        let same = ViewShift::between(&source, &source).unwrap();
        for i in 0..4 {
            for j in 0..4 {
                assert!((same.world[i][j] - if i == j { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn a_display_frame_moves_each_view_drawn_with_another_camera() {
        let (top, bottom) = (camera(TOP, 0.3, 0.0), camera(BOTTOM, 1.0, 4.0));
        let turned = camera(TOP, 0.35, 0.0);
        let (present, counts) = Present::display_frame(Some(image(&[top, bottom])), &[turned, bottom], 0.25);
        assert_eq!(counts, MarkShifts { moved: 1, held: 0 });
        let Present::Synthetic { source, shifts, fraction, reference } = present else { panic!() };
        assert!(source.is_some() && !reference);
        assert_eq!(fraction, 0.25);
        assert_eq!(shifts.len(), 1);
        assert_eq!(shifts[0].viewport, [0.0, 0.0, 0.5, 1.0]);
        // The window finds the top view's shift; the bottom drew its own.
        assert!(ViewShift::find(&shifts, [0.0, 0.0, 0.5, 1.0], 2).is_some());
        assert!(ViewShift::find(&shifts, [0.5, 0.0, 1.0, 1.0], 2).is_none());
    }

    #[test]
    fn a_view_with_no_drawn_camera_keeps_the_image_camera() {
        let (top, bottom) = (camera(TOP, 0.3, 0.0), camera(BOTTOM, 1.0, 4.0));
        let (present, counts) = Present::display_frame(Some(image(&[top, bottom])), &[camera(FULL, 0.3, 0.0)], 1.0);
        assert_eq!(counts, MarkShifts { moved: 0, held: 2 });
        let Present::Synthetic { shifts, .. } = present else { panic!() };
        assert!(shifts.is_empty());
        // One view each side pairs even when the rectangles disagree.
        let (_, counts) = Present::display_frame(Some(image(&[top])), &[camera(FULL, 0.4, 0.0)], 0.5);
        assert_eq!(counts, MarkShifts { moved: 1, held: 0 });
        // No image: nothing to place.
        let (present, counts) = Present::display_frame(None, &[top], 0.5);
        assert_eq!(counts, MarkShifts::default());
        assert!(matches!(present, Present::Synthetic { source: None, .. }));
    }

    #[test]
    fn the_guest_image_is_the_present_being_made() {
        let shown = image(&[camera(FULL, 0.0, 0.0)]);
        assert!(guest_image().is_none());
        let inside = with(Some(Present::Guest { image: Some(Arc::clone(&shown)), reference: false }), guest_image);
        assert!(inside.is_some_and(|i| Arc::ptr_eq(&i, &shown)));
        assert!(guest_image().is_none());
        let (synthetic, _) = Present::display_frame(Some(shown), &[], 0.5);
        assert!(with(Some(synthetic), guest_image).is_none());
    }
}
