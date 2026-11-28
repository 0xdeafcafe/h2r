//! Texture bind groups cached by the views they bind, kept while draws
//! still use them.
//!
//! A bind group holds its views and a view holds its texture, so a cached
//! group keeps its textures alive. Most views a native draw binds are the
//! resident textures of the map, the same every frame. Others are new each
//! frame or each draw: a copy of the frame buffer
//! (a scene copy) or a depth alias
//! (a depth alias) is a new texture at the target's
//! size every time, about 20 MB at 2560x1920 (4x render scale). A cache
//! that only starts over at a group count holds that many of them.
//!
//! [`TextureGroups`] drops a group once no draw has used it for
//! [`KEEP_FRAMES`] frames, as `gfx::program::ProgramBindings` does for the
//! D3D path, so a texture a frame made is released with that frame. A
//! steady scene hits its groups every frame and makes none again.

use std::collections::HashMap;
use std::hash::Hash;

/// Frames a group survives unused. The reference and display copies of
/// one frame, and the frames the render thread still has queued, all hit
/// the same groups inside this window.
pub const KEEP_FRAMES: u64 = 4;

struct Entry {
    group: wgpu::BindGroup,
    textures: Vec<wgpu::Texture>,
    used: u64,
}

/// What a cache holds now, for tests and the memory probe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub groups: usize,
    /// The textures its groups keep alive, each counted once.
    pub textures: usize,
    pub texture_bytes: u64,
}

/// Groups by key, aged by the frame of the draws that use them.
pub struct TextureGroups<K> {
    groups: HashMap<K, Entry>,
    /// The textures the groups bind: how many groups bind each, and its bytes.
    textures: HashMap<wgpu::Texture, (usize, u64)>,
    frame: u64,
    texture_bytes: u64,
    probe: mem_watch::Gauge,
}

impl<K: Eq + Hash> TextureGroups<K> {
    /// `probe` reports the bytes of the textures the groups keep alive.
    pub fn new(probe: &'static mem_watch::Counter) -> Self {
        Self {
            groups: HashMap::new(),
            textures: HashMap::new(),
            frame: 0,
            texture_bytes: 0,
            probe: probe.track(0),
        }
    }

    /// The frame of the draws that follow. A new frame drops the groups no
    /// draw used in the last [`KEEP_FRAMES`] frames, and with them the
    /// textures only they still held.
    pub fn begin_frame(&mut self, frame: u64) {
        if frame == self.frame {
            return;
        }
        self.frame = frame;
        let mut retired = Vec::new();
        self.groups.retain(|_, e| {
            let keep = frame.saturating_sub(e.used) <= KEEP_FRAMES;
            if !keep {
                retired.append(&mut e.textures);
            }
            keep
        });
        for t in retired {
            self.release(&t);
        }
        self.probe.set(self.texture_bytes);
    }

    /// The cached group for `key`, marked used this frame.
    pub fn get(&mut self, key: &K) -> Option<wgpu::BindGroup> {
        let entry = self.groups.get_mut(key)?;
        entry.used = self.frame;
        Some(entry.group.clone())
    }

    /// Keeps `group` for later draws of `key`. `textures` are the textures
    /// of the views it binds (`wgpu::TextureView::texture`).
    pub fn insert(
        &mut self,
        key: K,
        group: wgpu::BindGroup,
        textures: impl IntoIterator<Item = wgpu::Texture>,
    ) {
        let textures: Vec<wgpu::Texture> = textures.into_iter().collect();
        for t in &textures {
            let (refs, bytes) = self
                .textures
                .entry(t.clone())
                .or_insert_with(|| (0, texture_bytes(t)));
            if *refs == 0 {
                self.texture_bytes += *bytes;
            }
            *refs += 1;
        }
        if let Some(old) = self.groups.insert(
            key,
            Entry {
                group,
                textures,
                used: self.frame,
            },
        ) {
            for t in &old.textures {
                self.release(t);
            }
        }
        self.probe.set(self.texture_bytes);
    }

    pub fn held(&self) -> Held {
        Held {
            groups: self.groups.len(),
            textures: self.textures.len(),
            texture_bytes: self.texture_bytes,
        }
    }

    fn release(&mut self, t: &wgpu::Texture) {
        let Some((refs, bytes)) = self.textures.get_mut(t) else {
            return;
        };
        *refs -= 1;
        if *refs == 0 {
            self.texture_bytes -= *bytes;
            self.textures.remove(t);
        }
    }
}

/// The bytes of every level and layer of `t`, by its format's block size.
pub fn texture_bytes(t: &wgpu::Texture) -> u64 {
    let format = t.format();
    let (bw, bh) = format.block_dimensions();
    let block = u64::from(format.block_copy_size(None).unwrap_or(8));
    let layers = u64::from(t.depth_or_array_layers());
    let volume = t.dimension() == wgpu::TextureDimension::D3;
    (0..t.mip_level_count())
        .map(|level| {
            let w = u64::from((t.width() >> level).max(1).div_ceil(bw));
            let h = u64::from((t.height() >> level).max(1).div_ceil(bh));
            let d = if volume {
                (layers >> level).max(1)
            } else {
                layers
            };
            w * h * d * block
        })
        .sum()
}
