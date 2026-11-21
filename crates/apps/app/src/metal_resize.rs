//! Keep the next Metal drawable in the same Core Animation transaction as
//! the AppKit view's resize.

use objc2::rc::Retained;
use objc2_app_kit::{NSView, NSViewLayerContentsRedrawPolicy};
use objc2_quartz_core::{CAMetalLayer, CATransaction, kCAGravityResizeAspect};
use winit::dpi::PhysicalSize;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

pub(crate) struct MetalResize {
    view: Retained<NSView>,
    layer: Retained<CAMetalLayer>,
}

impl MetalResize {
    pub(crate) fn new(window: &Window, surface: &wgpu::Surface<'_>) -> Option<Self> {
        // SAFETY: The window thread only retains the layer through this guard.
        let surface = unsafe { surface.as_hal::<wgpu::hal::api::Metal>() }?;
        let layer = surface.render_layer().lock().clone();
        drop(surface);
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // SAFETY: The window handle guarantees a live NSView on this thread.
        let view = unsafe { Retained::retain(handle.ns_view.cast::<NSView>().as_ptr()) }?;

        // AppKit's drawRect feeds winit's RedrawRequested during live resize.
        view.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::DuringViewResize);
        // SAFETY: Core Animation exports this immutable gravity constant.
        layer.setContentsGravity(unsafe { kCAGravityResizeAspect });
        Some(Self { view, layer })
    }

    pub(crate) fn drawable_size(&self) -> PhysicalSize<u32> {
        let size = self.view.convertRectToBacking(self.view.bounds()).size;
        PhysicalSize::new(size.width.round() as u32, size.height.round() as u32)
    }

    pub(crate) fn transaction(&self, resized: bool) -> Option<ResizeTransaction<'_>> {
        if !resized && !self.view.inLiveResize() {
            return None;
        }
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        // wgpu samples this property at acquire and schedules the drawable
        // on this transaction when the window thread calls queue.present.
        let previous = self.layer.presentsWithTransaction();
        self.layer.setPresentsWithTransaction(true);
        Some(ResizeTransaction {
            layer: &self.layer,
            previous,
        })
    }
}

pub(crate) struct ResizeTransaction<'a> {
    layer: &'a CAMetalLayer,
    previous: bool,
}

impl Drop for ResizeTransaction<'_> {
    fn drop(&mut self) {
        CATransaction::commit();
        self.layer.setPresentsWithTransaction(self.previous);
    }
}
