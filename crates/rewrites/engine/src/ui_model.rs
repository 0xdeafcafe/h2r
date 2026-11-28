//! Read the live model widget and the index ranges its model view requests.
use crate::{light_constants::Sse, memory::GuestMemory};

const USER_SELECTION: u32 = 0x0054_d598;
const MODEL_USER: u32 = 4;
const WIDGET_DEFINITION: u32 = 0x70;
const WIDGET_ANIMATION: u32 = 0x58;
const PRIMARY_NAME_COUNT: u32 = 0x0c;
const SECONDARY_NAME_COUNT: u32 = 0x14;
const CAMERA_SCALE: u32 = 0x1c;
const CAMERA_CENTER: u32 = 0x28;
const CAMERA_FOV: u32 = 0x34;
const CAMERA_BOUNDS: u32 = 0x38;
const FOV_MIN: u32 = 0x0045_dbc0;
const FOV_MAX: u32 = 0x0045_e5b0;
const FOV_FACTOR: u32 = 0x0045_e5ac;
/// The near and far planes the model view `0x0002c560` puts in its render
/// camera (`+0x40`, `+0x44`; `0x0002c586`, `0x0002c5da`): its own, not the
/// game view's (`0x00485ad4`).
pub const VIEW_NEAR: u32 = 0x0046_7014;
pub const VIEW_FAR: u32 = 0x0046_7018;

const SECTION_RANGES: u32 = 0x0c;
const SECTION_INDICES: u32 = 0x24;
const GROUP_FIRST_INDEX: u32 = 6;
const GROUP_INDEX_COUNT: u32 = 8;
const GROUP_FIRST_RANGE: u32 = 10;
const GROUP_RANGE_COUNT: u32 = 12;
const RANGE_STRIDE: u32 = 8;
const RANGE_LIMIT: usize = 2560;

/// The guest callback returns without touching the camera, object flags or
/// draw state in these two cases.
pub fn is_idle(mem: &dyn GuestMemory, widget: u32) -> Option<bool> {
    Some(match selection(widget, |at| mem.u32(at).ok())? {
        Selection::Inactive => true,
        Selection::Names { objects, lights, .. } => objects <= 0 && lights <= 0,
    })
}

/// The widget's name blocks before the guest resolves them against the
/// current scenario. The inactive branch does not read the widget.
pub enum Selection {
    Inactive,
    Names { definition: u32, objects: i32, lights: i32 },
}

pub fn selection(widget: u32, word: impl Fn(u32) -> Option<u32>) -> Option<Selection> {
    if word(USER_SELECTION)? != MODEL_USER {
        return Some(Selection::Inactive);
    }
    let definition = word(widget.checked_add(WIDGET_DEFINITION)?)?;
    if definition == 0 {
        return None;
    }
    Some(Selection::Names {
        definition,
        objects: word(definition.checked_add(PRIMARY_NAME_COUNT)?)? as i32,
        lights: word(definition.checked_add(SECONDARY_NAME_COUNT)?)? as i32,
    })
}

fn definition(mem: &dyn GuestMemory, widget: u32) -> Option<u32> {
    let def = mem.u32(widget.checked_add(WIDGET_DEFINITION)?).ok()?;
    (def != 0).then_some(def)
}

/// The camera words handed to the model view. Component order matters:
/// the widget animation's second and third components are exchanged.
#[derive(Debug, PartialEq, Eq)]
pub struct Camera {
    pub center: [u32; 3],
    pub bounds: [u16; 4],
    pub fov: u32,
    /// Near and far ([`VIEW_NEAR`], [`VIEW_FAR`]) as `f32` bits.
    pub planes: [u32; 2],
}

fn bounds(mem: &dyn GuestMemory, at: u32) -> Option<[i32; 4]> {
    let mut out = [0; 4];
    for (i, word) in out.iter_mut().enumerate() {
        *word = i32::from(mem.u16(at.checked_add(i as u32 * 2)?).ok()? as i16);
    }
    Some(out)
}

pub fn camera(mem: &dyn GuestMemory, widget: u32, parent: u32, mxcsr: u32) -> Option<Camera> {
    let def = definition(mem, widget)?;
    let [top, left, bottom, right] = bounds(mem, parent)?;
    let (height, width) = (bottom - top, right - left);
    if !(1..=i32::from(i16::MAX)).contains(&width)
        || !(1..=i32::from(i16::MAX)).contains(&height)
    {
        return None;
    }
    let [tag_top, tag_left, tag_bottom, tag_right] = bounds(mem, def.checked_add(CAMERA_BOUNDS)?)?;
    let split_x = if width == 320 {
        if left == 0 { 20 } else { -20 }
    } else {
        0
    };
    let split_y = match (width == 320, height == 240, top == 0) {
        (false, false, _) => 0,
        (_, _, true) => -16,
        _ => 16,
    };
    let center_x = width / 2 + split_x;
    let center_y = height / 2 + split_y;
    let viewport = [
        center_x + tag_left, center_y - tag_top, center_x + tag_right, center_y - tag_bottom,
    ].map(|v| v as u16);

    let fov = mem.f32(def.checked_add(CAMERA_FOV)?).ok()?;
    let min = mem.f32(FOV_MIN).ok()?;
    let max = mem.f32(FOV_MAX).ok()?;
    if ![fov, min, max].iter().all(|v| v.is_finite()) || min > max {
        return None;
    }
    let mut sse = Sse { mxcsr };
    let fov = sse.mul(fov.clamp(min, max).to_bits(), mem.u32(FOV_FACTOR).ok()?).ok()?;
    let mut center = [0; 3];
    for (axis, (offset, result)) in [0, 8, 4].into_iter().zip(&mut center).enumerate() {
        let animation = mem.u32(widget.checked_add(WIDGET_ANIMATION + offset)?).ok()?;
        let scale = mem.u32(def.checked_add(CAMERA_SCALE + axis as u32 * 4)?).ok()?;
        let base = mem.u32(def.checked_add(CAMERA_CENTER + axis as u32 * 4)?).ok()?;
        let delta = sse.mul(scale, animation).ok()?;
        *result = sse.sub(base, delta).ok()?;
    }
    if !center.iter().chain([&fov]).all(|&bits| f32::from_bits(bits).is_finite()) {
        return None;
    }
    let planes = [mem.u32(VIEW_NEAR).ok()?, mem.u32(VIEW_FAR).ok()?];
    let [near, far] = planes.map(f32::from_bits);
    if !(near.is_finite() && far.is_finite() && near > 0.0 && far > near) {
        return None;
    }
    Some(Camera { center, bounds: viewport, fov, planes })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub indices: u32,
    pub count: u32,
    pub strip: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Inputs,
    RangeLimit,
}

/// Select the same indices as the standard model emitter. The masked
/// triangle-list path merges adjacent selected ranges, even across clear
/// bits. Strips keep each selected range as a separate primitive.
pub fn ranges(
    mem: &dyn GuestMemory,
    section: u32,
    group: u32,
    mask: Option<u32>,
    strip: bool,
) -> Result<Vec<Range>, Refusal> {
    use Refusal::{Inputs, RangeLimit};

    let word = |base: u32, offset: u32| {
        mem.u16(base.checked_add(offset).ok_or(Inputs)?).map(u32::from).map_err(|_| Inputs)
    };
    let addr = |base: u32, offset: u32| {
        let ptr = mem.u32(base.checked_add(offset).ok_or(Inputs)?).map_err(|_| Inputs)?;
        (ptr != 0).then_some(ptr).ok_or(Inputs)
    };
    if section == 0 || group == 0 {
        return Err(Inputs);
    }
    let range = |first: u32, count: u32| {
        if count < 3 || (!strip && !count.is_multiple_of(3)) {
            return Err(Inputs);
        }
        let indices = addr(section, SECTION_INDICES)?.checked_add(first * 2).ok_or(Inputs)?;
        indices.checked_add(count * 2 - 1).ok_or(Inputs)?;
        Ok(Range { indices, count, strip })
    };
    let Some(mask) = mask else {
        return Ok(vec![range(word(group, GROUP_FIRST_INDEX)?, word(group, GROUP_INDEX_COUNT)?)?]);
    };
    if mask == 0 {
        return Err(Inputs);
    }
    let first = word(group, GROUP_FIRST_RANGE)?;
    let count = word(group, GROUP_RANGE_COUNT)?;
    if first > i16::MAX as u32 || count > i16::MAX as u32 {
        return Err(Inputs);
    }
    let mut pending: Option<(u32, u32)> = None;
    let mut ranges = Vec::new();
    for i in first..first + count {
        let mask_word = mem.u32(mask.checked_add(i / 32 * 4).ok_or(Inputs)?).map_err(|_| Inputs)?;
        if mask_word & (1 << (i % 32)) == 0 {
            continue;
        }
        let at = addr(section, SECTION_RANGES)?.checked_add(i * RANGE_STRIDE).ok_or(Inputs)?;
        let index = word(at, 0)?;
        let len = word(at, 2)?;
        if len == 0 {
            return Err(Inputs);
        }
        if let Some((base, total)) = pending {
            if !strip && base + total == index {
                pending = Some((base, total + len));
                continue;
            }
            ranges.push(range(base, total)?);
            if ranges.len() == RANGE_LIMIT {
                return Err(RangeLimit);
            }
        }
        pending = Some((index, len));
    }
    if let Some((base, total)) = pending {
        ranges.push(range(base, total)?);
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeGuest;

    #[test]
    fn inactive_widget_and_empty_names_are_idle() {
        let mut mem = FakeGuest::new();
        let widget = 0x1000;
        let def = 0x2000;
        mem.put_u32(USER_SELECTION, 1);
        assert_eq!(is_idle(&mem, widget), Some(true));
        mem.put_u32(USER_SELECTION, MODEL_USER);
        assert_eq!(is_idle(&mem, widget), None);
        mem.put_u32(widget + WIDGET_DEFINITION, def)
            .put_u32(def + PRIMARY_NAME_COUNT, 0)
            .put_u32(def + SECONDARY_NAME_COUNT, 0);
        assert_eq!(is_idle(&mem, widget), Some(true));
        mem.put_u32(def + SECONDARY_NAME_COUNT, 1);
        assert_eq!(is_idle(&mem, widget), Some(false));
    }

    #[test]
    fn camera_uses_live_animation_axis_order_and_split_context() {
        let (widget, def, context) = (0x1000, 0x2000, 0x3000);
        let mut mem = FakeGuest::new();
        mem.put_u32(widget + WIDGET_DEFINITION, def)
            .put_f32(FOV_MIN, 1.0)
            .put_f32(FOV_MAX, 10.0)
            .put_f32(FOV_FACTOR, 0.25)
            .put_f32(VIEW_NEAR, 0.0625)
            .put_f32(VIEW_FAR, 200.0)
            .put_f32(def + CAMERA_FOV, 15.0);
        for (i, (base, scale, anim)) in [(25.0, 2.0, 3.0), (40.0, 1.0, 5.0), (51.0, 4.0, 7.0)]
            .into_iter().enumerate()
        {
            let offset = i as u32 * 4;
            mem.put_f32(def + CAMERA_SCALE + offset, scale)
                .put_f32(def + CAMERA_CENTER + offset, base)
                .put_f32(widget + WIDGET_ANIMATION + offset, anim);
        }
        for (i, (outer, inner)) in [240, 320, 480, 640].into_iter().zip([25, 10, -25, 30]).enumerate() {
            mem.put_u16(context + i as u32 * 2, outer as u16)
                .put_u16(def + CAMERA_BOUNDS + i as u32 * 2, inner as u16);
        }
        let pose = camera(&mem, widget, context, 0x1f80).unwrap();
        assert_eq!(pose.center.map(f32::from_bits), [19.0, 33.0, 31.0]);
        assert_eq!(pose.bounds, [150, 111, 170, 161]);
        assert_eq!(f32::from_bits(pose.fov), 2.5);
        assert_eq!(pose.planes.map(f32::from_bits), [0.0625, 200.0]);
        mem.put_f32(VIEW_FAR, 0.03125);
        assert_eq!(camera(&mem, widget, context, 0x1f80), None, "a far plane before the near one");
    }

    #[test]
    fn ranges_follow_the_mask_and_keep_strips_separate() {
        let (section, group, mask, table, indices) = (0x1000, 0x2000, 0x3000, 0x4000, 0x5000);
        let mut mem = FakeGuest::new();
        mem.put_u32(section + SECTION_RANGES, table)
            .put_u32(section + SECTION_INDICES, indices)
            .put_u16(group + GROUP_FIRST_INDEX, 3)
            .put_u16(group + GROUP_INDEX_COUNT, 6)
            .put_u16(group + GROUP_FIRST_RANGE, 31)
            .put_u16(group + GROUP_RANGE_COUNT, 3)
            .put_u32(mask, 1 << 31)
            .put_u32(mask + 4, 2)
            .put_u16(table + 31 * RANGE_STRIDE, 90)
            .put_u16(table + 31 * RANGE_STRIDE + 2, 3)
            .put_u16(table + 33 * RANGE_STRIDE, 93)
            .put_u16(table + 33 * RANGE_STRIDE + 2, 6);
        assert_eq!(ranges(&mem, section, group, None, false), Ok(vec![Range {
            indices: indices + 6, count: 6, strip: false,
        }]));
        assert_eq!(ranges(&mem, section, group, Some(mask), false), Ok(vec![Range {
            indices: indices + 180, count: 9, strip: false,
        }]));
        assert_eq!(ranges(&mem, section, group, Some(mask), true), Ok(vec![
            Range { indices: indices + 180, count: 3, strip: true },
            Range { indices: indices + 186, count: 6, strip: true },
        ]));
        mem.put_u16(table + 33 * RANGE_STRIDE, 80);
        assert_eq!(ranges(&mem, section, group, Some(mask), false), Ok(vec![
            Range { indices: indices + 180, count: 3, strip: false },
            Range { indices: indices + 160, count: 6, strip: false },
        ]));
        mem.put_u32(mask, 0).put_u32(mask + 4, 0);
        assert_eq!(ranges(&mem, section, group, Some(mask), true), Ok(Vec::new()));
    }
}
