//! Read the live model widget: the names it selects and its view's camera.
use crate::light_constants::Sse;

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

fn definition(word: &impl Fn(u32) -> Option<u32>, widget: u32) -> Option<u32> {
    let def = word(widget.checked_add(WIDGET_DEFINITION)?)?;
    (def != 0).then_some(def)
}

/// The byte at `at`, through aligned words.
fn byte(word: &impl Fn(u32) -> Option<u32>, at: u32) -> Option<u8> {
    Some((word(at & !3)? >> ((at & 3) * 8)) as u8)
}

/// The little-endian word at `at`, aligned or not.
fn word_at(word: &impl Fn(u32) -> Option<u32>, at: u32) -> Option<u32> {
    if at & 3 == 0 {
        return word(at);
    }
    let mut bytes = [0; 4];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = byte(word, at.checked_add(i as u32)?)?;
    }
    Some(u32::from_le_bytes(bytes))
}

fn half(word: &impl Fn(u32) -> Option<u32>, at: u32) -> Option<u16> {
    Some(u16::from_le_bytes([byte(word, at)?, byte(word, at.checked_add(1)?)?]))
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

fn bounds(word: &impl Fn(u32) -> Option<u32>, at: u32) -> Option<[i32; 4]> {
    let mut out = [0; 4];
    for (i, edge) in out.iter_mut().enumerate() {
        *edge = i32::from(half(word, at.checked_add(i as u32 * 2)?)? as i16);
    }
    Some(out)
}

/// The camera words of `widget`'s view against `parent`, the parent
/// view's `top, left, bottom, right`, its SSE arithmetic under `mxcsr`;
/// `word` reads the guest's aligned words.
pub fn camera(word: impl Fn(u32) -> Option<u32>, widget: u32, parent: u32, mxcsr: u32) -> Option<Camera> {
    let word = &word;
    let f32_at = |at: u32| word_at(word, at).map(f32::from_bits);
    let def = definition(word, widget)?;
    let [top, left, bottom, right] = bounds(word, parent)?;
    let (height, width) = (bottom - top, right - left);
    if !(1..=i32::from(i16::MAX)).contains(&width)
        || !(1..=i32::from(i16::MAX)).contains(&height)
    {
        return None;
    }
    let [tag_top, tag_left, tag_bottom, tag_right] = bounds(word, def.checked_add(CAMERA_BOUNDS)?)?;
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

    let fov = f32_at(def.checked_add(CAMERA_FOV)?)?;
    let min = f32_at(FOV_MIN)?;
    let max = f32_at(FOV_MAX)?;
    if ![fov, min, max].iter().all(|v| v.is_finite()) || min > max {
        return None;
    }
    let mut sse = Sse { mxcsr };
    let fov = sse.mul(fov.clamp(min, max).to_bits(), word_at(word, FOV_FACTOR)?).ok()?;
    let mut center = [0; 3];
    for (axis, (offset, result)) in [0, 8, 4].into_iter().zip(&mut center).enumerate() {
        let animation = word_at(word, widget.checked_add(WIDGET_ANIMATION + offset)?)?;
        let scale = word_at(word, def.checked_add(CAMERA_SCALE + axis as u32 * 4)?)?;
        let base = word_at(word, def.checked_add(CAMERA_CENTER + axis as u32 * 4)?)?;
        let delta = sse.mul(scale, animation).ok()?;
        *result = sse.sub(base, delta).ok()?;
    }
    if !center.iter().chain([&fov]).all(|&bits| f32::from_bits(bits).is_finite()) {
        return None;
    }
    let planes = [word_at(word, VIEW_NEAR)?, word_at(word, VIEW_FAR)?];
    let [near, far] = planes.map(f32::from_bits);
    if !(near.is_finite() && far.is_finite() && near > 0.0 && far > near) {
        return None;
    }
    Some(Camera { center, bounds: viewport, fov, planes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::GuestMemory;
    use guest_layout::testing::FakeGuest;

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
        let pose = camera(|at| mem.u32(at).ok(), widget, context, 0x1f80).unwrap();
        assert_eq!(pose.center.map(f32::from_bits), [19.0, 33.0, 31.0]);
        assert_eq!(pose.bounds, [150, 111, 170, 161]);
        assert_eq!(f32::from_bits(pose.fov), 2.5);
        assert_eq!(pose.planes.map(f32::from_bits), [0.0625, 200.0]);
        mem.put_f32(VIEW_FAR, 0.03125);
        assert_eq!(camera(|at| mem.u32(at).ok(), widget, context, 0x1f80), None, "a far plane before the near one");
    }
}
