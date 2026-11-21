//! Host metadata stays with swap-ring images and queued presents.
use std::any::Any;
use std::cell::RefCell;
use std::sync::Arc;

pub use frame_scene::RenderCamera as Camera;

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

pub(crate) fn with<R>(metadata: Option<Present>, present: impl FnOnce() -> R) -> R {
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
}
