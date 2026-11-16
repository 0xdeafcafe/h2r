//! Where the last frame drew each labelled widget and window, for the
//! headless screenshots' scripted input, crops and callouts: Dear ImGui's
//! test-engine hooks (`ItemAdd` gives an item's rectangle and window,
//! `ItemInfo` its label), switched on per frame by [`begin`]. The real
//! window never calls [`begin`], so the hooks stay idle there.
//!
//! Some widgets (combos) never report a label. For those, [`want`] names
//! labels to look for: an added item whose id is that label hashed on its
//! window's ID stack is recorded under it.
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};

use dear_imgui_rs::sys;

/// A labelled widget the last frame drew: its full label (with any `##`
/// id), the name of the window it is in, and its rectangle in points
/// (`[x0, y0, x1, y1]`).
#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub window: String,
    pub rect: [f32; 4],
}

/// A window the last frame drew.
#[derive(Clone, Debug)]
pub struct Window {
    pub name: String,
    pub rect: [f32; 4],
}

#[derive(Default)]
struct Frame {
    added: HashMap<u32, ([f32; 4], String)>,
    items: Vec<Item>,
}

thread_local! {
    static FRAME: RefCell<Frame> = RefCell::default();
    static WANTED: RefCell<Vec<CString>> = const { RefCell::new(Vec::new()) };
}

/// Labels to find even on widgets that never report one.
pub fn want(labels: Vec<String>) {
    let mut labels: Vec<CString> = labels
        .into_iter()
        .filter_map(|l| CString::new(l).ok())
        .collect();
    labels.sort();
    labels.dedup();
    WANTED.with(|w| *w.borrow_mut() = labels);
}

type ItemAddFn =
    unsafe extern "C" fn(*mut sys::ImGuiContext, u32, *const sys::ImRect, *const c_void);
type ItemInfoFn = unsafe extern "C" fn(*mut sys::ImGuiContext, u32, *const c_char, i32);

unsafe extern "C" {
    // dear-imgui-sys's test-engine shim (the `test-engine` feature).
    fn dear_imgui_rs_set_test_engine_hooks(
        item_add: Option<ItemAddFn>,
        item_info: Option<ItemInfoFn>,
        log_v: Option<unsafe extern "C" fn()>,
        find_item_debug_label: Option<unsafe extern "C" fn()>,
    );
}

unsafe fn name_of(w: *const sys::ImGuiWindow) -> String {
    if w.is_null() || unsafe { (*w).Name.is_null() } {
        return String::new();
    }
    unsafe { CStr::from_ptr((*w).Name) }
        .to_string_lossy()
        .into_owned()
}

unsafe extern "C" fn item_add(
    ctx: *mut sys::ImGuiContext,
    id: u32,
    bb: *const sys::ImRect,
    _data: *const c_void,
) {
    if ctx.is_null() || bb.is_null() || id == 0 {
        return;
    }
    let w = unsafe { (*ctx).CurrentWindow };
    if w.is_null() {
        return;
    }
    let (b, window) = unsafe { (*bb, name_of(w)) };
    let rect = [b.Min.x, b.Min.y, b.Max.x, b.Max.y];
    // The seed the widget hashed its label on: the window's ID stack top.
    let seed = unsafe {
        let stack = &(*w).IDStack;
        (stack.Size > 0).then(|| *stack.Data.add(stack.Size as usize - 1))
    };
    let named: Vec<String> = match seed {
        Some(seed) => WANTED.with(|wanted| {
            wanted
                .borrow()
                .iter()
                .filter(|l| unsafe { sys::igImHashStr(l.as_ptr(), 0, seed) } == id)
                .map(|l| l.to_string_lossy().into_owned())
                .collect()
        }),
        None => Vec::new(),
    };
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        for label in named {
            f.items.push(Item {
                label,
                window: window.clone(),
                rect,
            });
        }
        f.added.insert(id, (rect, window));
    });
}

unsafe extern "C" fn item_info(
    _ctx: *mut sys::ImGuiContext,
    id: u32,
    label: *const c_char,
    _: i32,
) {
    if label.is_null() {
        return;
    }
    let label = unsafe { CStr::from_ptr(label) }
        .to_string_lossy()
        .into_owned();
    FRAME.with(|f| {
        let mut f = f.borrow_mut();
        if let Some((rect, window)) = f.added.get(&id).cloned()
            && !f.items.iter().any(|i| i.rect == rect && i.label == label)
        {
            f.items.push(Item {
                label,
                window,
                rect,
            });
        }
    });
}

/// Starts recording the frame Dear ImGui is building (call after
/// `NewFrame`, which switches the hooks off).
pub fn begin() {
    unsafe {
        dear_imgui_rs_set_test_engine_hooks(Some(item_add), Some(item_info), None, None);
        let ctx = sys::igGetCurrentContext();
        if !ctx.is_null() {
            (*ctx).TestEngineHookItems = true;
        }
    }
    FRAME.with(|f| *f.borrow_mut() = Frame::default());
}

/// The labelled widgets recorded since [`begin`], in submission order.
pub fn items() -> Vec<Item> {
    FRAME.with(|f| f.borrow().items.clone())
}

/// The windows drawn this frame (after it is built), front-most last.
pub fn windows() -> Vec<Window> {
    let mut out = Vec::new();
    unsafe {
        let ctx = sys::igGetCurrentContext();
        if ctx.is_null() {
            return out;
        }
        let list = &(*ctx).Windows;
        for i in 0..list.Size.max(0) as usize {
            let w = *list.Data.add(i);
            if w.is_null() || !(*w).Active || (*w).Hidden {
                continue;
            }
            let (p, s) = ((*w).Pos, (*w).Size);
            out.push(Window {
                name: name_of(w),
                rect: [p.x, p.y, p.x + s.x, p.y + s.y],
            });
        }
    }
    out
}
