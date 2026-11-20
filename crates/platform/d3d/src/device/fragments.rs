//! D3D's internal viewport/transform fragments, reproduced over guest
//! memory: what they read, the device fields they write and the push-buffer
//! words they emit are the original's (verified against the XBE with
//! `h2 disasm`; the addresses in each doc comment are the evidence).
//!
//! Game code calls four of them directly from Halo's inlined `ZEnable`
//! (18 sites such as `0x1eb94`), so overriding the public setters is not
//! enough. The device also runs them where the original D3D does:
//! CreateDevice (`0x3fbbe4`), `SetViewport` (`0x3f82ed`, `0x3f8308`),
//! `SetTransform` and the `ZEnable` setter (`0x3f555b`..`0x3f5583`).
//!
//! Floating point: the originals compute on the x87 stack and round when
//! they store. Here intermediates are `f64` (exact for the operand ranges
//! involved: integers below 2^24 times `f32`s) and stores round to `f32`
//! at the same points.

use super::*;
use crate::hw::method;

/// `D3D__RenderState` slots the fragments read.
mod rs {
    /// `D3DRS_ZENABLE`; 2 is `D3DZB_USEW`.
    pub const ZENABLE: u32 = 143;
    /// `D3DRS_MULTISAMPLEANTIALIAS`.
    pub const MULTISAMPLE_ANTIALIAS: u32 = 152;
    /// `D3DRS_YUVENABLE`.
    pub const YUV_ENABLE: u32 = 160;
    /// Slot 93 (`D3DRS_FOGTABLEMODE` by the Cxbx numbering), which picks the
    /// pass-through program in `0x3f6f40`.
    pub const FOG_TABLE_MODE: u32 = 93;
}

/// Vertex-shader object flag bits (`[object + 4]`).
mod vs_flags {
    /// Screen-space pass-through (`D3DFVF_XYZRHW` and friends).
    pub const PASSTHROUGH: u32 = 0x02;
    /// Either pass-through or a user program (`0x3f619c`).
    pub const PROGRAMMABLE_OR_PASSTHROUGH: u32 = 0x12;
}

/// Device flag bits (`+0x8`) the fragments read.
mod dev_flags {
    /// The projection is affine (`0x3f80e8`).
    pub const AFFINE_PROJECTION: u32 = 0x02;
    /// Viewport registers are not pushed for programs (`0x3f61b6`).
    pub const NO_PROGRAM_VIEWPORT: u32 = 0x200;
    /// Super-sampled target (`0x3f66fa`).
    pub const SUPERSAMPLE: u32 = 0x8000;
}

/// 4294967296: the x87 `fild` of a negative dword is corrected to unsigned
/// by adding this (`0x45DBB8`).
fn unsigned(v: u32) -> f64 {
    v as f64
}

impl<B: Backend> Device<B> {
    fn fu(mem: &dyn GuestMemory, addr: u32) -> Result<f64, D3dError> {
        Ok(f32::from_bits(mem.read_u32(addr)?) as f64)
    }

    fn rs(&self, mem: &dyn GuestMemory, slot: u32) -> Result<u32, D3dError> {
        Ok(mem.read_u32(self.cfg.layout.render_state_slot(slot))?)
    }

    /// Subtracted from the screen offsets when super-sampling with
    /// multisample antialiasing on (`0x3f60ad..0x3f60cd`).
    fn supersample_bias(&self, mem: &dyn GuestMemory, dev: u32) -> Result<f64, D3dError> {
        let flags = mem.read_u32(dev + field::FLAGS)?;
        Ok(
            if flags & dev_flags::SUPERSAMPLE != 0 && self.rs(mem, rs::MULTISAMPLE_ANTIALIAS)? != 0
            {
                0.5
            } else {
                0.0
            },
        )
    }

    /// `0x3f6060`: the viewport offset and scale registers for programmable
    /// and pass-through shaders, from the viewport at `+0xEE0`.
    ///
    /// `offset = [x*ssx + ox + w*ssx/2, y*ssy + oy + h*ssy/2, minZ*zs, 0]`,
    /// `scale = [w*ssx/2, -h*ssy/2, (maxZ - minZ)*zs, 0]`, less the
    /// super-sample bias on the offsets.
    pub(crate) fn viewport_registers(
        &self,
        mem: &dyn GuestMemory,
    ) -> Result<([u32; 4], [u32; 4]), D3dError> {
        let dev = mem.read_u32(self.cfg.layout.p_device)?;
        let vp = crate::guest::read_array::<6>(mem, dev + field::VIEWPORT)?;
        let ssx = Self::fu(mem, dev + field::SUPERSAMPLE_X)?;
        let ssy = Self::fu(mem, dev + field::SUPERSAMPLE_Y)?;
        let zs = Self::fu(mem, dev + field::Z_SCALE)?;
        let bias = self.supersample_bias(mem, dev)?;
        let a = unsigned(vp[0]) * ssx + Self::fu(mem, dev + field::SCREEN_OFFSET_X)? - bias;
        let b = unsigned(vp[1]) * ssy + Self::fu(mem, dev + field::SCREEN_OFFSET_Y)? - bias;
        let half_w = (unsigned(vp[2]) * ssx * 0.5) as f32;
        let neg_half_h = (unsigned(vp[3]) * ssy * -0.5) as f32;
        let (min_z, max_z) = (f32::from_bits(vp[4]) as f64, f32::from_bits(vp[5]) as f64);
        let range = ((max_z - min_z) * zs) as f32;
        let z_min = (min_z * zs) as f32;
        let offset = [
            ((half_w as f64 + a) as f32).to_bits(),
            ((-(neg_half_h as f64) + b) as f32).to_bits(),
            z_min.to_bits(),
            0,
        ];
        let scale = [half_w.to_bits(), neg_half_h.to_bits(), range.to_bits(), 0];
        Ok((offset, scale))
    }

    /// Checks that `n` words fit at `put` inside the push buffer.
    fn push_room(&self, entry: &'static str, put: u32, words: u32) -> Result<(), D3dError> {
        let pb = self.cfg.push_buffer;
        let end = put as u64 + 4 * words as u64;
        if put < pb.addr || end > pb.addr as u64 + pb.len as u64 || !put.is_multiple_of(4) {
            return Err(D3dError::invalid(
                entry,
                format!(
                    "put {put:#010x} (+{} words) is outside the push buffer [{:#010x}, {:#010x})",
                    words,
                    pb.addr,
                    pb.addr as u64 + pb.len as u64
                ),
            ));
        }
        Ok(())
    }

    pub(crate) fn write_words(
        mem: &mut dyn GuestMemory,
        at: u32,
        words: &[u32],
    ) -> Result<(), D3dError> {
        for (i, w) in words.iter().enumerate() {
            mem.write_u32(at + 4 * i as u32, *w)?;
        }
        Ok(())
    }

    fn check_device_arg(&self, entry: &'static str, dev: u32) -> Result<(), D3dError> {
        if dev != self.cfg.layout.device {
            return Err(D3dError::invalid(
                entry,
                format!(
                    "device argument {dev:#010x} is not the device object {:#010x}",
                    self.cfg.layout.device
                ),
            ));
        }
        Ok(())
    }

    /// `0x3f6190` (`EAX` = put, `ESI` = device; returns the new put in
    /// `EAX`): pushes the viewport registers and the depth clip range for
    /// the current vertex-shader kind, and refreshes the clip shadow at
    /// `+0x1C08..+0x1C14` unless game code owns it (`+0x1C18`).
    ///
    /// * Program or pass-through (`[vs+4] & 0x12`): unless device flag
    ///   `0x200`, `VIEWPORT_OFFSET` and `VIEWPORT_SCALE` from
    ///   `viewport_registers` (`0x3f6060`); then the clip
    ///   range at `+0x1C10` (w-buffered: `[944*93C*948, 948]`, else
    ///   `[0, 948]`, unless `+0x1C18` bit 1).
    /// * Fixed function: `VIEWPORT_OFFSET = [x*ssx + ox, y*ssy + oy, 0, 0]`,
    ///   then the clip range at `+0x1C08` (w-buffered as above, else
    ///   `[minZ*948, maxZ*948]`, unless `+0x1C18` bit 0).
    pub fn push_viewport(
        &mut self,
        mem: &mut dyn GuestMemory,
        put: u32,
        device: u32,
    ) -> Result<u32, D3dError> {
        const E: &str = "D3D viewport push fragment";
        self.live(E)?;
        self.check_device_arg(E, device)?;
        self.push_room(E, put, 13)?;
        let dev = device;
        let vs = mem.read_u32(dev + field::VERTEX_SHADER)?;
        let vs_flags = mem.read_u32(at(vs, 4)?)?;
        let flags = mem.read_u32(dev + field::FLAGS)?;
        let zs_bits = mem.read_u32(dev + field::Z_SCALE)?;
        let usew = self.rs(mem, rs::ZENABLE)? == 2;
        let w_clip = ((Self::fu(mem, dev + field::INV_W_FAR)?
            * Self::fu(mem, dev + field::W_NEAR)?
            * f32::from_bits(zs_bits) as f64) as f32)
            .to_bits();
        let clip_flags = mem.read_u32(dev + field::CLIP_FLAGS)? & 0xFF;
        let mut p = put;
        let (clip_at, owned) = if vs_flags & vs_flags::PROGRAMMABLE_OR_PASSTHROUGH != 0 {
            let (offset, scale) = self.viewport_registers(mem)?;
            if flags & dev_flags::NO_PROGRAM_VIEWPORT == 0 {
                let mut w = vec![0x0010_0000 | method::VIEWPORT_OFFSET];
                w.extend_from_slice(&offset);
                w.push(0x0010_0000 | method::VIEWPORT_SCALE);
                w.extend_from_slice(&scale);
                Self::write_words(mem, p, &w)?;
                p += 4 * w.len() as u32;
            }
            (field::CLIP_PROGRAM, clip_flags & 2 != 0)
        } else {
            let vp = crate::guest::read_array::<6>(mem, dev + field::VIEWPORT)?;
            let bias = self.supersample_bias(mem, dev)?;
            let a = unsigned(vp[0]) * Self::fu(mem, dev + field::SUPERSAMPLE_X)?
                + Self::fu(mem, dev + field::SCREEN_OFFSET_X)?
                - bias;
            let b = unsigned(vp[1]) * Self::fu(mem, dev + field::SUPERSAMPLE_Y)?
                + Self::fu(mem, dev + field::SCREEN_OFFSET_Y)?
                - bias;
            let w = [
                0x0010_0000 | method::VIEWPORT_OFFSET,
                (a as f32).to_bits(),
                (b as f32).to_bits(),
                0,
                0,
            ];
            Self::write_words(mem, p, &w)?;
            p += 20;
            (field::CLIP_FIXED, clip_flags & 1 != 0)
        };
        if !owned {
            let (lo, hi) = if usew {
                (w_clip, zs_bits)
            } else if clip_at == field::CLIP_PROGRAM {
                (0, zs_bits)
            } else {
                let zs = f32::from_bits(zs_bits) as f64;
                (
                    ((Self::fu(mem, dev + field::VIEWPORT + 16)? * zs) as f32).to_bits(),
                    ((Self::fu(mem, dev + field::VIEWPORT + 20)? * zs) as f32).to_bits(),
                )
            };
            mem.write_u32(dev + clip_at, lo)?;
            mem.write_u32(dev + clip_at + 4, hi)?;
        }
        let lo = mem.read_u32(dev + clip_at)?;
        let hi = mem.read_u32(dev + clip_at + 4)?;
        Self::write_words(mem, p, &[0x0008_0000 | method::CLIP_MIN, lo, hi])?;
        Ok(p + 12)
    }

    /// `0x3f6390` (`EDX` = put, `[esp+4]` = device; returns the new put in
    /// `EAX`, `ret 4`): pushes `SET_CONTROL0` = `0x100001`, plus
    /// `0x10000000` with YUV enabled (slot 160), `0x10000` when w-buffered
    /// and `0x1000` for a float depth format (`0x2B`, `0x2D`, `0x2F`,
    /// `0x31`).
    pub fn push_control0(
        &mut self,
        mem: &mut dyn GuestMemory,
        put: u32,
        device: u32,
    ) -> Result<u32, D3dError> {
        const E: &str = "D3D push fragment (0x3f6390)";
        self.live(E)?;
        self.check_device_arg(E, device)?;
        self.push_room(E, put, 2)?;
        let mut v = if self.rs(mem, rs::YUV_ENABLE)? != 0 {
            0x1010_0001
        } else {
            0x0010_0001
        };
        if self.rs(mem, rs::ZENABLE)? == 2 {
            v |= 0x1_0000;
        }
        let depth = mem.read_u32(device + field::DEPTH_STENCIL)?;
        if depth != 0 {
            let format = (mem.read_u32(at(depth, 12)?)? >> 8) & 0xFF;
            if matches!(format, 0x2B | 0x2D | 0x2F | 0x31) {
                v |= 0x1000;
            }
        }
        Self::write_words(mem, put, &[0x0004_0000 | method::CONTROL0, v])?;
        Ok(put + 8)
    }

    /// `D3D_UpdateProjectionViewportTransform` (`0x3f6840`, no arguments):
    /// builds the viewport matrix from `+0xEE0`, the super-sample scale and
    /// the depth scale (scaled by `+0x944 * +0x948` when w-buffered),
    /// stores `projection (+0xCA0) * viewport` at `+0x980` (`0x3fd080`),
    /// and sets dirty bit `0x200`.
    pub fn update_projection_viewport(
        &mut self,
        mem: &mut dyn GuestMemory,
    ) -> Result<(), D3dError> {
        const E: &str = "D3D_UpdateProjectionViewportTransform";
        self.live(E)?;
        let dev = mem.read_u32(self.cfg.layout.p_device)?;
        let vp = crate::guest::read_array::<6>(mem, dev + field::VIEWPORT)?;
        let ssx = Self::fu(mem, dev + field::SUPERSAMPLE_X)?;
        let ssy = Self::fu(mem, dev + field::SUPERSAMPLE_Y)?;
        let zs = Self::fu(mem, dev + field::Z_SCALE)?;
        let half_w_ext = unsigned(vp[2]) * ssx * 0.5;
        let half_w = half_w_ext as f32;
        let nh_ext = unsigned(vp[3]) * ssy * -0.5;
        let nh = nh_ext as f32;
        let hh = (-nh_ext) as f32;
        let (min_z, max_z) = (f32::from_bits(vp[4]) as f64, f32::from_bits(vp[5]) as f64);
        let mut range = ((max_z - min_z) * zs) as f32;
        let mut z_min = (zs * min_z) as f32;
        let (m11, m22, m44, m41, m42) = if self.rs(mem, rs::ZENABLE)? == 2 {
            let k = Self::fu(mem, dev + field::INV_W_FAR)? * zs;
            range = (k * range as f64) as f32;
            let m41 = (k * half_w as f64) as f32;
            let m42 = (k * hh as f64) as f32;
            z_min = (k * z_min as f64) as f32;
            (
                (half_w_ext * k) as f32,
                (nh as f64 * k) as f32,
                k as f32,
                m41,
                m42,
            )
        } else {
            (half_w, nh, 1.0, half_w, hh)
        };
        let mut v = [0f32; 16];
        v[0] = m11;
        v[5] = m22;
        v[10] = range;
        v[12] = m41;
        v[13] = m42;
        v[14] = z_min;
        v[15] = m44;
        let proj = read_matrix(mem, dev + field::PROJECTION)?;
        let out = mat_mul(&proj, &v);
        for (i, f) in out.iter().enumerate() {
            mem.write_u32(dev + field::COMPOSITE + 4 * i as u32, f.to_bits())?;
        }
        let dirty = self.cfg.layout.dirty_flags;
        let d = mem.read_u32(dirty)?;
        mem.write_u32(dirty, d | 0x200)?;
        Ok(())
    }

    /// `0x3f6f40` (`ESI` = device): for a pass-through vertex shader
    /// (`[vs+4] & 2`), reserves space like the original
    /// (`put + 4n + 0x78 >= threshold + 0x200` calls `MakeRequestedSpace`),
    /// then pushes constant load 0, constants 0..1 =
    /// `[ssx, ssy, zs, w]`, `[ox - b, oy - b, 0, 0]` (`w = 948*944` when
    /// w-buffered, else 1; `b` the super-sample bias), program load 0 and
    /// the pass-through program the image holds for the fog/w mode, and
    /// stores the new put. Returns it (as `EAX`), or `None` when the shader
    /// is not pass-through (the original then returns with `EAX` = the
    /// shader object, from its first instruction `mov eax,[esi+794h]`).
    pub fn load_passthrough(
        &mut self,
        mem: &mut dyn GuestMemory,
        device: u32,
    ) -> Result<Option<u32>, D3dError> {
        const E: &str = "D3D transform/viewport push (0x3f6f40)";
        self.live(E)?;
        self.check_device_arg(E, device)?;
        let dev = device;
        let vs = mem.read_u32(dev + field::VERTEX_SHADER)?;
        if mem.read_u32(at(vs, 4)?)? & vs_flags::PASSTHROUGH == 0 {
            return Ok(None);
        }
        let flags = mem.read_u32(dev + field::FLAGS)?;
        let progs = self.cfg.layout.passthrough_programs;
        let (table, n) = if self.rs(mem, rs::FOG_TABLE_MODE)? == 0 {
            progs[0]
        } else if flags & dev_flags::AFFINE_PROJECTION != 0 {
            progs[1]
        } else {
            progs[2]
        };
        let program = mem.read_words(table, n as usize)?;
        let mut put = mem.read_u32(dev + field::PUT)?;
        let threshold = mem.read_u32(dev + field::THRESHOLD)?;
        if put as u64 + 4 * n as u64 + 0x78 >= threshold as u64 + 0x200 {
            let kick = mem.read_u32(self.cfg.layout.kickoff_size)?;
            let need = 4 * n + 0x27C;
            put = self.make_requested_space(mem, need.max(kick / 2), need.max(kick))?;
        }
        let words = 13 + n + n.div_ceil(32);
        self.push_room(E, put, words)?;
        let w = if self.rs(mem, rs::ZENABLE)? == 2 {
            ((Self::fu(mem, dev + field::Z_SCALE)? * Self::fu(mem, dev + field::INV_W_FAR)?) as f32)
                .to_bits()
        } else {
            1.0f32.to_bits()
        };
        let bias = self.supersample_bias(mem, dev)?;
        let ox = ((Self::fu(mem, dev + field::SCREEN_OFFSET_X)? - bias) as f32).to_bits();
        let oy = ((Self::fu(mem, dev + field::SCREEN_OFFSET_Y)? - bias) as f32).to_bits();
        let mut out = vec![
            0x0004_0000 | method::CONSTANT_LOAD,
            0,
            0x0020_0000 | method::TRANSFORM_CONSTANT,
            mem.read_u32(dev + field::SUPERSAMPLE_X)?,
            mem.read_u32(dev + field::SUPERSAMPLE_Y)?,
            mem.read_u32(dev + field::Z_SCALE)?,
            w,
            ox,
            oy,
            0,
            0,
            0x0004_0000 | method::PROGRAM_LOAD,
            0,
        ];
        // `0x3f6b70`: at most 32 words per header.
        for chunk in program.chunks(32) {
            out.push(((chunk.len() as u32) << 18) | method::TRANSFORM_PROGRAM);
            out.extend_from_slice(chunk);
        }
        Self::write_words(mem, put, &out)?;
        let put = put + 4 * out.len() as u32;
        mem.write_u32(dev + field::PUT, put)?;
        Ok(Some(put))
    }

    /// The fragment sequence the original runs after a depth-mode change
    /// (`0x3f555b..0x3f5588`): update the composite, reload the
    /// pass-through program, then push `SET_CONTROL0` and the viewport.
    pub(crate) fn refresh_transform(
        &mut self,
        mem: &mut dyn GuestMemory,
        passthrough: bool,
        control0: bool,
    ) -> Result<(), D3dError> {
        let dev = self.cfg.layout.device;
        self.update_projection_viewport(mem)?;
        if passthrough {
            self.load_passthrough(mem, dev)?;
        }
        let put = self.reserve(mem)?;
        let put = if control0 {
            self.push_control0(mem, put, dev)?
        } else {
            put
        };
        let put = self.push_viewport(mem, put, dev)?;
        mem.write_u32(dev + field::PUT, put)?;
        Ok(())
    }

    /// The device-object state CreateDevice sets up that the fragments read
    /// (`0x3fc421..0x3fc462` and `0x3fba30`): identity transforms (with
    /// the W parameters they imply), the default FVF shader object at
    /// `+0x794` and the cleared static programmable object (flags `0x10`),
    /// the multisample type and back-buffer scale, the depth scale for the
    /// depth format, the target size, super-sample scale 1, the 0.53125
    /// screen offsets; then the composite, the pass-through program (only
    /// for a pass-through shader) and the viewport push.
    pub(crate) fn init_transform_state(
        &mut self,
        mem: &mut dyn GuestMemory,
    ) -> Result<(), D3dError> {
        let l = self.cfg.layout;
        let dev = l.device;
        let p = self.params.expect("created");
        for t in 0..10u32 {
            for i in 0..16u32 {
                let v = if i % 5 == 0 { 1.0f32 } else { 0.0 };
                mem.write_u32(dev + field::TRANSFORMS + 64 * t + 4 * i, v.to_bits())?;
            }
        }
        self.derive_w_params(mem)?;
        mem.write_u32(dev + field::VERTEX_SHADER, l.default_vertex_shader)?;
        for i in 0..0x46u32 {
            mem.write_u32(l.static_vertex_shader + 4 * i, 0)?;
        }
        mem.write_u32(l.static_vertex_shader + 4, 0x10)?;
        let ms = if p.multisample_type == 0 {
            0x11
        } else {
            p.multisample_type
        };
        mem.write_u32(dev + field::MULTISAMPLE_TYPE, ms)?;
        let sx = ((ms >> 4) & 0xF) as f32;
        let sy = (ms & 0xF) as f32;
        mem.write_u32(dev + field::BACK_BUFFER_SCALE_X, sx.to_bits())?;
        mem.write_u32(dev + field::BACK_BUFFER_SCALE_Y, sy.to_bits())?;
        mem.write_u32(dev + field::SUPERSAMPLE_X, sx.to_bits())?;
        mem.write_u32(dev + field::SUPERSAMPLE_Y, sy.to_bits())?;
        self.store_target_fields(mem)?;
        let offset = 0.53125f32.to_bits();
        mem.write_u32(dev + field::SCREEN_OFFSET_X, offset)?;
        mem.write_u32(dev + field::SCREEN_OFFSET_Y, offset)?;
        self.update_projection_viewport(mem)?;
        self.load_passthrough(mem, dev)?;
        let put = self.reserve(mem)?;
        let put = self.push_viewport(mem, put, dev)?;
        mem.write_u32(dev + field::PUT, put)?;
        Ok(())
    }

    /// The target-dependent fields SetRenderTarget maintains: the depth
    /// scale for the depth surface's format (`0x3fd970`, kept when there is
    /// no depth surface) and the target size (`0x3f6741`).
    pub(crate) fn store_target_fields(
        &mut self,
        mem: &mut dyn GuestMemory,
    ) -> Result<(), D3dError> {
        let dev = self.cfg.layout.device;
        if self.depth_stencil != 0 {
            let format = (mem.read_u32(at(self.depth_stencil, 12)?)? >> 8) & 0xFF;
            let scale: f32 = match format {
                0x2A | 0x2E => 16_777_215.0,
                0x2B | 0x2F => 1.0e30,
                0x2C | 0x30 => 65_535.0,
                0x2D | 0x31 => 511.9375,
                f => {
                    return Err(D3dError::invalid(
                        "SetRenderTarget",
                        format!("depth format {f:#x} has no depth scale"),
                    ));
                }
            };
            mem.write_u32(dev + field::Z_SCALE, scale.to_bits())?;
        }
        let (w, h) = self.target_size(mem)?;
        mem.write_u32(dev + field::TARGET_WIDTH, w)?;
        mem.write_u32(dev + field::TARGET_HEIGHT, h)?;
        Ok(())
    }

    /// `0x3f6d50` (`EAX` = handle `object | 1`): makes the object current
    /// at `+0x794` (handle at `+0x798`), sets dirty bits `0x70`, and when
    /// the shader kind changes (`([old+4] ^ [new+4]) & 0x13`) sets dirty
    /// bit `0x1000` and pushes the viewport (`0x3f6190`) and
    /// `SET_TRANSFORM_EXECUTION_MODE = [6, flags & 1]`.
    pub(crate) fn select_shader_object(
        &mut self,
        mem: &mut dyn GuestMemory,
        handle: u32,
    ) -> Result<(), D3dError> {
        let l = self.cfg.layout;
        let dev = l.device;
        let obj = handle - 1;
        let old = mem.read_u32(dev + field::VERTEX_SHADER)?;
        let old_flags = mem.read_u32(at(old, 4)?)?;
        let d = mem.read_u32(l.dirty_flags)? | 0x70;
        mem.write_u32(l.dirty_flags, d)?;
        mem.write_u32(dev + field::VERTEX_SHADER, obj)?;
        mem.write_u32(dev + 0x798, handle)?;
        let new_flags = mem.read_u32(at(obj, 4)?)?;
        if (old_flags ^ new_flags) & 0x13 != 0 {
            mem.write_u32(l.dirty_flags, d | 0x1000)?;
            let put = self.reserve(mem)?;
            let put = self.push_viewport(mem, put, dev)?;
            self.push_room("D3DDevice_SetVertexShader", put, 3)?;
            Self::write_words(
                mem,
                put,
                &[
                    0x0008_0000 | crate::hw::method::EXECUTION_MODE,
                    6,
                    new_flags & 1,
                ],
            )?;
            mem.write_u32(dev + field::PUT, put + 12)?;
        }
        Ok(())
    }

    /// The inline reservation every original setter starts with: if put has
    /// reached the threshold, `MakeRequestedSpace(kick/2, kick)`.
    pub(crate) fn reserve(&mut self, mem: &mut dyn GuestMemory) -> Result<u32, D3dError> {
        let dev = self.cfg.layout.device;
        let put = mem.read_u32(dev + field::PUT)?;
        if put >= mem.read_u32(dev + field::THRESHOLD)? {
            let kick = mem.read_u32(self.cfg.layout.kickoff_size)?;
            return self.make_requested_space(mem, kick / 2, kick);
        }
        Ok(put)
    }

    /// The W-buffer parameters `SetTransform(D3DTS_PROJECTION)` derives
    /// (`0x3f802c..0x3f80eb`) and the affine-projection flag.
    pub(crate) fn derive_w_params(&mut self, mem: &mut dyn GuestMemory) -> Result<(), D3dError> {
        let dev = self.cfg.layout.device;
        let p = read_matrix(mem, dev + field::PROJECTION)?;
        let (m14, m24, m33, m34, m43, m44) = (p[3], p[7], p[10], p[11], p[14], p[15]);
        let (near, far) = if m33 != m34 && m33 != 0.0 {
            let (m33d, m34d, m43d, m44d) = (m33 as f64, m34 as f64, m43 as f64, m44 as f64);
            (
                (m44d - (m43d / m33d) * m34d) as f32,
                (((m44d - m43d) / (m33d - m34d)) * m34d + m44d) as f32,
            )
        } else {
            (0.0, 1.0)
        };
        mem.write_u32(dev + field::W_NEAR, near.to_bits())?;
        mem.write_u32(dev + field::W_FAR, far.to_bits())?;
        mem.write_u32(
            dev + field::INV_W_FAR,
            ((1.0 / far as f64) as f32).to_bits(),
        )?;
        let mut flags = mem.read_u32(dev + field::FLAGS)? & !dev_flags::AFFINE_PROJECTION;
        if m14 == 0.0 && m24 == 0.0 && m34 == 0.0 && m44.to_bits() == 1.0f32.to_bits() {
            flags |= dev_flags::AFFINE_PROJECTION;
        }
        mem.write_u32(dev + field::FLAGS, flags)?;
        Ok(())
    }
}

/// Reads a row-major 4x4 `f32` matrix.
pub(crate) fn read_matrix(mem: &dyn GuestMemory, addr: u32) -> Result<[f32; 16], D3dError> {
    let w = crate::guest::read_array::<16>(mem, addr)?;
    Ok(std::array::from_fn(|i| f32::from_bits(w[i])))
}

/// `0x3fd080`: `a * b` in `f32`, row `i` accumulated as
/// `((a[i][0]*b0 + a[i][1]*b1) + a[i][2]*b2) + a[i][3]*b3` (the SSE order).
pub(crate) fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0f32; 16];
    for i in 0..4 {
        for c in 0..4 {
            let t = |j: usize| a[4 * i + j] * b[4 * j + c];
            out[4 * i + c] = ((t(0) + t(1)) + t(2)) + t(3);
        }
    }
    out
}
