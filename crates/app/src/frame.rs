//! The frame handoff between the guest thread and the window thread.
//!
//! Both threads share one wgpu device (the shell creates it and hands the
//! guest a clone of the same `GfxContext`). Frames move through three GPU
//! textures managed by [`TripleBuffer`]:
//!
//! * the guest side ([`FrameSink`]) copies each presented frame into its
//!   back slot (a GPU blit from the D3D back buffer, or a CPU upload for
//!   synthetic sources) and publishes it. It never waits for the display.
//! * the window side ([`FrameSource`]) takes the newest published frame at
//!   each redraw and [`Compositor`] scales it into the window surface.
//!
//! Ordering is by the shared queue: the guest's copy is submitted before the
//! slot is published, and the window's read of a slot is submitted before
//! that slot can return to the guest (the swap happens under the same lock
//! at the next take), so no GPU fences are needed. All writes into the back
//! slot happen with the lock held, which also makes several sinks on one
//! channel safe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use gfx::GfxContext;
use gfx::blit::{BlitOptions, Blitter};
use gfx::format::TexelFormat;
use gfx::surface::{SurfaceId, SurfaceKind, SurfaceStore};

use crate::mailbox::TripleBuffer;
use crate::scale::Viewport;

/// Format of the handoff slots. Non-sRGB: the Xbox pipeline is gamma-encoded
/// throughout, so no conversion happens on the way to the window.
pub const SLOT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Errors submitting a frame.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame size {0}x{1} is empty or too large")]
    Size(u32, u32),
    #[error("pixel buffer is {got} bytes, expected {expected}")]
    Pixels { got: usize, expected: usize },
    #[error("surface: {0}")]
    Surface(String),
}

/// Counters for both sides of the handoff.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Frames the guest presented.
    pub published: u64,
    /// Frames the window picked up.
    pub shown: u64,
    /// Frames replaced before the window picked them up (the guest
    /// outran the display).
    pub dropped: u64,
}

/// A frame the window can draw.
#[derive(Clone, Debug)]
pub struct ShownFrame {
    /// The slot's view (valid until the next [`FrameSource::latest`]).
    pub view: wgpu::TextureView,
    /// The slot's texture (for readback in tests).
    pub texture: wgpu::Texture,
    /// Size in pixels.
    pub size: (u32, u32),
    /// Publish sequence number, from 1.
    pub seq: u64,
    /// True if this frame is newer than the one returned last time.
    pub new: bool,
}

struct Slot {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    seq: u64,
}

struct State {
    tb: TripleBuffer,
    slots: [Option<Slot>; 3],
    next_seq: u64,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

struct Shared {
    state: Mutex<State>,
    wake_pending: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stats(&self) -> FrameStats {
        let s = self.lock();
        FrameStats {
            published: s.tb.published(),
            shown: s.tb.taken(),
            dropped: s.tb.dropped(),
        }
    }

    /// Wake the window once per batch of frames, not once per frame.
    fn wake(&self) {
        if !self.wake_pending.swap(true, Ordering::AcqRel) {
            let w = self.waker.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(w) = w {
                w();
            }
        }
    }
}

/// Creates a connected sink (guest side) and source (window side) on `ctx`'s
/// device.
pub fn channel(ctx: &GfxContext) -> (FrameSink, FrameSource) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            tb: TripleBuffer::new(),
            slots: [None, None, None],
            next_seq: 1,
        }),
        wake_pending: AtomicBool::new(false),
        waker: Mutex::new(None),
    });
    (
        FrameSink::new(Arc::clone(&shared), crate::share_context(ctx)),
        FrameSource { shared },
    )
}

/// The guest side of the handoff. `Send`; one per producing thread (create
/// more with [`FrameSink::another`]).
pub struct FrameSink {
    shared: Arc<Shared>,
    ctx: GfxContext,
    blitter: Option<Blitter>,
    hook_errors: u64,
}

impl FrameSink {
    fn new(shared: Arc<Shared>, ctx: GfxContext) -> Self {
        Self {
            shared,
            ctx,
            blitter: None,
            hook_errors: 0,
        }
    }

    /// Another sink publishing into the same channel.
    pub fn another(&self) -> FrameSink {
        FrameSink::new(Arc::clone(&self.shared), crate::share_context(&self.ctx))
    }

    /// The device frames are copied on.
    pub fn context(&self) -> &GfxContext {
        &self.ctx
    }

    /// Handoff counters.
    pub fn stats(&self) -> FrameStats {
        self.shared.stats()
    }

    /// Publishes a CPU frame: `size` pixels of tightly packed RGBA8
    /// (`[r, g, b, a]` per pixel, top row first).
    pub fn submit_rgba(&mut self, size: (u32, u32), pixels: &[u8]) -> Result<u64, FrameError> {
        check_size(size)?;
        let expected = size.0 as usize * size.1 as usize * 4;
        if pixels.len() != expected {
            return Err(FrameError::Pixels {
                got: pixels.len(),
                expected,
            });
        }
        let ctx = &self.ctx;
        publish_with(&self.shared, ctx, size, |slot| {
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &slot.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.0 * 4),
                    rows_per_image: Some(size.1),
                },
                wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
            );
            Ok(())
        })
    }

    /// Publishes any sampleable 2D colour view of `size` pixels, scaled to
    /// fill the slot. `opaque` forces alpha to 1 (X8 formats).
    pub fn submit_view(
        &mut self,
        view: &wgpu::TextureView,
        size: (u32, u32),
        opaque: bool,
    ) -> Result<u64, FrameError> {
        self.submit_encoded(size, opaque, view, None, [0., 0., 1., 1.], |_| Ok(()))
    }

    /// Publishes a gfx colour surface: the D3D back buffer on `Swap`.
    /// Resolves MSAA first, like `gfx::surface::present_surface`.
    pub fn submit_surface(
        &mut self,
        store: &SurfaceStore,
        id: SurfaceId,
    ) -> Result<u64, FrameError> {
        self.submit_surface_region(store, id, None, None)
    }

    fn submit_surface_region(
        &mut self,
        store: &SurfaceStore,
        id: SurfaceId,
        gamma: Option<&[u8; 768]>,
        crop: Option<[u32; 4]>,
    ) -> Result<u64, FrameError> {
        let s = store
            .get(id)
            .map_err(|e| FrameError::Surface(e.to_string()))?;
        if s.kind() != SurfaceKind::Color {
            return Err(FrameError::Surface(format!(
                "{id:?} is not a colour surface"
            )));
        }
        let size = (s.desc().width, s.desc().height);
        let opaque = !matches!(
            s.desc().format.texel(),
            TexelFormat::A8R8G8B8 | TexelFormat::A1R5G5B5 | TexelFormat::A4R4G4B4
        );
        let view = s.view().clone();
        let (output_size, rect) = match crop {
            Some([x, y, w, h]) if w != 0 && h != 0 && x.checked_add(w).is_some_and(|n| n <= size.0)
                && y.checked_add(h).is_some_and(|n| n <= size.1) => {
                ((w, h), [x as f32 / size.0 as f32, y as f32 / size.1 as f32,
                    w as f32 / size.0 as f32, h as f32 / size.1 as f32])
            }
            _ => (size, [0., 0., 1., 1.]),
        };
        self.submit_encoded(output_size, opaque, &view, gamma, rect, |enc| {
            store
                .resolve_msaa(enc, id)
                .map_err(|e| FrameError::Surface(e.to_string()))
        })
    }

    /// Turns this sink into the closure `d3d::GfxBackend::on_present` takes:
    /// `FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32, Option<&[u8; 768]>) + Send + 'static`.
    /// The `GfxBackend` must be built on [`crate::GuestHost::gfx_context`] (the
    /// shell's device). Errors are printed (first ten) and the frame is
    /// skipped; presentation never stops the guest.
    pub fn into_present_hook(
        self,
    ) -> impl FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32, Option<&[u8; 768]>) + Send + 'static
    {
        self.into_cropped_present_hook(1, || None)
    }

    /// Present a player's completed split-screen image; guest rendering and
    /// the guest's own back buffer remain under the engine's control.
    pub fn into_cropped_present_hook(
        mut self,
        every: u32,
        mut crop: impl FnMut() -> Option<[u32; 4]> + Send + 'static,
    ) -> impl FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32, Option<&[u8; 768]>) + Send + 'static
    {
        let mut frames = 0u64;
        move |ctx, store, id, _flags, gamma| {
            let rect = crop();
            let frame = frames;
            frames = frames.wrapping_add(1);
            if every == 0 || !frame.is_multiple_of(u64::from(every)) {
                return;
            }
            debug_assert!(
                ctx.device == self.ctx.device,
                "GfxBackend is not on the shell's device"
            );
            if let Err(e) = self.submit_surface_region(store, id, gamma, rect) {
                self.hook_errors += 1;
                if self.hook_errors <= 10 {
                    eprintln!("app: present hook: {e}");
                }
            }
        }
    }

    fn submit_encoded(
        &mut self,
        size: (u32, u32),
        opaque: bool,
        src: &wgpu::TextureView,
        gamma: Option<&[u8; 768]>,
        src_rect: [f32; 4],
        before: impl FnOnce(&mut wgpu::CommandEncoder) -> Result<(), FrameError>,
    ) -> Result<u64, FrameError> {
        check_size(size)?;
        let ctx = &self.ctx;
        let blitter = self.blitter.get_or_insert_with(|| Blitter::new(ctx));
        let shared = &self.shared;
        publish_with(shared, ctx, size, |slot| {
            let mut enc = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("app frame copy"),
                });
            before(&mut enc)?;
            blitter.blit(
                ctx,
                &mut enc,
                src,
                &slot.view,
                SLOT_FORMAT,
                BlitOptions {
                    linear: true,
                    force_opaque: opaque,
                    gamma,
                    src_rect,
                },
            );
            ctx.queue.submit([enc.finish()]);
            Ok(())
        })
    }
}

fn publish_with(
    shared: &Shared,
    ctx: &GfxContext,
    size: (u32, u32),
    write: impl FnOnce(&Slot) -> Result<(), FrameError>,
) -> Result<u64, FrameError> {
    let seq = {
        let mut st = shared.lock();
        let back = st.tb.back();
        let slot = match &mut st.slots[back] {
            Some(s) if s.size == size => s,
            entry => entry.insert(make_slot(ctx, size)),
        };
        write(slot)?;
        let seq = st.next_seq;
        st.next_seq += 1;
        st.slots[back].as_mut().expect("just written").seq = seq;
        st.tb.publish();
        seq
    };
    shared.wake();
    Ok(seq)
}

fn check_size(size: (u32, u32)) -> Result<(), FrameError> {
    if size.0 == 0 || size.1 == 0 || size.0 > 8192 || size.1 > 8192 {
        return Err(FrameError::Size(size.0, size.1));
    }
    Ok(())
}

fn make_slot(ctx: &GfxContext, size: (u32, u32)) -> Slot {
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("app frame slot"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SLOT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Slot {
        texture,
        view,
        size,
        seq: 0,
    }
}

/// The window side of the handoff.
pub struct FrameSource {
    shared: Arc<Shared>,
}

impl FrameSource {
    /// Called (from the guest thread) when a frame arrives while no wake is
    /// outstanding. Call [`ack_wake`](Self::ack_wake) before drawing so the
    /// next frame wakes again.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        *self.shared.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(waker));
    }

    /// Re-arms the waker.
    pub fn ack_wake(&self) {
        self.shared.wake_pending.store(false, Ordering::Release);
    }

    /// The newest frame (taking it if one was published since the last
    /// call), or `None` before the first.
    pub fn latest(&self) -> Option<ShownFrame> {
        let mut st = self.shared.lock();
        let new = st.tb.take().is_some();
        let front = st.tb.front()?;
        let slot = st.slots[front].as_ref()?;
        Some(ShownFrame {
            view: slot.view.clone(),
            texture: slot.texture.clone(),
            size: slot.size,
            seq: slot.seq,
            new,
        })
    }

    /// Handoff counters.
    pub fn stats(&self) -> FrameStats {
        self.shared.stats()
    }
}

/// Texture filtering when scaling to the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Filter {
    /// Bilinear: smooth at non-integer scales.
    #[default]
    Linear,
    /// Nearest: crisp pixels, best with [`crate::Scaling::Integer`].
    Nearest,
}

/// Draws a frame into a viewport of a target view, clearing the rest.
pub struct Compositor {
    module: wgpu::ShaderModule,
    bgl: wgpu::BindGroupLayout,
    layout: wgpu::PipelineLayout,
    pipelines: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    linear: wgpu::Sampler,
    nearest: wgpu::Sampler,
}

const COMPOSITE_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var out: VsOut;
    out.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(src, smp, in.uv).rgb, 1.0);
}
"#;

impl Compositor {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("app composite"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("app composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("app composite"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("app composite"),
                mag_filter: filter,
                min_filter: filter,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            })
        };
        Self {
            module,
            bgl,
            layout,
            pipelines: Vec::new(),
            linear: sampler(wgpu::FilterMode::Linear),
            nearest: sampler(wgpu::FilterMode::Nearest),
        }
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        if let Some(i) = self.pipelines.iter().position(|(f, _)| *f == format) {
            return &self.pipelines[i].1;
        }
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("app composite"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &self.module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.push((format, p));
        &self.pipelines.last().expect("pushed").1
    }

    /// Records a pass that clears `dst` to `clear` and, if `src` is given,
    /// draws it into `viewport`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: Option<&wgpu::TextureView>,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        viewport: Viewport,
        filter: Filter,
        clear: wgpu::Color,
    ) {
        let bind = src.filter(|_| !viewport.is_empty()).map(|src| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("app composite"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(match filter {
                            Filter::Linear => &self.linear,
                            Filter::Nearest => &self.nearest,
                        }),
                    },
                ],
            })
        });
        let pipeline = bind
            .as_ref()
            .map(|_| self.pipeline(device, dst_format).clone());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("app composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if let (Some(bind), Some(pipeline)) = (bind, pipeline) {
            pass.set_viewport(
                viewport.x as f32,
                viewport.y as f32,
                viewport.width as f32,
                viewport.height as f32,
                0.0,
                1.0,
            );
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
