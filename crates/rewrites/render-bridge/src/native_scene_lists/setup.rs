//! Whole-call replacement for object render setup. Keep render and object
//! cache writes staged until the last child has been visited.

mod lighting;
mod palette;
mod residency;
mod sections;
mod staged;

pub(in crate::native_scene_lists) use palette::{PALETTE_CURSOR, PALETTE_LIMIT, blend_group};
use records::ObjectRecord;
pub(in crate::native_scene_lists) use residency::resource_cache_in_use;
use sections::Prepared;
pub(super) use sections::{HIGHEST_LOD, LOD_FRACTION_MAX};
pub(in crate::native_scene_lists) use sections::{finest_lod, permutations, section_index};
use staged::Staged;

use super::lights::{MIN_DEPTH, PIXEL_SCALE, PROJECTION_DEPTH, SIZE_FACTOR};
use super::objects::{LIGHT_VIS, List, MAIN_VIS, VIEW_KIND, VIS_FLAGS};
use super::*;

const CAMERA: u32 = 0x004b_9da0;
const CAMERA_DISTANCE_SCALE: u32 = CAMERA + 0x2c;
const OBJECT_SIZE_SCALE: u32 = 0x004b_a028;
const FORCE_FULL_LOD: u32 = 0x004b_a019;
const FADE_SPAN_SCALE: u32 = 0x0045_dda8;
const FADE_MIN: u32 = 0x0045_df60;
const DEFAULT_MODEL_DISTANCE: u32 = 0x0044_aed4;
const ONE: u32 = 0x0045_dbc0;
const BYTE_SCALE: u32 = 0x0045_dc38;
const VIEW_CATEGORY: u32 = 0x004b_9ef0;
const VIEW_SKY_DATUM: u32 = 0x004b_9f8c;
const SKY_FADE_FRACTION: u32 = 0x004b_9ffc;
const SKY_FLAT_AMBIENT: u32 = 0x004b_a004;
const DISTANCE_EPSILON: u32 = 0x0045_dbd8;
const MAX_CHILD_DEPTH: usize = 64;
const CAMERA_FORWARD: u32 = CAMERA + 0xc;
const VIS_CAMERA_POSITION: u32 = 0x2a68;

#[derive(Clone, Copy)]
struct Scene<'a> {
    tags: u32,
    objects: Pool,
    frame: u32,
    vis: u32,
    kind: u32,
    retry: bool,
    lod: LodDetail,
    private: bool,
    geometry: Option<DisplayGeometry<'a>>,
}

#[derive(Clone, Copy)]
struct ObjectState {
    centre: [f32; 3],
    radius: f32,
    pixels: f32,
    /// Scaled camera distance, the model chooser's input (`0x0004bcc0`'s
    /// second argument). Children keep their root's, as the guest does.
    distance: f32,
    opacity: u8,
    lock_pose: bool,
    flags: u32,
    mode: u32,
    clip_mask: u16,
    group: u32,
    group_cookie: u32,
    group_counter: u32,
    attachment: u32,
    mirror_alpha: f32,
    effect_alpha: f32,
    group_depth: f32,
    mirror_mode: bool,
}

/// The two LOD fades as seen at the section builder's caller. Keep the
/// original comparison order around the clamp and near/far transitions.
fn fades(
    distance: f32,
    radius_span: f32,
    range_end: f32,
    minimum: f32,
    near: f32,
    near_start: f32,
    one: f32,
) -> (f32, f32) {
    let detail = if distance >= range_end + radius_span {
        0.0
    } else if distance > range_end {
        let factor = one - (distance - range_end) / radius_span;
        if factor < 0.0 {
            0.0
        } else if factor > one {
            one
        } else if minimum >= factor {
            0.0
        } else {
            factor
        }
    } else {
        one
    };
    let visible = if near <= 0.0 || distance <= near_start {
        one
    } else if distance >= near {
        0.0
    } else {
        one - (distance - near_start) / (near - near_start)
    };
    (visible, detail)
}

fn lod_bytes(
    m: &dyn GuestMemory,
    table: u32,
    handle: u32,
    body: u32,
    distance: f32,
    radius: f32,
) -> Result<(u8, u8)> {
    if m.u8(FORCE_FULL_LOD)? != 0 || m.u32(at(body, 4)?)? & (1 << 19) != 0 {
        return Ok((255, 255));
    }
    let object = tag(m, table, m.u32(body)?)?;
    let model = m.u32(at(object, 0x38)?)?;
    if model == NONE {
        return Ok((0, 0));
    }
    let root = lights::root_owner(m, handle)?;
    let model = if root != handle {
        let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
        let root_body = objects.body(m, root)?;
        let root_object = tag(m, table, m.u32(root_body)?)?;
        let root_model = m.u32(at(root_object, 0x38)?)?;
        if root_model == NONE {
            model
        } else {
            root_model
        }
    } else {
        model
    };
    let distances = at(tag(m, table, model)?, 0x28)?;
    let range = (4i32 - i32::from(m.u16(at(distances, 0x24)?)?)).clamp(0, 4);
    let range_end = if range == 0 {
        m.f32(DEFAULT_MODEL_DISTANCE)?
    } else {
        m.f32(at(distances, 0xc + range as u32 * 4)?)?
    };
    let near = m.f32(distances)?;
    let near_start = m.f32(at(distances, 4)?)?;
    let span = radius * m.f32(FADE_SPAN_SCALE)?;
    let min = m.f32(FADE_MIN)?;
    let one = m.f32(ONE)?;
    let scale = m.f32(BYTE_SCALE)?;
    if ![range_end, near, near_start, span, min, one, scale]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err(Refusal::Unsupported);
    }
    let (visible, detail) = fades(distance, span, range_end, min, near, near_start, one);
    let byte = |v: f32| -> Result<u8> {
        // The CRT helper truncates the x87 product to an integer.
        let scaled = f64::from(v) * f64::from(scale);
        if !(0.0..=255.0).contains(&scaled) {
            return Err(Refusal::Unsupported);
        }
        Ok(scaled as u8)
    };
    Ok((byte(visible)?, byte(detail)?))
}

pub(super) fn section_flags(m: &dyn GuestMemory, vis: u32, tags: u32, flags: u16) -> Result<u32> {
    let flags = u32::from(flags);
    let mut section = ((((flags >> 2) & 0x2c00) | (flags & 0x4000)) >> 9) | ((flags & 0x200) << 5);
    if m.u8(at(vis, VIS_FLAGS)?)? & 0x80 != 0 {
        section |= 1;
    }
    if m.u32(VIEW_CATEGORY)? == 3 {
        let sky = m.u32(VIEW_SKY_DATUM)?;
        if sky != NONE
            && flags & 0x400 == 0
            && m.f32(SKY_FADE_FRACTION)? == m.f32(DISTANCE_EPSILON)?
            && m.u8(tag(m, tags, sky)?)? & 2 == 0
        {
            section |= 0x100;
        }
        if m.u8(SKY_FLAT_AMBIENT)? == 0 || flags & 0x400 != 0 {
            section |= 0x80;
        }
    }
    Ok(section)
}

pub(super) fn build(m: &dyn GuestMemory, cpu: &CpuAbi, detail: LodDetail) -> (SceneList, Result<Output>) {
    (SceneList::ObjectSetup, setup(m, cpu, detail))
}

/// [`super::residency_request`].
pub(super) fn residency_request(m: &dyn GuestMemory, resource: u32, request: bool) -> ResidencyRequest {
    let mut staged = Staged::new(m);
    match residency::touch(&mut staged, resource, request) {
        Ok(resident) => {
            let writes = staged.finish().writes.into_iter().map(|w| (w.addr, w.bytes.slice().to_vec())).collect();
            ResidencyRequest::Made { resident, writes }
        }
        Err(Refusal::Residency) => ResidencyRequest::NeedsGuestService,
        Err(_) => ResidencyRequest::Unreadable,
    }
}

fn projected(m: &dyn GuestMemory, body: u32) -> Result<([f32; 3], f32, f32, f32)> {
    let [px, py, pz] = m.vec3(at(body, 0x30)?)?;
    let radius = m.f32(at(body, 0x3c)?)?;
    let [x, y, z, w] = PROJECTION_DEPTH;
    let [x, y, z, w] = [m.f32(x)?, m.f32(y)?, m.f32(z)?, m.f32(w)?];
    let depth = (((z * pz + y * py) + x * px) + w).abs();
    let min_depth = m.f32(MIN_DEPTH)?;
    let depth = if depth > min_depth { depth } else { min_depth };
    let pixel_size = (((m.f32(PIXEL_SCALE)? / depth) * radius) * m.f32(SIZE_FACTOR)?)
        * m.f32(OBJECT_SIZE_SCALE)?;
    let [cx, cy, cz] = m.vec3(CAMERA)?;
    let (dx, dy, dz) = (
        f64::from(px) - f64::from(cx),
        f64::from(py) - f64::from(cy),
        f64::from(pz) - f64::from(cz),
    );
    let distance =
        (((dz * dz + dy * dy) + dx * dx).sqrt() * f64::from(m.f32(CAMERA_DISTANCE_SCALE)?)) as f32;
    if ![px, py, pz, radius, depth, pixel_size, distance]
        .into_iter()
        .all(f32::is_finite)
    {
        return Err(Refusal::Unsupported);
    }

    Ok(([px, py, pz], radius, pixel_size, distance))
}

fn setup(m: &dyn GuestMemory, cpu: &CpuAbi, lod: LodDetail) -> Result<Output> {
    if let Some(out) = outside_gather_lens(m, cpu, |handle| crate::scene_step::gather_lens_object(m, handle))? {
        return Ok(out);
    }
    fp_supported(cpu)?;
    if cpu.x87.control != 0x027f || cpu.x87.tag != 0xffff {
        return Err(Refusal::Unsupported);
    }
    let vis = cpu.esi;
    let kind = m.u32(at(vis, VIEW_KIND)?)?;
    if !matches!((vis, kind), (MAIN_VIS, 0) | (LIGHT_VIS, 1 | 2)) {
        return Err(Refusal::Unsupported);
    }
    let list_addr = ptr(m, at(vis, 0x14)?)?;
    let list = List::read(m, list_addr)?;
    let index = u32::from(stack(m, cpu, 4)? as u16);
    if index as usize >= list.count {
        return Err(Refusal::Layout);
    }
    let handle = m.u32(at(list.column(m, 0xc)?, index * 4)?)?;
    let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
    let body = objects.body(m, handle)?;
    let (centre, radius, pixel_size, distance) = projected(m, body)?;

    let tags = ptr(m, TAGS)?;
    let (opacity, _) = lod_bytes(m, tags, handle, body, distance, radius)?;
    let row_flags = m.u16(at(list.column(m, 8)?, index * 2)?)?;
    let flags = section_flags(m, vis, tags, row_flags)?;
    let mask = if row_flags & 0x800 == 0 {
        m.u16(at(list.column(m, 0x14)?, index * 2)?)?
    } else {
        u16::MAX
    };
    let explicit_retry = stack(m, cpu, 8)? as u8 != 0;
    let scene = Scene {
        tags,
        lod,
        private: false,
        geometry: None,
        objects,
        frame: m.u32(FRAME)?,
        vis,
        kind,
        retry: kind == 2 || explicit_retry,
    };
    let mode = if mask == u16::MAX { 1 } else { 0x40 };
    let mut state = ObjectState {
        centre,
        radius,
        pixels: pixel_size,
        distance,
        opacity,
        lock_pose: row_flags & 0x200 != 0,
        flags,
        mode: mode | 0x208,
        clip_mask: mask,
        group: stack(m, cpu, 16)?,
        group_cookie: m.u32(cpu.esp.checked_sub(0x20).ok_or(Refusal::Layout)?)?,
        group_counter: stack(m, cpu, 12)?,
        attachment: NONE,
        mirror_alpha: 0.0,
        effect_alpha: 0.0,
        group_depth: 0.0,
        mirror_mode: false,
    };
    let mut staged = Staged::new(m);
    let mut prepared = sections::prepare(&mut staged, scene, handle, state)?;
    if explicit_retry {
        prepared.first_person = false;
        state.flags = (state.flags & !2) | 4;
    } else if prepared.first_person && kind == 1 {
        state.flags &= !4;
    }
    let mut tail_registers = None;
    if opacity == 0 {
        let column = list.column(m, 8)?;
        let address = mark_no_draw(&mut staged, column, index)?;
        tail_registers = Some((address, column));
    } else if prepared.effect_flags & 2 == 0 || kind == 0 {
        let descend = stack(m, cpu, 20)? as u8 != 0;
        Walk {
            scene,
            ancestors: Vec::new(),
        }
        .visit(&mut staged, handle, &prepared, state, descend, false)?;
    }
    let mut out = staged.finish();
    if let Some((ecx, edx)) = tail_registers {
        out.ecx = Some(ecx);
        out.edx = Some(edx);
    }
    out.al = Some(u8::from(prepared.first_person));
    Ok(out)
}

/// A main-view object a window wider than the gather lens listed outside
/// that lens's set (`scene_step::gather_lens_object`): the "not drawn"
/// exit, before any of the setup runs. Its row's flag bit 0 is set, AL is 0, and ECX
/// and EDX hold the flag's address and the column, as the setup's own
/// zero-opacity exit leaves them. No section choice, pose evaluation
/// (`0x000bd610`, whose `0x000bd7b2` evaluates the shared node matrices
/// and clears the object's dirty bit), animation selection, palette,
/// lighting or record write runs for it, so the simulation finds the
/// object as the gather lens leaves it in every window
/// (docs/graphics/high-resolution.md, "Determinism"). `None`: the setup
/// runs.
fn outside_gather_lens(
    m: &dyn GuestMemory, cpu: &CpuAbi, gathered: impl Fn(u32) -> Option<bool>,
) -> Result<Option<Output>> {
    let vis = cpu.esi;
    if vis != MAIN_VIS || m.u32(at(vis, VIEW_KIND)?)? != 0 {
        return Ok(None);
    }
    let list_addr = ptr(m, at(vis, 0x14)?)?;
    let list = List::read(m, list_addr)?;
    let index = u32::from(stack(m, cpu, 4)? as u16);
    if index as usize >= list.count {
        return Ok(None);
    }
    let handle = m.u32(at(list.column(m, 0xc)?, index * 4)?)?;
    if gathered(handle) != Some(false) {
        return Ok(None);
    }
    let column = list.column(m, 8)?;
    let mut staged = Staged::new(m);
    let address = mark_no_draw(&mut staged, column, index)?;
    let mut out = staged.finish();
    out.ecx = Some(address);
    out.edx = Some(column);
    out.al = Some(0);
    Ok(Some(out))
}

fn mark_no_draw(m: &mut Staged<'_>, column: u32, index: u32) -> Result<u32> {
    let address = at(column, index * 2)?;
    let flags = m.u8(address)?;
    m.set_u8(address, flags | 1)?;
    Ok(address)
}

/// Layer-3 section inputs selected on a private overlay of the game's
/// current state. The overlay owns every palette, cache and record write.
pub struct DisplaySection {
    pub section: u16,
    pub implementation: usize,
    pub rows: Vec<frame_scene::Row>,
}

/// Section node remaps from the native renderer's mounted model.
#[derive(Clone, Copy)]
pub struct DisplayGeometry<'a> {
    pub model: u32,
    pub node_maps: &'a [Vec<u8>],
}

/// Produce an ordinary third-person object's records without publishing
/// any renderer, resource, lighting, animation or simulation state.
pub fn display_model(
    m: &dyn GuestMemory, handle: u32, lod: LodDetail, geometry: Option<DisplayGeometry<'_>>,
) -> Option<(frame_scene::ObjectFrame, u32, Vec<DisplaySection>)> {
    display_model_inner(m, handle, lod, geometry).ok()
}

fn display_model_inner(
    m: &dyn GuestMemory, handle: u32, lod: LodDetail, geometry: Option<DisplayGeometry<'_>>,
) -> Result<(frame_scene::ObjectFrame, u32, Vec<DisplaySection>)> {
    let objects = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
    let body = objects.body(m, handle)?;
    let (centre, radius, pixels, distance) = projected(m, body)?;
    let scene = Scene {
        tags: ptr(m, TAGS)?, objects, frame: m.u32(FRAME)?, vis: MAIN_VIS,
        kind: 0, retry: false, lod, private: true, geometry,
    };
    let (opacity, _) = lod_bytes(m, scene.tags, handle, body, distance, radius)?;
    // The display gather supplies the object set, without the optional
    // secondary clipping plane, restriction list, or cluster subpart mask.
    if opacity != u8::MAX || m.u32(at(body, 0x14)?)? != NONE
        || m.u32(at(MAIN_VIS, VIS_FLAGS)?)? & 0x201 != 0
        || m.u8(at(body, 7)?)? & 0x80 != 0
    {
        return Err(Refusal::ObjectModel);
    }
    let state = ObjectState {
        centre, radius, pixels, distance, opacity, lock_pose: false,
        flags: section_flags(m, MAIN_VIS, scene.tags, 0xf000)?, mode: 1 | 0x208, clip_mask: u16::MAX,
        group: 31, group_cookie: NONE, group_counter: 0, attachment: NONE,
        mirror_alpha: 0., effect_alpha: 0., group_depth: 0., mirror_mode: false,
    };
    let mut overlay = Staged::new(m);
    overlay.set_u32(palette::PALETTE_CURSOR, 0)?;
    let prepared = sections::prepare(&mut overlay, scene, handle, state)?;
    let [model] = prepared.models.as_slice() else { return Err(Refusal::ObjectModel) };
    if prepared.first_person || prepared.overlay.is_some() || prepared.function_overlay.is_some()
        || prepared.variant_overlays.iter().any(Option::is_some)
    {
        return Err(Refusal::ObjectModel);
    }
    // These scratch records are private; a full guest list cannot stop a
    // fresh display record. The palette still respects the live model cache.
    overlay.set_u32(records::COUNT, 0)?;
    overlay.set_u32(records::CURRENT_VIS, MAIN_VIS)?;
    Walk { scene, ancestors: Vec::new() }.emit(&mut overlay, &prepared, state, true)?;
    let count = overlay.u32(records::COUNT)?;
    let read = crate::native_world::Mem(&mut overlay);
    use scene_extract::layout::object_lighting::LightingRead;
    let mut frame = native_render::objects::read_object(&read, handle).ok_or(Refusal::ObjectModel)?;
    frame.lighting = Some(frame_scene::Lighting::read(&read, model.lighting).ok_or(Refusal::Lighting)?);
    let mut sections = Vec::new();
    for i in 0..count {
        use native_render::{camera::GuestRead, object_nodes, skip};
        let addr = skip::RENDER_RECORDS + i * skip::RENDER_RECORD_SIZE;
        let word = read.read_u32(addr).ok_or(Refusal::Layout)?;
        if word & (native_render::object_walk::RECORD_AUX_STATE | native_render::object_walk::RECORD_STENCIL_MARK) != 0 {
            return Err(Refusal::Unsupported);
        }
        let key = read.read_u32(addr + object_nodes::RECORD_KEY).ok_or(Refusal::Layout)?;
        sections.push(DisplaySection {
            section: ((key >> 9) & 0x1ff) as u16,
            implementation: object_nodes::record_implementation(word),
            rows: object_nodes::node_rows(&read, addr).map_err(|_| Refusal::Palette)?,
        });
    }
    Ok((frame, model.datum, sections))
}

struct Walk<'a> {
    scene: Scene<'a>,
    ancestors: Vec<u32>,
}

impl Walk<'_> {
    fn visit(
        &mut self,
        m: &mut Staged<'_>,
        handle: u32,
        prepared: &Prepared,
        mut state: ObjectState,
        descend: bool,
        overlay_only: bool,
    ) -> Result<()> {
        let mut emit = true;
        if !prepared.models.is_empty() && !overlay_only {
            let mut group_set = false;
            if let Some(overlay) = &prepared.function_overlay {
                state.group_depth = self.depth(m, state.centre)?;
                Self::allocate_group(m, &mut state, 3)?;
                group_set = true;
                let pass = ObjectState {
                    flags: state.mode,
                    mode: 0,
                    attachment: overlay.attachment,
                    effect_alpha: overlay.alpha,
                    ..state
                };
                self.visit(m, handle, prepared, pass, true, true)?;
            }
            if let Some(overlay) = &prepared.overlay {
                if !group_set {
                    Self::allocate_group(m, &mut state, 1)?;
                }
                // Camo draws instead of the base record. Both passes
                // independently traverse attachments.
                let pass = ObjectState {
                    flags: state.mode,
                    mode: 0,
                    attachment: overlay.attachment,
                    mirror_alpha: overlay.alpha,
                    mirror_mode: true,
                    group_depth: self.depth(m, state.centre)?,
                    ..state
                };
                self.visit(m, handle, prepared, pass, true, true)?;
                emit = false;
            }
            for overlay in prepared.variant_overlays.iter().flatten() {
                let pass = ObjectState {
                    flags: state.flags | 0x8000,
                    attachment: overlay.attachment,
                    effect_alpha: overlay.alpha,
                    ..state
                };
                self.visit(m, handle, prepared, pass, false, true)?;
            }
        }
        if emit {
            self.emit(m, prepared, state, descend)?;
        }
        if !descend || prepared.first_person {
            return Ok(());
        }
        if self.ancestors.len() == MAX_CHILD_DEPTH || self.ancestors.contains(&handle) {
            return Err(Refusal::Layout);
        }
        self.ancestors.push(handle);
        let body = self.scene.objects.body(m, handle)?;
        let mut child = m.u32(at(body, 0x10)?)?;
        for _ in 0..self.scene.objects.end {
            if child == NONE {
                break;
            }
            if self.ancestors.contains(&child) {
                return Err(Refusal::Layout);
            }
            let mut child_state = ObjectState {
                clip_mask: u16::MAX,
                mode: (state.mode & !0x40) | 1,
                ..state
            };
            self.split_weapon(m, child, &mut child_state)?;
            let child_setup = sections::prepare(m, self.scene, child, child_state)?;
            self.visit(m, child, &child_setup, child_state, true, overlay_only)?;
            let body = self.scene.objects.body(m, child)?;
            child = m.u32(at(body, 0xc)?)?;
        }
        self.ancestors.pop();
        if child != NONE {
            return Err(Refusal::Layout);
        }
        Ok(())
    }

    fn depth(&self, m: &dyn GuestMemory, centre: [f32; 3]) -> Result<f32> {
        let eye = m.vec3(at(self.scene.vis, VIS_CAMERA_POSITION)?)?;
        let forward = m.vec3(CAMERA_FORWARD)?;
        let x = (centre[0] - eye[0]) * forward[0];
        let y = (centre[1] - eye[1]) * forward[1];
        let z = (centre[2] - eye[2]) * forward[2];
        let depth = (z + y) + x;
        if depth.is_finite() {
            Ok(depth)
        } else {
            Err(Refusal::ObjectModel)
        }
    }

    fn allocate_group(m: &mut Staged<'_>, state: &mut ObjectState, cookie: u32) -> Result<()> {
        let next = m.u32(state.group_counter)?;
        m.set_u32(state.group_counter, next.wrapping_add(1))?;
        state.group = next;
        state.group_cookie = cookie;
        Ok(())
    }

    fn emit(
        &self,
        m: &mut Staged<'_>,
        prepared: &Prepared,
        state: ObjectState,
        descend: bool,
    ) -> Result<()> {
        let mut first_owner = None;
        for model in &prepared.models {
            let mut record_state = state;
            let owner = m.u32(model.lighting)?;
            if !descend && *first_owner.get_or_insert(owner) != owner {
                continue;
            }
            self.split_weapon(m, owner, &mut record_state)?;
            for (region, &section) in model.sections.iter().enumerate() {
                let record = ObjectRecord {
                    section: if section == u8::MAX {
                        NONE
                    } else {
                        u32::from(section)
                    },
                    selector: model.selector,
                    node_count: model.node_count,
                    flags: record_state.flags
                        | record_state.mode
                        | model.draw_flags
                        | (u32::from(prepared.first_person) << 10),
                    model: model.datum,
                    palette: model.palette,
                    region: region as u32,
                    lighting: model.lighting,
                    clip_mask: record_state.clip_mask,
                    opacity: record_state.opacity,
                    attachment: record_state.attachment,
                    pixels: record_state.pixels,
                    group: record_state.group,
                    group_cookie: record_state.group_cookie,
                    clip_flags: model.attributes[region],
                    full_rect: prepared.effect_flags & 0xa == 0xa,
                    empty_rect: prepared.effect_flags & 0x12 == 0x12,
                    mirror_mode: record_state.mirror_mode,
                    mirror_alpha: record_state.mirror_alpha,
                    effect_alpha: record_state.effect_alpha,
                    group_depth: record_state.group_depth,
                    sphere: (record_state.centre, record_state.radius),
                };
                let writes = record.plan(m, self.scene.lod.pixel_multiplier(), self.scene.geometry.is_some())?;
                m.apply(writes)?;
            }
        }
        Ok(())
    }

    fn split_weapon(
        &self,
        m: &dyn GuestMemory,
        handle: u32,
        state: &mut ObjectState,
    ) -> Result<()> {
        if state.group_cookie == 1 {
            let body = self.scene.objects.body(m, handle)?;
            if m.u8(at(body, 0xaa)?)? == 2 {
                let def = tag(m, self.scene.tags, m.u32(body)?)?;
                if m.u16(at(def, 0x290)?)? != 0 {
                    state.group = 31;
                    state.group_cookie = NONE;
                    state.attachment = NONE;
                    state.mirror_mode = false;
                    state.mirror_alpha = 0.0;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_setup_lod_uses_both_fade_ranges() {
        let pick = |distance| fades(distance, 10.0, 50.0, 0.001, 20.0, 10.0, 1.0);
        assert_eq!(pick(15.0), (0.5, 1.0));
        assert_eq!(pick(25.0), (0.0, 1.0));
        assert_eq!(pick(55.0), (0.0, 0.5));
        assert_eq!(pick(60.0), (0.0, 0.0));
    }
}

#[cfg(test)]
mod setup_tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use native_render::object_nodes::{BLOCK_MATRICES, MATRIX_STRIDE, NODE_BLOCKS};
    use native_render::skip::{RENDER_RECORD_SIZE, RENDER_RECORDS};

    const LIST: u32 = 0x10000;
    const FLAGS: u32 = 0x10080;
    const OBJECT_POOL: u32 = 0x11000;
    const OBJECT_SLOTS: u32 = 0x11100;
    const ROOT: u32 = 0x12000;
    const CHILD: u32 = 0x13000;
    const ROOT_HANDLE: u32 = 0x7100_0000;
    const CHILD_HANDLE: u32 = 0x7100_0001;
    const TAG_TABLE: u32 = 0x14000;
    const DEFINITION: u32 = 0x15000;
    const HLMT: u32 = 0x16000;
    const MODEL: u32 = 0x17000;
    const SECTIONS: u32 = 0x18000;
    const LIGHT_CACHE: u32 = 0x19000;
    const CACHE_ENTRIES: u32 = LIGHT_CACHE + 0x100;
    const RESOURCE_CACHE: u32 = 0x1a000;
    const RESOURCE_POOL: u32 = 0x1a100;
    const RESOURCE_SLOTS: u32 = 0x1a200;
    const STACK: u32 = 0x1b100;
    const CURRENT_VIS: u32 = 0x004c_0b80;
    const RECORD_COUNT: u32 = 0x004c_0b78;
    const MAIN_COUNT: u32 = 0x004c_0b7c;

    fn fixture() -> (FakeGuest, CpuAbi) {
        let mut mem = FakeGuest::new();
        mem.map(LIST, 0xc000)
            .map(0x004c_1000, 0x1000)
            .map(NODE_BLOCKS - 4, 0x400)
            .map(RENDER_RECORDS, 2 * RENDER_RECORD_SIZE)
            .map(0x0045_dc00, 0x100)
            .map(0x004b_9000, 0x1000)
            .put_u32(MAIN_VIS + 0x14, LIST)
            .put_u32(MAIN_VIS + VIEW_KIND, 0)
            .put_u32(LIST, 4)
            .put_u16(LIST + 4, 1)
            .put_u32(LIST + 8, FLAGS)
            .put_u32(LIST + 0xc, LIST + 0x90)
            .put_u32(LIST + 0x14, LIST + 0xa0)
            .put_u16(LIST + 0xa0, u16::MAX)
            .put_u32(LIST + 0x90, ROOT_HANDLE)
            .put_u32(OBJECTS, OBJECT_POOL)
            .put_u32(OBJECT_POOL + 0x24, 12)
            .put_u32(OBJECT_POOL + 0x38, 2)
            .put_u32(OBJECT_POOL + 0x44, OBJECT_SLOTS)
            .put_u32(TAGS, TAG_TABLE)
            .put_u32(FRAME, 23)
            .put_f32(PROJECTION_DEPTH[3], 1.0)
            .put_f32(PIXEL_SCALE, 10.0)
            .put_f32(SIZE_FACTOR, 1.0)
            .put_f32(OBJECT_SIZE_SCALE, 1.0)
            .put_f32(CAMERA_DISTANCE_SCALE, 1.0)
            .put_u32(CURRENT_VIS, MAIN_VIS)
            .put_u32(sections::FIRST_PERSON_OWNER, NONE)
            .put_u32(0x004e_0344, 0)
            .put_u32(0x0050_9434, LIGHT_CACHE)
            .put_u32(0x004d_e2f4, 0x19e00)
            .put_u32(LIGHT_CACHE + 0x24, 0x100)
            .put_u32(LIGHT_CACHE + 0x38, 2)
            .put_u32(LIGHT_CACHE + 0x44, CACHE_ENTRIES)
            .put_u32(0x004e_649c, RESOURCE_CACHE)
            .put_u32(0x004e_6494, RESOURCE_CACHE)
            .put_u32(RESOURCE_CACHE + 0x38, 73)
            .put_u32(RESOURCE_CACHE + 0x64, RESOURCE_POOL)
            .put_u32(RESOURCE_POOL + 0x24, 0x18)
            .put_u32(RESOURCE_POOL + 0x38, 2)
            .put_u32(RESOURCE_POOL + 0x44, RESOURCE_SLOTS)
            .put_u32(STACK + 16, NONE)
            .put_u32(STACK + 20, 1);

        let def = 0x4141_0001;
        let hlmt = 0x4141_0002;
        let model = 0x4141_0003;
        for (datum, addr) in [(def, DEFINITION), (hlmt, HLMT), (model, MODEL)] {
            let entry = TAG_TABLE + (datum & 0xffff) * 16;
            mem.put_u32(entry + 4, datum).put_u32(entry + 8, addr);
        }
        mem.put_u32(DEFINITION + 0x38, hlmt)
            .put_u32(HLMT + 4, model)
            .put_u32(HLMT + 0xf4, NONE)
            .put_u32(MODEL + 4, 4)
            .put_u32(MODEL + 0x1c, 1)
            .put_u32(MODEL + 0x20, 0x17500)
            .put_u32(0x17508, 2)
            .put_u32(0x1750c, 0x17510)
            .put_u16(0x17510 + 14, 0)
            .put_u16(0x17520 + 14, 1)
            .put_u32(MODEL + 0x24, 2)
            .put_u32(MODEL + 0x28, SECTIONS)
            .put_u32(MODEL + 0x34, 1)
            .put_u32(MODEL + 0x38, 0x17600)
            .put_u32(MODEL + 0x48, 1)
            .put_u32(MODEL + 0x4c, 0x17700);

        let identity = [
            1.0f32, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
        ];
        let matrix: Vec<_> = identity.into_iter().flat_map(f32::to_le_bytes).collect();
        mem.put(0x17728, &matrix);
        for (i, (handle, body, first, parent, permutation)) in [
            (ROOT_HANDLE, ROOT, CHILD_HANDLE, NONE, 1u8),
            (CHILD_HANDLE, CHILD, NONE, ROOT_HANDLE, 0u8),
        ]
        .into_iter()
        .enumerate()
        {
            let index = i as u32;
            let slot = OBJECT_SLOTS + 12 * index;
            let entry = CACHE_ENTRIES + 0x100 * index;
            let section = SECTIONS + 0x5c * index;
            mem.put_u16(slot, (handle >> 16) as u16)
                .put_u8(slot + 3, 2)
                .put_u32(slot + 8, body)
                .put_u32(body, def)
                .put_u32(body + 4, (1 << 19) | (1 << 24))
                .put_u32(body + 0xc, NONE)
                .put_u32(body + 0x10, first)
                .put_u32(body + 0x14, parent)
                .put_f32(body + 0x3c, 1.0)
                .put_u8(body + 0xaa, 2)
                .put_u8(body + 0xb1, u8::MAX)
                .put_u32(body + 0xcc, 0x7a00_0000 | index)
                .put_u16(body + 0x112, u16::MAX)
                .put_u16(body + 0x114, 0x34)
                .put_u16(body + 0x116, 0x200)
                .put_u16(body + 0x118, 10)
                .put_u16(body + 0x11a, 0x280)
                .put_u8(body + 0x280, permutation)
                .put(body + 0x200, &matrix)
                .put_u32(entry + 4, handle)
                .put_u32(entry + 0x44, handle)
                .put_u32(entry + 0x4c, NONE)
                .put_u8(entry + 0xbc, u8::MAX)
                .put_u32(section + 0x34, 0x18500)
                .put_u8(section + 0x38 + 0x1e, 1)
                .put_u32(section + 0x38 + 0x20, index);
        }
        let mut cpu = CpuAbi::xbox_thread();
        cpu.esi = MAIN_VIS;
        cpu.esp = STACK;
        (mem, cpu)
    }

    fn publish(mem: &mut FakeGuest, out: Output) {
        for write in out.writes {
            mem.write(write.addr, write.bytes.slice()).unwrap();
        }
    }

    /// An object outside the gather lens's set takes the "not drawn" exit
    /// before anything of its setup runs: only its row's flag changes, so
    /// its pose, dirty bit and node matrices stay as the simulation left
    /// them. Inside the set, or with no set, the setup runs.
    #[test]
    fn an_object_outside_the_gather_lens_is_left_undrawn_and_untouched() {
        let (mut m, cpu) = fixture();
        let body = m.bytes(ROOT, 0x300).unwrap();
        let out = outside_gather_lens(&m, &cpu, |handle| Some(handle != ROOT_HANDLE)).unwrap().unwrap();
        assert_eq!((out.al, out.ecx, out.edx), (Some(0), Some(FLAGS), Some(FLAGS)));
        assert_eq!(out.writes.len(), 1, "the row's flag, nothing else");
        publish(&mut m, out);
        assert_eq!(m.u8(FLAGS).unwrap() & 1, 1);
        assert_eq!(m.bytes(ROOT, 0x300).unwrap(), body, "the object's pose and flags are the simulation's");

        let (m, cpu) = fixture();
        assert!(outside_gather_lens(&m, &cpu, |_| Some(true)).unwrap().is_none(), "an object in the set");
        assert!(outside_gather_lens(&m, &cpu, |_| None).unwrap().is_none(), "no window wider than the lens");
        let mut light = cpu;
        light.esi = LIGHT_VIS;
        assert!(outside_gather_lens(&m, &light, |_| Some(false)).unwrap().is_none(), "a light's view");
    }

    /// The gather-lens fixture reversed (docs/engine/sim-render-split.md,
    /// "2. Node matrices"). Both objects have a dirty pose and
    /// pose-dependent sections, and the view lists only the root. The
    /// tick-end phase plans both, parent first; once it has built them
    /// (bit 30 cleared, as `0x000bd090` leaves it) the setup draws without
    /// handing the pose to the guest, and writes neither body's flags nor
    /// its node matrices (`+0xcc` is slice 8's render scratch).
    #[test]
    fn after_the_tick_end_build_the_setup_reads_every_pose_and_writes_none() {
        use engine::tick_poses::{self, DIRTY};
        let (mut m, cpu) = fixture();
        m.put_u32(HLMT + 0xc4, 0b11);
        for body in [ROOT, CHILD] {
            m.put_u16(body + 0x112, 0x500).put_u32(body + 4, (1 << 19) | (1 << 24) | DIRTY);
        }
        assert!(matches!(setup(&m, &cpu, LodDetail::Retail), Err(Refusal::Palette)), "a dirty pose is the guest's");
        assert_eq!(tick_poses::plan(&m), Some(vec![ROOT_HANDLE, CHILD_HANDLE]));

        for body in [ROOT, CHILD] {
            let flags = m.u32(body + 4).unwrap();
            m.put_u32(body + 4, flags & !DIRTY);
        }
        assert_eq!(tick_poses::plan(&m), Some(vec![]));
        // The flags word and the node matrices (`+0x116`, `+0x114` bytes).
        let pose = |m: &FakeGuest, body: u32| (m.u32(body + 4).unwrap(), m.bytes(body + 0x200, 0x34).unwrap());
        let before = [pose(&m, ROOT), pose(&m, CHILD)];
        let out = setup(&m, &cpu, LodDetail::Retail).unwrap();
        let touches = |from: u32, len: u32| {
            out.writes.iter().any(|w| w.addr < from + len && w.addr + w.bytes.slice().len() as u32 > from)
        };
        for body in [ROOT, CHILD] {
            assert!(!touches(body + 4, 4) && !touches(body + 0x200, 0x34), "the frame step builds no pose");
        }
        publish(&mut m, out);
        assert_eq!(m.u32(MAIN_COUNT).unwrap(), 2);
        assert_eq!([pose(&m, ROOT), pose(&m, CHILD)], before);
    }

    #[test]
    fn private_sections_use_the_current_permutation_and_leave_the_record_list_untouched() {
        let (mut m, _) = fixture();
        let (_, _, sections) = display_model(&m, ROOT_HANDLE, LodDetail::Retail, None).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].section, 1);
        assert_eq!(sections[0].rows.len(), 3);
        assert_eq!(sections[0].rows[0], [1., 0., 0., 0.]);
        assert_eq!(m.u32(RECORD_COUNT).unwrap(), 0);
        assert_eq!(m.u32(NODE_BLOCKS - 4).unwrap(), 0);
        assert_eq!(m.u32(CACHE_ENTRIES + 0x4c).unwrap(), NONE);
        m.put_u8(ROOT + 4, 1);
        assert!(display_model(&m, ROOT_HANDLE, LodDetail::Retail, None).is_none());
        m.put_u8(ROOT + 4, 0).put_u8(OBJECT_SLOTS + 2, 0x10);
        assert!(display_model(&m, ROOT_HANDLE, LodDetail::Retail, None).is_none());
    }

    #[test]
    fn highest_selects_the_finest_section_at_long_range_on_a_private_overlay() {
        let (mut m, _) = fixture();
        // Keep the object visible while putting it beyond every detail
        // threshold. The active permutation has distinct far and near meshes.
        m.put_u32(ROOT + 4, 1 << 24)
            .put_f32(ROOT + 0x30, 100_000.0)
            .put_f32(DEFAULT_MODEL_DISTANCE, 200_000.0)
            .put_f32(ONE, 1.0)
            .put_f32(FADE_SPAN_SCALE, 2.0)
            .put_f32(FADE_MIN, 0.01)
            .put_f32(0x0044_ae88, 0.2)
            .put_f32(0x0045_dc5c, 0.8)
            .put_f32(BYTE_SCALE, 255.0)
            .put_u32(0x004b_a01c, 5)
            .put_f32(LOD_FRACTION_MAX, 1.0)
            .put_u16(0x17520 + 4, 0)
            .put_u32(HLMT + 0xc4, 1 << 1);
        for (i, threshold) in [100., 80., 60., 40., 20., 0.].into_iter().enumerate() {
            m.put_f32(HLMT + 0x34 + i as u32 * 4, threshold);
        }
        let before = m.clone();
        let (_, _, retail) = display_model(&m, ROOT_HANDLE, LodDetail::Retail, None).unwrap();
        let (_, _, highest) = display_model(&m, ROOT_HANDLE, LodDetail::Highest, None).unwrap();
        assert_eq!(retail[0].section, 0);
        assert_eq!(highest[0].section, 1);
        for (base, len) in [(ROOT, 0x300), (CACHE_ENTRIES, 0x100), (RENDER_RECORDS, RENDER_RECORD_SIZE as usize)] {
            assert_eq!(m.bytes(base, len).unwrap(), before.bytes(base, len).unwrap());
        }
    }

    #[test]
    fn native_display_uses_mounted_sections_without_requesting_guest_resources() {
        let (mut m, _) = fixture();
        let resource = SECTIONS + 0x5c;
        m.put_u32(resource + 0x34, 0)
            .put_u8(resource + 0x38 + 0x1e, 0)
            .put_u32(resource + 0x38 + 8, 128)
            .put_u32(resource + 0x38 + 0x20, NONE);
        let mounted = DisplayGeometry {
            model: 0x4141_0003,
            node_maps: &[Vec::new(), Vec::new()],
        };
        let (_, _, highest) = display_model(&m, ROOT_HANDLE, LodDetail::Highest, Some(mounted)).unwrap();
        assert_eq!(highest[0].section, 1);
        assert_eq!(m.u8(resource + 0x38 + 0x1e).unwrap(), 0);
        assert_eq!(m.u32(resource + 0x38 + 0x20).unwrap(), NONE);
    }

    #[test]
    fn highest_uses_the_next_authored_tier_when_the_finest_is_absent() {
        let (mut m, _) = fixture();
        m.put_u16(0x17520 + 14, u16::MAX)
            .put_u16(0x17520 + 12, 1);
        let (_, _, highest) = display_model(&m, ROOT_HANDLE, LodDetail::Highest, None).unwrap();
        assert_eq!(highest[0].section, 1);
    }

    #[test]
    fn both_arm_sections_use_their_own_pose_or_return_the_whole_setup_to_the_guest() {
        let (mut m, cpu) = fixture();
        let identity = m.bytes(ROOT + 0x200, 0x34).unwrap();
        let regions = 0x17200;
        let permutations = 0x17300;
        let actor = ROOT + 0x600;
        let animation = ROOT + 0x700;
        m.put_u32(ROOT + 0x10, NONE)
            .put_u32(MODEL + 4, 0)
            .put_u32(MODEL + 0x1c, 2)
            .put_u32(MODEL + 0x20, regions)
            .put_u32(MODEL + 0x48, 2)
            .put(0x17728 + 0x60, &identity)
            .put_u16(ROOT + 0x114, 2 * 0x34)
            .put(ROOT + 0x234, &identity)
            .put_u16(ROOT + 0x118, 20)
            .put_u16(ROOT + 0x280, 0)
            .put_u8(OBJECT_SLOTS + 3, 0)
            .put_u16(ROOT + 0x112, 0x500)
            .put_u16(ROOT + 0x33e, 0x600)
            .put_u16(actor + 0x36, u16::MAX)
            .put_u16(actor + 0x5e, u16::MAX)
            .put_u16(ROOT + 0x12a, 0x700)
            .put_u32(animation, 1)
            .put_u32(animation + 0x68, 1)
            .put_u16(ROOT + 0x148, 0x8000)
            .put_u16(ROOT + 0x1f6, 0x0100)
            .put_u8(palette::UNIT_NODE_ADJUSTMENTS_ENABLED, 0);
        for (i, x) in [(0, -3.0), (1, 5.0)] {
            let region = regions + i * 16;
            let perm = permutations + i * 16;
            let section = SECTIONS + i * 0x5c;
            m.put_u32(region + 8, 1)
                .put_u32(region + 12, perm)
                .put_u16(perm + 14, i as u16)
                .put_u16(section + 0x18, 1)
                .put_u16(section + 0x2c, i as u16)
                .put_f32(ROOT + 0x200 + i * 0x34 + 40, x);
        }
        let (_, _, sections) = display_model(&m, ROOT_HANDLE, LodDetail::Retail, None).unwrap();
        assert_eq!(sections.len(), 2);
        for (section, (index, x)) in sections.iter().zip([(0, -3.), (1, 5.)]) {
            assert_eq!(section.section, index);
            assert_eq!(section.rows, [[1., 0., 0., x], [0., 1., 0., 0.], [0., 0., 1., 0.]]);
        }

        let mut submitted = m.clone();
        let out = setup(&submitted, &cpu, LodDetail::Retail).unwrap();
        publish(&mut submitted, out);
        assert_eq!(submitted.u32(MAIN_COUNT).unwrap(), 2);
        let read = crate::native_world::Mem(&mut submitted);
        for (i, section) in sections.iter().enumerate() {
            let record = RENDER_RECORDS + i as u32 * RENDER_RECORD_SIZE;
            assert_eq!(native_render::object_nodes::node_rows(&read, record).unwrap(), section.rows);
        }

        m.put_u8(palette::UNIT_NODE_ADJUSTMENTS_ENABLED, 1);
        assert!(matches!(setup(&m, &cpu, LodDetail::Retail), Err(Refusal::Palette)));
        assert!(display_model(&m, ROOT_HANDLE, LodDetail::Retail, None).is_none());
        assert_eq!(m.u32(RECORD_COUNT).unwrap(), 0);
        assert_eq!(m.u32(NODE_BLOCKS - 4).unwrap(), 0);
        assert_eq!(m.u32(CACHE_ENTRIES + 0x4c).unwrap(), NONE);
    }

    #[test]
    fn setup_emits_parent_then_child_and_publishes_palette_and_cache() {
        let (mut mem, cpu) = fixture();
        let out = setup(&mem, &cpu, LodDetail::Retail).unwrap();
        assert_eq!(out.al, Some(0));
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 0);
        publish(&mut mem, out);
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 2);
        assert_eq!(mem.u32(MAIN_COUNT).unwrap(), 2);
        let record = |index: u32| RENDER_RECORDS + index * RENDER_RECORD_SIZE;
        let section = |index| (mem.u32(record(index) + 0xc).unwrap() >> 9) & 0x1ff;
        assert_eq!([section(0), section(1)], [1, 0]);
        let bytes = BLOCK_MATRICES + MATRIX_STRIDE;
        assert_eq!(mem.u32(NODE_BLOCKS - 4).unwrap(), 2 * bytes);
        assert_eq!(mem.u32(record(0) + 8).unwrap(), 0);
        assert_eq!(mem.u32(record(1) + 8).unwrap(), bytes);
        assert_eq!(mem.u16(NODE_BLOCKS).unwrap(), 1);
        assert_eq!(mem.f32(NODE_BLOCKS + BLOCK_MATRICES).unwrap(), 1.0);
        for i in 0..2 {
            let entry = CACHE_ENTRIES + i * 0x100;
            assert_eq!(mem.u32(entry + 0xc).unwrap(), 23);
            assert_eq!(mem.u8(entry + 0xbc).unwrap(), 5);
            assert_eq!(mem.u8(entry + 0xc0).unwrap(), (1 - i) as u8);
            assert_eq!(mem.u32(RESOURCE_SLOTS + i * 0x18 + 0x14).unwrap(), 73);
        }
    }

    #[test]
    fn object_and_child_choose_detail_by_distance_at_output_resolution() {
        let run = |scale, pose_mask| {
            let (mut mem, cpu) = fixture();
            mem.put_u32(ROOT + 4, 1 << 24)
                .put_u32(CHILD + 4, 1 << 24)
                .put_u32(HLMT + 0xc4, pose_mask)
                .put_u8(FORCE_FULL_LOD, 1)
                .put_f32(0x0044_ae90, 1.0) // override's local LOD multiplier
                .put_f32(0x0044_ae88, 0.1) // hysteresis
                .put_f32(0x0045_dc78, 1.0)
                .put_u32(0x004b_a01c, 5)
                .put_f32(records::PIXEL_QUANTIZE, 0.5)
                .put_f32(CAMERA, -100.0);
            for (i, threshold) in [55.0, 35.0, 20.0, 15.0, 5.0, 0.0].into_iter().enumerate() {
                mem.put_f32(HLMT + 0x34 + 4 * i as u32, threshold);
            }
            let lod = if scale == 1.0 { LodDetail::Retail } else { LodDetail::Scaled(scale) };
            let output = setup(&mem, &cpu, lod).unwrap();
            publish(&mut mem, output);
            assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 2);
            let lods = [0, 1].map(|i| mem.u8(CACHE_ENTRIES + 0x100 * i + 0xbc).unwrap());
            let bytes = [0, 1].map(|i| mem.u8(0x004c_0b84 + i).unwrap());
            (lods, bytes)
        };
        assert_eq!(run(1.0, 0), ([0; 2], [5; 2]));
        assert_eq!(run(4.0, 0), ([2; 2], [20; 2]));
        assert_eq!(run(4.0 * 1.5, 0), ([3; 2], [30; 2]));
        assert_eq!(run(4.0, 1), ([0; 2], [20; 2]));
    }

    #[test]
    fn setup_retries_the_prior_lod_when_the_requested_section_is_unavailable() {
        let (mut mem, cpu) = fixture();
        let resource = SECTIONS + 0x5c + 0x38;
        mem.put_u8(resource + 0x1e, 0)
            .put_u32(resource + 8, 0)
            .put_u8(CACHE_ENTRIES + 0xbc, 4)
            .put_u8(CACHE_ENTRIES + 0xc0, 1);
        let out = setup(&mem, &cpu, LodDetail::Retail).unwrap();
        publish(&mut mem, out);
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 2);
        assert_eq!((mem.u32(RENDER_RECORDS + 0xc).unwrap() >> 9) & 0x1ff, 0);
        assert_eq!(mem.u8(CACHE_ENTRIES + 0xbc).unwrap(), 4);
        assert_eq!(mem.u32(CACHE_ENTRIES + 0x4c).unwrap(), NONE);
        assert_eq!(mem.u32(CACHE_ENTRIES + 0xc).unwrap(), 23);
    }

    #[test]
    fn incomplete_child_discards_the_parent_plan() {
        let (mut mem, cpu) = fixture();
        let child_resource = SECTIONS + 0x38;
        mem.put_u8(child_resource + 0x1e, 0)
            .put_u32(child_resource + 8, 16)
            .put_u32(child_resource + 0x20, NONE);
        assert!(matches!(setup(&mem, &cpu, LodDetail::Retail), Err(Refusal::Residency)));
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 0);
        assert_eq!(mem.u32(NODE_BLOCKS - 4).unwrap(), 0);
        assert_eq!(mem.u32(CACHE_ENTRIES + 0xc).unwrap(), 0);
    }

    #[test]
    fn first_person_emits_its_slot_and_leaves_the_caller_a_retry() {
        let (mut mem, mut cpu) = fixture();
        let first = sections::FIRST_PERSON_MODELS;
        let pose = mem.bytes(ROOT + 0x200, 0x34).unwrap();
        mem.put_u32(sections::FIRST_PERSON_OWNER, ROOT_HANDLE)
            .put_u16(sections::FIRST_PERSON_COUNT, 1)
            .put_u32(first, 0x4141_0003)
            .put_u32(first + 4, ROOT_HANDLE)
            .put_u32(first + 8, 1)
            .put(first + 0xc, &pose)
            .put_u32(0x004b_9ed8, NONE)
            .put_u32(LIGHT_VIS + 0x14, LIST)
            .put_u32(LIGHT_VIS + VIEW_KIND, 1)
            .put_u32(CURRENT_VIS, LIGHT_VIS);
        cpu.esi = LIGHT_VIS;
        mem.put_u8(CACHE_ENTRIES + 0xbc, 0)
            .put_u32(CACHE_ENTRIES + 0x4c, 0)
            .put_u32(CACHE_ENTRIES + 0x50, 23)
            .put_u16(0x17510 + 4, 1);
        let out = setup(&mem, &cpu, LodDetail::Highest).unwrap();
        assert_eq!(out.al, Some(1));
        publish(&mut mem, out);
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 1);
        assert_eq!(mem.u32(MAIN_COUNT).unwrap(), 0);
        assert_eq!(mem.u32(RENDER_RECORDS).unwrap() & 0xc00, 0xc00);
        assert_eq!((mem.u32(RENDER_RECORDS + 0xc).unwrap() >> 9) & 0x1ff, 0);

        mem.put_u32(STACK + 8, 1);
        let retry = setup(&mem, &cpu, LodDetail::Retail).unwrap();
        assert_eq!(retry.al, Some(0));
        publish(&mut mem, retry);
        assert_eq!(mem.u32(RECORD_COUNT).unwrap(), 3);
        assert_eq!(
            mem.u32(RENDER_RECORDS + RENDER_RECORD_SIZE).unwrap() & 0xc06,
            4
        );
    }

    /// A call the setup cannot build is the guest's, never the "not drawn"
    /// exit: the first person's arms whose light sample is due (the guest
    /// samples it, and the weapon copies it), and a model in a menu's view
    /// (Choose Profile's Spartan). Taking the exit for them lost the arms,
    /// left the weapon black on a light never sampled, and lost the
    /// Spartan.
    #[test]
    fn a_setup_the_build_refuses_runs_the_guest_body_and_is_not_dropped() {
        let (mut mem, mut cpu) = fixture();
        let first = sections::FIRST_PERSON_MODELS;
        let pose = mem.bytes(ROOT + 0x200, 0x34).unwrap();
        mem.put_u32(sections::FIRST_PERSON_OWNER, ROOT_HANDLE)
            .put_u16(sections::FIRST_PERSON_COUNT, 1)
            .put_u32(first, 0x4141_0003)
            .put_u32(first + 4, ROOT_HANDLE)
            .put_u32(first + 8, 1)
            .put(first + 0xc, &pose)
            .put_u32(0x004b_9ed8, NONE)
            .put_u16(0x17510 + 4, 1)
            // The arms' owner is the root and takes its own sample, which
            // it has never taken (not the default lighting, no BSP light).
            .put_u32(ROOT + 4, 1 << 19)
            .put_u16(ROOT + 0xd0, u16::MAX)
            .put_u32(CACHE_ENTRIES + 8, NONE);
        // The sample period's pixel thresholds.
        mem.map(0x0044_5000, 0x1000).map(0x0045_e000, 0x1000);
        let flags = mem.u16(FLAGS).unwrap();
        let built = setup(&mem, &cpu, LodDetail::Highest).map(|o| o.al);
        assert!(matches!(built, Err(Refusal::Lighting)), "{built:?}");
        let (_, call) = super::super::setup_call(&mem, &cpu, LodDetail::Highest);
        assert!(matches!(call, Err(Refusal::Lighting)), "the guest samples the light and builds the records");

        // A menu's model view is not one the build takes.
        let ui_view = 0x0004_2000;
        mem.map(ui_view, 0x3000)
            .put_u32(ui_view + 0x14, LIST)
            .put_u32(ui_view + VIEW_KIND, 3)
            .put_u32(sections::FIRST_PERSON_OWNER, NONE);
        cpu.esi = ui_view;
        let (_, call) = super::super::setup_call(&mem, &cpu, LodDetail::Retail);
        assert!(matches!(call, Err(Refusal::Unsupported)), "the guest builds the menu's model");
        assert_eq!(mem.u16(FLAGS).unwrap(), flags, "no row is marked not drawn");
    }

    #[test]
    fn zero_opacity_marks_only_the_low_visibility_byte_and_returns_retry() {
        let (mut mem, mut cpu) = fixture();
        mem.put_u32(ROOT + 4, 0)
            .put_u32(DEFINITION + 0x38, NONE)
            .put_u32(sections::FIRST_PERSON_OWNER, ROOT_HANDLE)
            .put_u16(FLAGS, 0x84a0)
            .put_u32(LIGHT_VIS + 0x14, LIST)
            .put_u32(LIGHT_VIS + VIEW_KIND, 1);
        cpu.esi = LIGHT_VIS;
        let out = setup(&mem, &cpu, LodDetail::Retail).unwrap();
        assert_eq!(out.al, Some(1));
        assert_eq!((out.ecx, out.edx), (Some(FLAGS), Some(FLAGS)));
        assert_eq!(mem.u16(FLAGS).unwrap(), 0x84a0);
        publish(&mut mem, out);
        assert_eq!(mem.u16(FLAGS).unwrap(), 0x84a1);

        mem.put_u32(STACK + 8, 1);
        assert_eq!(setup(&mem, &cpu, LodDetail::Retail).unwrap().al, Some(0));
    }
}
