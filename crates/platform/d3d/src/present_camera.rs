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

#[derive(Clone)]
pub enum Present {
    Guest {
        image: Option<Arc<Image>>,
        reference: bool,
    },
    Synthetic {
        /// Projection from the last genuine source image at this blend fraction.
        frozen: Option<Arc<Image>>,
        reference: bool,
    },
}

#[cfg(any(feature = "gpu", test))]
impl Present {
    pub(crate) fn synthetic(
        fraction: f32,
        previous: Option<&Arc<Image>>,
        current: Option<&Arc<Image>>,
    ) -> Self {
        Self::Synthetic {
            frozen: if fraction >= 1.0 { current } else { previous }.cloned(),
            reference: false,
        }
    }
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

    #[test]
    fn synthetic_frames_hold_the_source_projection_until_the_endpoint() {
        let image = |n: u32| {
            Arc::new(Image {
                views: [None; 4],
                payload: Arc::new(n),
            })
        };
        let previous = image(1);
        let current = image(2);
        let sample = |fraction, previous: Option<&Arc<Image>>, current: Option<&Arc<Image>>| {
            let Present::Synthetic { frozen, .. } = Present::synthetic(fraction, previous, current)
            else {
                panic!("an interpolated image must be synthetic");
            };
            frozen.map(|frame| *frame.payload.downcast_ref::<u32>().unwrap())
        };
        assert_eq!(sample(0.0, Some(&previous), Some(&current)), Some(1));
        assert_eq!(sample(0.75, Some(&previous), Some(&current)), Some(1));
        assert_eq!(sample(1.0, Some(&previous), Some(&current)), Some(2));
        assert_eq!(sample(0.75, None, Some(&current)), None);
    }
}
