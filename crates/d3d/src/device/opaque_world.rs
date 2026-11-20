//! A sorted geometry entry owns these immutable index windows before its
//! handler changes any state. The shader's declaration and streams are
//! selected by that handler. Render targets, sampled render surfaces and
//! writebacks retain D3D submission order, including between subparts.

use super::*;

/// An empty draw still drains its preceding push commands.
pub struct PreparedIndexRange {
    address: u32,
    generation: u64,
    cache_epoch: Option<u64>,
    values: Option<(u16, u16, IndexData)>,
}

impl PreparedIndexRange {
    pub fn window(&self) -> Option<(u16, u16)> {
        self.values.as_ref().map(|(lo, hi, _)| (*lo, *hi))
    }

    pub fn count(&self) -> u32 {
        self.values.as_ref().map_or(0, |(_, _, indices)| indices.len() as u32)
    }
}

/// Cache key of a bitmap texture uploaded during section prepare. Owning
/// this value does not acquire an Xbox resource reference.
#[derive(Clone, Copy)]
pub struct PreparedStageTexture(TextureKey);

/// Attachments selected by the view. Their backing header identities can
/// change across auxiliary render targets, even when an address is reused.
#[derive(Clone, Copy)]
pub struct PreparedOpaqueTargets {
    pub bound: [u32; 8],
    identities: [[u32; 3]; 2],
    pub(super) info: TargetInfo,
}

#[derive(Clone, Copy)]
pub(super) struct PacketInputs<'a> {
    pub targets: &'a PreparedOpaqueTargets,
    pub textures: &'a [Option<PreparedStageTexture>; 4],
    pub material: u32,
}

impl<B: Backend> Device<B> {
    pub fn prepare_opaque_targets(
        &mut self, mem: &dyn GuestMemory, accumulation: Option<(u32, u32, (u32, u32))>,
    ) -> Option<PreparedOpaqueTargets> {
        if !self.can_external_pass() || self.render_target == 0 || self.depth_stencil == 0
            || self.guest_z_scale(mem).is_none() { return None; }
        if let Some((surface, texture, size)) = accumulation {
            let image = PixelContainer::read(mem, texture).ok()?;
            if image.resource.kind() != ResourceType::Texture { return None; }
            let desc = crate::texdesc::from_header(image.format, image.size).ok()?;
            let data = self.cfg.address_map.to_virtual(image.resource.data);
            let (header, level) = self.direct_color_level(data, &desc, 0)?;
            if header != surface || (level.width, level.height) != size { return None; }
        }
        let bound = self.bound_target();
        let color = PixelContainer::read(mem, bound[0]).ok()?;
        let depth = PixelContainer::read(mem, bound[1]).ok()?;
        let identities = [color.identity(), depth.identity()];
        if let Some(cached) = self.opaque_targets
            && (cached.bound, cached.identities) == (bound, identities)
        {
            return Some(cached);
        }
        let c = crate::texdesc::from_header(color.format, color.size).ok()?;
        let d = crate::texdesc::from_header(depth.format, depth.size).ok()?;
        let info = TargetInfo {
            width: c.width, height: c.height, format: c.format, depth_format: Some(d.format),
        };
        let targets = PreparedOpaqueTargets { bound, identities, info };
        self.opaque_targets = Some(targets);
        Some(targets)
    }

    pub(super) fn matches_opaque_targets(&self, mem: &dyn GuestMemory, prepared: &PreparedOpaqueTargets) -> bool {
        self.bound_target() == prepared.bound && prepared.identities.iter().enumerate().all(|(i, identity)| {
            PixelContainer::read(mem, prepared.bound[i]).is_ok_and(|p| p.identity() == *identity)
        })
    }

    pub fn prepare_index_range(
        &mut self, mem: &dyn GuestMemory, addr: u32, count: u32,
    ) -> Result<PreparedIndexRange, D3dError> {
        const E: &str = "native opaque indices";
        vertex_limit(E, count)?;
        let len = span(E, count, 2)?;
        crate::guest::check_range(addr, len)?;
        let values = if len == 0 { None } else { Some(self.index_list(mem, addr, len)?) };
        Ok(PreparedIndexRange {
            address: addr, generation: self.tracker.generation(),
            cache_epoch: self.index_cache.as_ref().map(IndexCache::epoch), values,
        })
    }

    /// Warm the ordinary bitmap cache for a resident source. Engine
    /// render textures are resolved as surfaces at their packet boundary.
    pub fn prepare_stage_texture(
        &mut self, mem: &dyn GuestMemory, header: u32, palette: u32, sign: u32,
    ) -> Option<PreparedStageTexture> {
        if self.cfg.upload != UploadPolicy::Tracked { return None; }
        // A warm key is only a hint. Header and palette identity and both
        // data generations are rechecked after the shader binder runs.
        for p in [palette, 0] {
            for s in [sign & COLOR_SIGN_MASK, 0] {
                let key = (header, p, s);
                if self.cache.get(&key).is_some_and(|c| {
                    texture_color_sign(c.info.format, sign) == s
                        && (c.info.format != D3DFormat::P8 || palette == p)
                }) {
                    return Some(PreparedStageTexture(key));
                }
            }
        }
        let (key, _) = self.resolve_texture(mem, "native opaque bitmap", header, palette, sign).ok()?;
        Some(PreparedStageTexture(key))
    }

    pub fn bound_palette_header(&self, stage: usize) -> Option<u32> {
        self.palettes.get(stage).copied()
    }

    pub(super) fn prepared_texture(
        &mut self, mem: &dyn GuestMemory, hint: PreparedStageTexture, header: u32, palette: u32,
        sign: u32,
    ) -> Option<(TextureKey, TextureInfo)> {
        let key = hint.0;
        if key.0 != header || self.cfg.upload != UploadPolicy::Tracked { return None; }
        let c = self.cache.get(&key)?;
        let pc = PixelContainer::read(mem, header).ok()?;
        if pc.resource.kind() != ResourceType::Texture || pc.identity() != c.identity
            || texture_color_sign(c.info.format, sign) != key.2
            || self.tracker.dirty_since(c.va, c.len, c.generation)
        {
            return None;
        }
        if let Some(p) = c.palette {
            if p.header != palette { return None; }
            let live = PaletteHeader::read(mem, palette).ok()?;
            if live.resource.common != p.common
                || self.cfg.address_map.to_virtual(live.resource.data) != p.va
                || self.tracker.dirty_since(p.va, p.len, c.generation)
            {
                return None;
            }
        }
        self.stats.texture_hits += 1;
        Some((key, c.info))
    }

    /// Bitmap residency callbacks and material setters ran since section
    /// preparation. Recheck this original index span at its ordered packet
    /// boundary. Untracked heap indices are compared by value.
    fn refresh_opaque_indices(
        &mut self, mem: &dyn GuestMemory, range: &mut PreparedIndexRange,
    ) -> Result<(), D3dError> {
        let Some((lo, _, indices)) = &range.values else { return Ok(()) };
        let len = indices.len() * 2;
        let addr = range.address;
        let tracked = self.cfg.upload == UploadPolicy::Tracked && indices.cache_id().is_some()
            && self.index_cache.as_ref().is_some_and(|c| Some(c.epoch()) == range.cache_epoch);
        if tracked && !self.tracker.dirty_since(addr, len as u32, range.generation) {
            return Ok(());
        }
        if !tracked {
            let raw = mem.read_vec(addr, len)?;
            if raw.as_chunks::<2>().0.iter().zip(&indices[..])
                .all(|(&b, &i)| u32::from(u16::from_le_bytes(b)) == u32::from(*lo) + i)
            {
                return Ok(());
            }
        }
        range.values = Some(self.index_list(mem, addr, len)?);
        Ok(())
    }

    /// A callback is left queued for the ordinary D3D ABI before the
    /// first draw. Each packet then resolves render-surface dependencies
    /// separately, in submission order, with its own exact backend fallback.
    /// Ok(false) always means zero packets were submitted.
    pub fn emit_world_ranges<G>(
        &mut self, mem: &mut dyn GuestMemory, primitive: PrimitiveType,
        mut ranges: Vec<PreparedIndexRange>, vertices: PreparedWorldVertices,
        code: Option<&Microcode>, targets: &PreparedOpaqueTargets, textures: &[Option<PreparedStageTexture>; 4],
        material: u32, mut enter_range: impl FnMut(u32) -> G,
    ) -> Result<bool, D3dError> {
        const E: &str = "D3DDevice_DrawIndexedVertices";
        self.live(E)?;
        if self.swallowing.is_some() || self.scoped_admission.is_some()
            || !self.drain_admission_boundary(mem)? || self.immediate.is_open() || self.hw.zpass_count_enable
        {
            return Ok(false);
        }
        if !self.matches_opaque_targets(mem, targets) { return Ok(false); }
        for (i, range) in ranges.iter_mut().enumerate() {
            if range.values.is_none() { continue; }
            // A previous range can force a texture-level writeback. Check
            // these exact indices and stream windows after that writeback.
            self.refresh_opaque_indices(mem, range)?;
            let (layout, vertex, mut streams) = self.world_vertex_part(mem, &vertices, i, range, code)?;
            let (lo, hi, indices) = range.values.take().expect("nonempty range");
            let _entry = enter_range(indices.len() as u32);
            let parts = DrawParts {
                entry: E, indexed_esp: None, primitive,
                vertex_count: u32::from(hi) - u32::from(lo) + 1,
                layout, vertex, streams: Vec::new(), indices: Some(indices), words: self.draw_state(mem)?,
            };
            let result = self.submit_inner(mem, parts, &mut streams, None, Some(PacketInputs { targets, textures, material }));
            self.recycle_streams(streams);
            result?;
        }
        Ok(true)
    }
}
