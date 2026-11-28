//! A model widget (type 7, `0x002b0baa`) at the sorted draw: the objects
//! its names select and the view its camera call `0x0002c560` would take,
//! read in Rust (docs/graphics/native-menus-overlay.md, "Model views";
//! the dated note `docs/graphics/notes/2026-11-24-menu-model-selection.md`).
//! The overlay's model view stage draws it; the game's draw does not run.
use scene_extract::{
    GuestRead,
    ui::{UiContent, UiModel, Unread},
    visibility::view::LensInputs,
};

use super::Gap;

const SCENARIO: u32 = 0x004e_0350;
const OBJECT_HANDLES: u32 = 0x004d_e2d0;

/// The MXCSR the camera's SSE arithmetic runs under: the default, as the
/// sorted draw holds no other.
const MXCSR: u32 = 0x1f80;

fn word(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u32, Unread> {
    base.checked_add(offset)
        .and_then(|p| mem.read_u32(p))
        .ok_or(Unread::Unreadable)
}

fn pointer(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u32, Unread> {
    let ptr = word(mem, base, offset)?;
    (ptr != 0).then_some(ptr).ok_or(Unread::Unreadable)
}

// Widget names occupy 32 bytes; scenario names occupy the first 32 of 36.
fn name(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<Vec<u8>, Gap> {
    let p = base.checked_add(offset).ok_or(Unread::Unreadable)?;
    let mut bytes = Vec::new();
    for at in (0..32).step_by(4) {
        for byte in word(mem, p, at)?.to_le_bytes() {
            if byte == 0 {
                return Ok(bytes);
            }
            bytes.push(byte);
        }
    }
    unimplemented!("ui model: a name of 32 bytes with no terminator")
}

/// The scenario objects a model widget's names select, as its draw
/// resolves them: each non-empty name's first equal row of the scenario's
/// object names (the resolver takes rows below 640), and that row's handle
/// when it is not the absent datum, in name order. `Ok(None)`: the
/// selector is inactive or the widget names no object, so no view draws.
pub(in crate::native_hud) fn objects(mem: &dyn GuestRead, widget: u32) -> Result<Option<Vec<u32>>, Gap> {
    let selection =
        engine::ui_model::selection(widget, |at| mem.read_u32(at)).ok_or(Unread::Unreadable)?;
    let engine::ui_model::Selection::Names {
        definition: def,
        objects: count,
        lights,
    } = selection
    else {
        return Ok(None);
    };
    if lights > 0 {
        unimplemented!("ui model: light names ({lights}): the view's lights are not read");
    }
    if count > 32 {
        unimplemented!("ui model: {count} object names, past the draw's 32");
    }
    if count <= 0 {
        return Ok(None);
    }
    let names = pointer(mem, def, 0x10)?;
    let mut live = Vec::new();
    for index in 0..count as u32 {
        let key = name(mem, names, index * 32)?;
        if key.is_empty() {
            continue;
        }
        let scenario = pointer(mem, SCENARIO, 0)?;
        let rows = word(mem, scenario, 0x48)? as i32;
        if rows > i32::from(i16::MAX) {
            unimplemented!("ui model: {rows} scenario object names, past an i16");
        }
        if rows <= 0 {
            continue;
        }
        let table = pointer(mem, scenario, 0x4c)?;
        // Only the first equal name counts, including when its handle is
        // the absent datum.
        for row in 0..rows.min(640) as u32 {
            if name(mem, table, row * 36)? == key {
                let handles = pointer(mem, OBJECT_HANDLES, 0)?;
                let handle = word(mem, handles, row * 4)?;
                if handle != u32::MAX {
                    live.push(handle);
                }
                break;
            }
        }
    }
    Ok(Some(live))
}

/// The view `widget` draws against `parent`, the parent view's `top,
/// left, bottom, right` (the sorted draw's argument): its live objects,
/// the camera words of `engine::ui_model::camera` and the projection its
/// camera call builds ([`UiModel::with_lens`]). `None`: no live object.
pub(in crate::native_hud) fn view(mem: &dyn GuestRead, widget: u32, parent: u32) -> Result<Option<UiModel>, Gap> {
    let Some(live) = objects(mem, widget)?.filter(|live| !live.is_empty()) else {
        return Ok(None);
    };
    let Some(camera) = engine::ui_model::camera(|at| mem.read_u32(at), widget, parent, MXCSR) else {
        unimplemented!("ui model: a camera that does not read (widget {widget:#x})");
    };
    let lens = LensInputs::read(mem).unwrap_or_else(|why| unimplemented!("ui model: the lens does not read ({why:?})"));
    Ok(Some(UiModel::new(live, camera.center, camera.bounds, camera.fov, camera.planes).with_lens(&lens)))
}

/// At the sorted list: a widget that selects no live object draws nothing
/// ([`UiContent::None`]); one that does is its view ([`UiContent::Model`]),
/// which the overlay draws in place of the game's draw.
pub(super) fn read(mem: &dyn GuestRead, widget: u32, parent: u32) -> Result<UiContent, Gap> {
    Ok(view(mem, widget, parent)?.map_or(UiContent::None, UiContent::Model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Words(HashMap<u32, u32>);
    impl GuestRead for Words {
        fn read_u32(&self, address: u32) -> Option<u32> {
            self.0.get(&address).copied()
        }
    }

    const PARENT: u32 = 0x7000;

    /// The selector, names and scenario of a widget at `0x1000` whose one
    /// name `A` is the scenario's second row; its camera and the lens.
    fn widget() -> Words {
        let (def, names, scenario, table, handles) = (0x2000, 0x3000, 0x4000, 0x5000, 0x6000);
        let mut mem = Words(HashMap::from([
            (0x0054_d598, 4),
            (0x1070, def),
            (def + 0x0c, 1),
            (def + 0x10, names),
            (def + 0x14, 0),
            (names, 65),
            (SCENARIO, scenario),
            (scenario + 0x48, 2),
            (scenario + 0x4c, table),
            (table, 66),
            (table + 36, 65),
            (OBJECT_HANDLES, handles),
            (handles, 7),
            (handles + 4, 42),
        ]));
        let f = |v: f32| v.to_bits();
        // The camera: FOV 1.5 clamped to 0.5..2.5 and halved, the centre
        // 0, 1, 2 with no animation, and the view's planes.
        mem.0.extend([
            (def + 0x34, f(1.5)),
            (0x0045_dbc0, f(0.5)),
            (0x0045_e5b0, f(2.5)),
            (0x0045_e5ac, f(0.5)),
            (engine::ui_model::VIEW_NEAR, f(0.0625)),
            (engine::ui_model::VIEW_FAR, f(200.0)),
        ]);
        for i in 0..3 {
            mem.0.extend([(0x1058 + 4 * i, f(0.0)), (def + 0x1c + 4 * i, f(1.0)), (def + 0x28 + 4 * i, f(i as f32))]);
        }
        // The parent view 0, 0, 480, 640 and the relative viewport
        // 12, -10, -12, 10, as i16 pairs.
        let pair = |lo: i16, hi: i16| u32::from(lo as u16) | u32::from(hi as u16) << 16;
        mem.0.extend([(PARENT, pair(0, 0)), (PARENT + 4, pair(480, 640)), (def + 0x38, pair(12, -10)), (def + 0x3c, pair(-12, 10))]);
        // The lens `0x0002fb70` reads; the widescreen byte off.
        mem.0.extend([
            (0x0045_dbcc, f(-0.5)),
            (0x0045_dbbc, f(0.5)),
            (0x0045_dbc4, f(1.0)),
            (0x0045_dfdc, f(4.0 / 3.0)),
            (0x0045_e108, f(1.0)),
            (0x0048_5ac0, 0),
        ]);
        mem
    }

    #[test]
    fn a_live_selection_is_its_view_and_none_is_nothing() {
        let mut mem = widget();
        let Ok(UiContent::Model(model)) = read(&mem, 0x1000, PARENT) else { panic!("a view") };
        assert_eq!(model.view.objects, [42]);
        assert_eq!(model.viewport, [310, 228, 330, 252], "about the parent's centre");
        assert_eq!(model.view.camera.position, [0.0, 1.0, 2.0]);
        assert_eq!((model.view.camera.fov_y, model.view.camera.planes), (0.75, [0.0625, 200.0]));
        let lens = LensInputs::read(&mem).unwrap();
        let expected = scene_extract::visibility::view::projection_terms(0.75, [228, 310, 252, 330], &lens);
        assert_eq!(model.view.camera.projection, Some(expected), "the lens its camera call builds");
        mem.0.insert(0x6004, u32::MAX);
        assert!(matches!(read(&mem, 0x1000, PARENT), Ok(UiContent::None)), "an absent datum draws nothing");
        mem.0.insert(0x0054_d598, 0);
        assert!(matches!(read(&mem, 0x1000, PARENT), Ok(UiContent::None)), "an inactive selector");
        mem.0.insert(0x0054_d598, 4);
        mem.0.remove(&0x6000);
        assert_eq!(read(&mem, 0x1000, PARENT), Err(Unread::Unreadable.into()));
    }

    /// Two names, the first resolving to a live object and the second to
    /// a row whose handle is absent: the view holds the one live handle.
    #[test]
    fn the_names_select_their_live_handles_in_order() {
        let mut mem = Words(HashMap::from([
            (0x0054_d598, 4),
            (0x1070, 0x2000),
            (0x200c, 2),
            (0x2010, 0x3000),
            (0x2014, 0),
            (0x3000, 66),
            (0x3020, 65),
            (SCENARIO, 0x4000),
            (0x4048, 2),
            (0x404c, 0x5000),
            (0x5000, 65),
            (0x5024, 66),
            (OBJECT_HANDLES, 0x6000),
            (0x6000, u32::MAX),
            (0x6004, 0xe001_0009),
        ]));
        assert_eq!(objects(&mem, 0x1000), Ok(Some(vec![0xe001_0009])));
        mem.0.insert(0x6000, 0xe002_0004);
        assert_eq!(objects(&mem, 0x1000), Ok(Some(vec![0xe001_0009, 0xe002_0004])), "name order, not row order");
        mem.0.insert(0x0054_d598, 0);
        assert_eq!(objects(&mem, 0x1000), Ok(None), "an inactive selector");
    }

    #[test]
    #[should_panic(expected = "ui model: light names (1)")]
    fn a_widget_with_lights_is_unimplemented() {
        let mut mem = widget();
        mem.0.insert(0x2014, 1);
        let _ = objects(&mem, 0x1000);
    }

    #[test]
    #[should_panic(expected = "ui model: 33 object names, past the draw's 32")]
    fn more_than_32_names_is_unimplemented() {
        let mut mem = widget();
        mem.0.insert(0x200c, 33);
        let _ = objects(&mem, 0x1000);
    }

    #[test]
    #[should_panic(expected = "ui model: a name of 32 bytes with no terminator")]
    fn a_name_with_no_terminator_is_unimplemented() {
        let mut mem = widget();
        for at in (0..32).step_by(4) {
            mem.0.insert(0x3000 + at, 0x4141_4141);
        }
        let _ = objects(&mem, 0x1000);
    }

    #[test]
    #[should_panic(expected = "ui model: 32768 scenario object names, past an i16")]
    fn a_scenario_past_an_i16_is_unimplemented() {
        let mut mem = widget();
        mem.0.insert(0x4048, 32_768);
        let _ = objects(&mem, 0x1000);
    }
}
