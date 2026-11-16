//! Synthetic UI tags only: every byte here is invented for the test.
use super::*;

const BASE: u32 = 0x8000_0000;

/// A flat metadata image with a tag table and a string-ID table.
struct Mem {
    bytes: Vec<u8>,
    groups: HashMap<TagId, FourCC>,
    /// Payload address per tag, for [`Source::address`].
    addresses: HashMap<TagId, u32>,
    /// Optional owner spans (address, size, tag) reported by `read`.
    owners: Vec<(u32, u32, TagId)>,
    strings: Vec<Option<String>>,
    /// Overrides the budget (default: the image size).
    budget: Option<u64>,
}
impl Mem {
    fn new() -> Self {
        let strings = vec![
            Some(String::new()),
            Some("label".into()),
            Some("title".into()),
        ];
        Self {
            bytes: Vec::new(),
            groups: HashMap::new(),
            addresses: HashMap::new(),
            owners: Vec::new(),
            strings,
            budget: None,
        }
    }
    /// Append zeroed space and return its address (4-byte aligned).
    fn alloc(&mut self, size: usize) -> u32 {
        let at = self.bytes.len();
        self.bytes.resize(at + size.next_multiple_of(4), 0);
        BASE + at as u32
    }
    fn put(&mut self, address: u32, offset: usize, data: &[u8]) {
        let o = (address - BASE) as usize + offset;
        self.bytes[o..o + data.len()].copy_from_slice(data);
    }
    fn u16(&mut self, a: u32, o: usize, v: u16) {
        self.put(a, o, &v.to_le_bytes());
    }
    fn u32(&mut self, a: u32, o: usize, v: u32) {
        self.put(a, o, &v.to_le_bytes());
    }
    fn f32(&mut self, a: u32, o: usize, v: f32) {
        self.put(a, o, &v.to_le_bytes());
    }
    fn block(&mut self, a: u32, o: usize, count: u32, address: u32) {
        self.u32(a, o, count);
        self.u32(a, o + 4, address);
    }
    fn tag(&mut self, group: FourCC, slot: u32) -> TagId {
        let id = TagId(0xe000_0000 | (slot << 16) | slot);
        self.groups.insert(id, group);
        id
    }
    fn tag_ref(&mut self, a: u32, o: usize, group: FourCC, id: Option<TagId>) {
        self.u32(a, o, group.as_u32());
        self.u32(a, o + 4, id.map_or(u32::MAX, |i| i.0));
    }
}
impl Source for Mem {
    fn read(&mut self, address: u32, size: u32) -> Result<Bytes> {
        let bounds = Error::Bounds {
            address,
            size: size.into(),
        };
        let start = address.checked_sub(BASE).ok_or(bounds)? as usize;
        let owner = self
            .owners
            .iter()
            .find(|&&(a, n, _)| (a..a + n).contains(&address))
            .map(|o| o.2);
        // Test images are small; copying the image per read is fine here.
        Bytes::new(self.bytes.clone().into(), start, size as usize, owner).ok_or(Error::Bounds {
            address,
            size: size.into(),
        })
    }
    fn group(&self, id: TagId) -> Option<FourCC> {
        self.groups.get(&id).copied()
    }
    fn address(&self, id: TagId) -> Option<u32> {
        self.addresses.get(&id).copied()
    }
    fn budget(&self) -> u64 {
        self.budget.unwrap_or(self.bytes.len() as u64)
    }
    fn strings(&self) -> &[Option<String>] {
        &self.strings
    }
}
/// String ID for entry `index` of `Mem::strings`.
fn sid(m: &Mem, index: usize) -> u32 {
    let len = m.strings[index].as_ref().unwrap().len() as u32;
    (len << 24) | index as u32
}

/// A screen with one pane holding one of each common widget kind.
fn sample() -> (Mem, u32) {
    let mut m = Mem::new();
    let unic = m.tag(STRINGS, 1);
    let bitm = m.tag(BITMAP, 2);
    let root = m.alloc(Screen::SIZE);
    m.u32(root, 0, 4);
    m.u16(root, 4, 6);
    m.tag_ref(root, 0x18, STRINGS, Some(unic));
    let label = sid(&m, 1);
    let title = sid(&m, 2);
    m.u32(root, 0x2c, title);
    let pane = m.alloc(0x4c);
    m.block(root, 0x20, 1, pane);
    m.u16(pane, 2, 30);

    let lst = m.alloc(0x18);
    m.block(pane, 0xc, 1, lst);
    m.u32(lst, 0, 1);
    m.u16(lst, 4, 11);
    m.u16(lst, 6, 6);
    m.u16(lst, 8, (-178i16) as u16);
    m.u16(lst, 0xa, (-65i16) as u16);
    m.u16(lst, 0xc, 35);

    let bmp = m.alloc(0x38 * 2);
    m.block(pane, 0x24, 2, bmp);
    m.u16(bmp, 4, 30);
    m.u16(bmp, 0xc, (-255i16) as u16);
    m.u16(bmp, 0xe, 45);
    m.f32(bmp, 0x10, 0.01);
    m.tag_ref(bmp, 0x18, BITMAP, Some(bitm));
    m.u16(bmp, 0x20, 3);
    let meter = bmp + 0x38;
    m.u32(meter, 0, BitmapBlock::METER);
    m.u16(meter, 0xc, 10);
    m.u16(meter, 0xe, 20);
    m.u16(meter, 0x28, 110);
    m.u16(meter, 0x2a, 5);
    m.tag_ref(meter, 0x18, BITMAP, None);

    let txt = m.alloc(0x2c);
    m.block(pane, 0x1c, 1, txt);
    m.u32(txt, 0, 2);
    m.u16(txt, 0xa, 1);
    m.f32(txt, 0x10, 1.0);
    for (i, v) in [100i16, -50, 80, 50].iter().enumerate() {
        m.u16(txt, 0x1c + i * 2, *v as u16);
    }
    m.u32(txt, 0x24, label);
    m.u16(txt, 0x28, 2);

    let btn = m.alloc(0x3c);
    m.block(pane, 4, 1, btn);
    m.u16(btn, 4, 5);
    m.tag_ref(btn, 0x24, BITMAP, Some(bitm));
    m.u16(btn, 0x2e, (-5i16) as u16);
    m.u32(btn, 0x30, label);
    m.u32(btn, 0x38, 3);
    (m, root)
}

fn screen_of(d: Decoded) -> Screen {
    match d.tag {
        UiTag::Screen(s) => s,
        other => panic!("expected a screen, got {other:?}"),
    }
}

#[test]
fn screen_tree_decodes() {
    let (mut m, root) = sample();
    let d = decode(&mut m, SCREEN, root).unwrap();
    assert_eq!(d.stats.nonempty_blocks, 5);
    let s = screen_of(d);
    assert_eq!((s.flags, s.screen_id), (4, 6));
    assert_eq!(s.strings.group, STRINGS);
    assert_eq!(s.header.0 & 0xffffff, 2);
    let p = &s.panes[0];
    assert_eq!(p.animation.index(), Some(29));
    let l = &p.lists[0];
    assert_eq!(
        (l.skin, l.visible_items, l.anchor),
        (11, 6, Point { x: -178, y: -65 })
    );
    assert_eq!(l.animation.index(), Some(34));
    let b = &p.bitmaps[0];
    assert_eq!(b.top_left, Point { x: -255, y: 45 });
    assert_eq!(b.scroll_rate, [0.01, 0.0]);
    assert_eq!(b.render_depth_bias, 3);
    assert!(b.bitmap.id.is_some());
    // Y is up: the bottom edge is top minus the image height.
    assert_eq!(
        b.rect(Some((64, 32))),
        Rect {
            top: 45,
            left: -255,
            bottom: 13,
            right: -191
        }
    );
    assert_eq!(b.rect(None), Rect::default());
    let meter = &p.bitmaps[1];
    assert_eq!(meter.bitmap.id, None);
    assert_eq!(
        meter.rect(Some((64, 32))),
        Rect {
            top: 20,
            left: 10,
            bottom: 5,
            right: 10
        }
    );
    let t = &p.text_blocks[0];
    assert_eq!(
        t.rect,
        Rect {
            top: 100,
            left: -50,
            bottom: 80,
            right: 50
        }
    );
    assert_eq!((t.rect.width(), t.rect.height()), (100, 20));
    assert_eq!(justification(t.flags), 1);
    assert_eq!(
        (t.font, t.render_depth_bias, t.text_color),
        (1, 2, [1.0, 0.0, 0.0])
    );
    let btn = &p.buttons[0];
    assert_eq!(btn.bitmap_offset, Point { x: 0, y: -5 });
    assert_eq!((btn.flags_38, justification(btn.flags)), (3, 2));
    assert_eq!(btn.string, t.string);
}

#[test]
fn header_layout_slots_follow_flag_priority() {
    let (mut m, root) = sample();
    let mut s = screen_of(decode(&mut m, SCREEN, root).unwrap());
    for (flags, slot) in [(0, 0), (0x10, 1), (0x18, 2), (0x19, 3), (4, 0)] {
        s.flags = flags;
        assert_eq!(s.header_layout(), slot, "flags {flags:#x}");
    }
}

fn field_of(e: &Error) -> Vec<&'static str> {
    let mut out = Vec::new();
    let mut e = e;
    while let Error::Field { field, source, .. } = e {
        out.push(*field);
        e = source;
    }
    out
}

#[test]
fn reference_group_mismatch_is_reported_with_context() {
    let (mut m, root) = sample();
    let shader = m.tag(SHADER, 9);
    let pane = u32::from_le_bytes(m.bytes[0x24..0x28].try_into().unwrap());
    let bmp = u32::from_le_bytes(
        m.bytes[(pane - BASE) as usize + 0x28..][..4]
            .try_into()
            .unwrap(),
    );
    // Claims to be a bitmap but the datum is a shader tag.
    m.tag_ref(bmp, 0x18, BITMAP, Some(shader));
    let e = decode(&mut m, SCREEN, root).unwrap_err();
    assert_eq!(field_of(&e), ["panes", "bitmaps", "bitmap"]);
    // A correct group pairing with the wrong expected class is also refused.
    m.tag_ref(bmp, 0x18, SHADER, Some(shader));
    let e = decode(&mut m, SCREEN, root).unwrap_err();
    assert!(matches!(e, Error::Field { .. }));
    // An unresolvable datum is refused rather than treated as null.
    m.tag_ref(bmp, 0x18, BITMAP, Some(TagId(0xdead_0001)));
    assert!(decode(&mut m, SCREEN, root).is_err());
}

#[test]
fn populated_unreviewed_block_is_refused() {
    let (mut m, root) = sample();
    let pane = u32::from_le_bytes(m.bytes[0x24..0x28].try_into().unwrap());
    m.block(pane, 0x14, 1, root);
    let e = decode(&mut m, SCREEN, root).unwrap_err();
    assert_eq!(field_of(&e), ["panes", "unknown_14"]);
    let Error::Field { source, .. } = &e else {
        unreachable!()
    };
    let Error::Field { source, .. } = &**source else {
        unreachable!()
    };
    assert!(matches!(**source, Error::UnsupportedBlock("unknown_14")));
}

#[test]
fn block_bounds_and_alignment_are_checked() {
    let (mut m, root) = sample();
    let end = BASE + m.bytes.len() as u32;
    m.block(root, 0x20, 1, end - 8);
    assert!(matches!(
        field_of(&decode(&mut m, SCREEN, root).unwrap_err())[..],
        ["panes"]
    ));
    m.block(root, 0x20, 1, root + 2);
    assert!(decode(&mut m, SCREEN, root).is_err());
    // A count whose byte size overflows u32 is a bounds error, not a wrap.
    m.block(root, 0x20, u32::MAX, root);
    assert!(decode(&mut m, SCREEN, root).is_err());
}

#[test]
fn string_ids_must_match_the_table() {
    let (mut m, root) = sample();
    m.u32(root, 0x2c, (9 << 24) | 2);
    assert_eq!(
        field_of(&decode(&mut m, SCREEN, root).unwrap_err()),
        ["header"]
    );
    m.u32(root, 0x2c, u32::MAX);
    assert!(decode(&mut m, SCREEN, root).is_ok());
}

#[test]
fn ui_globals_and_screen_list() {
    let mut m = Mem::new();
    let wigl = m.tag(SHARED_GLOBALS, 1);
    let wgit = m.tag(SCREEN, 2);
    let root = m.alloc(UiGlobals::SIZE);
    let list = m.alloc(16);
    m.tag_ref(root, 0, SHARED_GLOBALS, Some(wigl));
    m.block(root, 8, 2, list);
    m.tag_ref(list, 0, SCREEN, Some(wgit));
    m.tag_ref(list, 8, SCREEN, None);
    m.tag_ref(root, 0x10, VARIANT_SETTINGS, None);
    m.tag_ref(root, 0x18, STRINGS, None);
    let UiTag::UiGlobals(g) = decode(&mut m, UI_GLOBALS, root).unwrap().tag else {
        panic!()
    };
    assert_eq!(g.shared.id, Some(wigl));
    assert_eq!(
        g.screens.iter().map(|r| r.id).collect::<Vec<_>>(),
        [Some(wgit), None]
    );
    // A non-screen entry in the screen list is refused.
    m.tag_ref(list, 0, SHARED_GLOBALS, Some(wigl));
    assert!(decode(&mut m, UI_GLOBALS, root).is_err());
}

fn null_refs(m: &mut Mem, root: u32, offsets: &[usize]) {
    for &o in offsets {
        m.tag_ref(root, o, FourCC::from_u32(u32::MAX), None);
    }
}

#[test]
fn shared_globals_animations_skins_and_layouts() {
    let mut m = Mem::new();
    let skin_id = m.tag(SKIN, 3);
    let root = m.alloc(SharedGlobals::SIZE);
    let mut refs: Vec<usize> = (0..16).map(|i| 0x90 + i * 8).collect();
    refs.extend([0x110, 0x118, 0x140, 0x148, 0x150, 0x1b8]);
    null_refs(&mut m, root, &refs);
    let anim = m.alloc(0x2c);
    let keys = m.alloc(0x14 * 2);
    m.block(root, 0x120, 1, anim);
    m.u32(anim, 4, 300);
    m.block(anim, 8, 2, keys);
    m.f32(keys, 4, 0.0);
    m.f32(keys, 0x14 + 4, 1.0);
    m.f32(keys, 0x14 + 8, -20.0);
    m.u16(anim, 0x20, 2);
    let skins = m.alloc(8);
    m.block(root, 0x138, 1, skins);
    m.tag_ref(skins, 0, SKIN, Some(skin_id));
    m.u16(root, 0x160 + 2, 7);
    m.u16(root, 0x178 + 16, 240);
    m.u16(root, 0x178 + 16 + 8, 10);
    let UiTag::SharedGlobals(g) = decode(&mut m, SHARED_GLOBALS, root).unwrap().tag else {
        panic!()
    };
    let a = &g.animations[0];
    assert_eq!(a.primary.period_ms, 300);
    assert_eq!(a.primary.keyframes[1].alpha, 1.0);
    assert_eq!(a.primary.keyframes[1].offset, [-20.0, 0.0, 0.0]);
    assert_eq!((a.ambient_loop, a.secondary.keyframes.len()), (2, 0));
    assert_eq!(g.skins[0].id, Some(skin_id));
    assert_eq!(g.header_fonts[1], 7);
    assert_eq!(g.header_layouts[1].header.top, 240);
    assert_eq!(g.header_layouts[1].secondary.top, 10);
    // A sound slot pointing at a non-sound tag is refused.
    m.tag_ref(root, 0x90, SKIN, Some(skin_id));
    assert_eq!(
        field_of(&decode(&mut m, SHARED_GLOBALS, root).unwrap_err()),
        ["sounds"]
    );
}

#[test]
fn skin_and_table_layout() {
    let mut m = Mem::new();
    let skin_id = m.tag(SKIN, 4);
    let root = m.alloc(Skin::SIZE);
    m.tag_ref(root, 4, BITMAP, None);
    let tbl = m.alloc(0x18);
    m.block(root, 0x34, 1, tbl);
    m.tag_ref(tbl, 4, SKIN, Some(skin_id));
    m.u16(tbl, 0xc, 100);
    m.u16(tbl, 0xe, 50);
    m.put(tbl, 0x10, &[0, 6, 3, 2]);
    m.u16(tbl, 0x14, (-20i16) as u16);
    m.u16(tbl, 0x16, 30);
    let items = m.alloc(0x10);
    m.block(root, 0x14, 1, items);
    m.u32(items, 4, 120);
    let UiTag::Skin(s) = decode(&mut m, SKIN, root).unwrap().tag else {
        panic!()
    };
    assert_eq!(s.item_animations[0].animation.period_ms, 120);
    let t = &s.tables[0];
    assert_eq!(t.cells, 6);
    // order 0: index runs down `rows` first.
    assert_eq!(t.cell_origin(0), Some(Point { x: 100, y: 50 }));
    assert_eq!(t.cell_origin(1), Some(Point { x: 130, y: 50 }));
    assert_eq!(t.cell_origin(2), Some(Point { x: 100, y: 30 }));
    let ordered = Table {
        order: 1,
        ..t.clone()
    };
    assert_eq!(ordered.cell_origin(1), Some(Point { x: 100, y: 30 }));
    assert_eq!(ordered.cell_origin(3), Some(Point { x: 130, y: 50 }));
}

#[test]
fn model_scene_names_and_viewport() {
    let mut m = Mem::new();
    let root = m.alloc(0x18);
    let scene = m.alloc(0x4c);
    let names = m.alloc(0x20);
    m.block(root, 8, 1, scene);
    m.block(scene, 0xc, 1, names);
    m.put(names, 0, b"some_object");
    m.f32(scene, 0x34, 50.0);
    for (i, v) in [240i16, -320, -240, 320].iter().enumerate() {
        m.u16(scene, 0x38 + i * 2, *v as u16);
    }
    let mut c = Ctx::new(&mut m);
    let bytes = c.root(root, 0x18).unwrap();
    let g = shape_group(&mut c, &bytes).unwrap();
    let s = &g.model_scenes[0];
    assert_eq!(s.objects[0].as_str(), "some_object");
    assert_eq!((s.viewport.width(), s.viewport.height()), (640, 480));
    assert_eq!(s.fov, 50.0);
}

#[test]
fn unsupported_group_and_animation_index() {
    let mut m = Mem::new();
    assert!(matches!(
        decode(&mut m, BITMAP, BASE),
        Err(Error::UnsupportedGroup(_))
    ));
    assert_eq!(AnimationRef(0).index(), None);
    assert_eq!(AnimationRef(1).index(), Some(0));
    assert!(supports(SKIN) && !supports(STRINGS));
}

/// Innermost error under any `Field` context.
fn root_cause(e: &Error) -> &Error {
    match e {
        Error::Field { source, .. } => root_cause(source),
        e => e,
    }
}

/// A screen whose `panes` panes all point at one shared button array of
/// `buttons` buttons: the aliasing shape that multiplies decoding work.
fn aliased_screen(panes: u32, buttons: u32) -> (Mem, u32) {
    let mut m = Mem::new();
    let root = m.alloc(Screen::SIZE);
    m.tag_ref(root, 0x18, STRINGS, None);
    let btn = m.alloc(0x3c * buttons as usize);
    for i in 0..buttons as usize {
        m.tag_ref(btn, i * 0x3c + 0x24, BITMAP, None);
    }
    let pane = m.alloc(0x4c * panes as usize);
    m.block(root, 0x20, panes, pane);
    for i in 0..panes as usize {
        m.block(pane, i * 0x4c + 4, buttons, btn);
    }
    (m, root)
}

#[test]
fn aliased_blocks_are_charged_every_time_and_budgeted() {
    // Two panes sharing one array: decodes, and the array is charged twice.
    let (mut m, root) = aliased_screen(2, 3);
    // The image is smaller than the repeated reads; give it room.
    m.budget = Some(10_000);
    let d = decode(&mut m, SCREEN, root).unwrap();
    let s = screen_of(d.clone());
    assert_eq!((s.panes.len(), s.panes[1].buttons.len()), (2, 3));
    let expected = Screen::SIZE as u64 + 2 * 0x4c + 2 * 3 * 0x3c;
    assert_eq!(d.stats.bytes, expected);

    // 200 panes x 100 buttons would follow 1.2 MB out of a ~21 KB image.
    let (mut m, root) = aliased_screen(200, 100);
    let e = decode(&mut m, SCREEN, root).unwrap_err();
    assert_eq!(field_of(&e), ["panes", "buttons"]);
    let budget = m.budget();
    assert!(matches!(
        root_cause(&e),
        Error::Budget { kind: "bytes", limit } if *limit == budget
    ));

    // The same image decodes once the budget covers the multiplied work,
    // and a budget below the root alone is refused before any block.
    let (mut m, root) = aliased_screen(200, 100);
    m.budget = Some(2_000_000);
    assert!(decode(&mut m, SCREEN, root).is_ok());
    m.budget = Some(Screen::SIZE as u64 - 1);
    assert!(matches!(
        decode(&mut m, SCREEN, root),
        Err(Error::Budget { kind: "bytes", .. })
    ));
}

#[test]
fn block_count_past_the_budget_is_refused_before_reading() {
    let (mut m, root) = sample();
    // 0x10000 panes fit in u32 bytes but not in the image.
    m.block(root, 0x20, 0x1_0000, root);
    let e = decode(&mut m, SCREEN, root).unwrap_err();
    assert!(matches!(root_cause(&e), Error::Budget { .. }), "{e}");
}

#[test]
fn nested_payload_spans_resolve_to_a_containing_span() {
    let (a, b) = (TagId(1), TagId(2));
    // B nests inside A.
    let spans = [(0x100, 0x100, a), (0x120, 0x10, b)];
    let max_end = [0x200, 0x200];
    let find = |addr, size| span_containing(&spans, &max_end, addr, size).map(|s| s.2);
    assert_eq!(find(0x122, 4), Some(b));
    // Starts inside B but runs past it: only A holds the whole range.
    assert_eq!(find(0x12c, 0x20), Some(a));
    assert_eq!(find(0x180, 4), Some(a));
    assert_eq!(find(0x1f0, 0x20), None);
    assert_eq!(find(0x80, 4), None);
}

#[test]
fn aliases_are_counted_with_their_target_group() {
    let (mut m, root) = aliased_screen(1, 2);
    let (screen_id, other) = (m.tag(SCREEN, 1), m.tag(SCREEN, 2));
    let btn = root + Screen::SIZE as u32;
    let pane = btn + 2 * 0x3c;
    m.owners = vec![
        (root, Screen::SIZE as u32, screen_id),
        (btn, 2 * 0x3c, other),
        (pane, 0x4c, screen_id),
    ];
    let d = decode(&mut m, SCREEN, root).unwrap();
    assert_eq!((d.stats.aliased_blocks, d.stats.foreign_aliases), (1, 0));
    // The same alias into a bitmap tag is counted as foreign.
    m.groups.insert(other, BITMAP);
    let d = decode(&mut m, SCREEN, root).unwrap();
    assert_eq!((d.stats.aliased_blocks, d.stats.foreign_aliases), (1, 1));
}

#[test]
fn string_list_errors_carry_field_context() {
    let (mut m, root) = sample();
    let list = m.alloc(0xc);
    let ids = m.alloc(8);
    m.block(root, 0x30, 1, list);
    m.block(list, 4, 2, ids);
    m.u32(ids, 0, sid(&m, 1));
    m.u32(ids, 4, (7 << 24) | 1);
    let e = decode(&mut m, SCREEN, root).unwrap_err();
    assert_eq!(field_of(&e), ["string_lists", "strings", "string"]);
    m.u32(ids, 4, sid(&m, 2));
    let s = screen_of(decode(&mut m, SCREEN, root).unwrap());
    assert_eq!(s.string_lists[0].strings.len(), 2);
}

#[test]
fn shared_globals_accessors_check_ranges() {
    let skin = TagRef {
        group: SKIN,
        id: Some(TagId(5)),
    };
    let anim = Animation {
        period_ms: 0,
        keyframes: Vec::new(),
    };
    let g = SharedGlobals {
        unknown_000: Vec::new(),
        text_parameter: 0.0,
        meter_color: [0.0; 4],
        unknown_060: [0; 0x28],
        unknown_088: Block::default(),
        sounds: [skin; 16],
        bitmap: skin,
        strings: skin,
        animations: vec![AnimationCollection {
            flags: 0,
            primary: anim.clone(),
            secondary: anim.clone(),
            ambient: anim,
            ambient_loop: 0,
            unknown_22: 0,
        }],
        shape_groups: vec![ShapeGroup {
            unknown_00: [0; 8],
            model_scenes: Vec::new(),
            bitmaps: Vec::new(),
        }],
        unknown_130: Block::default(),
        skins: vec![skin],
        strings_140: skin,
        strings_148: skin,
        reference_150: skin,
        unknown_158: Block::default(),
        header_fonts: [0; 4],
        header_color: [0.0; 4],
        header_layouts: [HeaderLayout::default(); 4],
        looping_sound: skin,
        unknown_1c0: 0,
    };
    assert!(g.animation(AnimationRef(1)).is_some());
    assert!(g.animation(AnimationRef(0)).is_none());
    assert!(g.animation(AnimationRef(2)).is_none());
    assert_eq!(g.skin(0), Some(skin));
    assert_eq!(g.skin(1), None);
    assert_eq!(g.skin(-1), None);
    assert!(g.shape_group(1).is_some());
    assert!(g.shape_group(0).is_none() && g.shape_group(2).is_none());
}

/// A skin tag whose single table names `next` (or no table if `None`).
fn skin_tag(m: &mut Mem, slot: u32, next: Option<TagId>) -> TagId {
    let id = m.tag(SKIN, slot);
    let root = m.alloc(Skin::SIZE);
    m.addresses.insert(id, root);
    m.tag_ref(root, 4, BITMAP, None);
    if next.is_some() {
        let tbl = m.alloc(0x18);
        m.block(root, 0x34, 1, tbl);
        m.tag_ref(tbl, 4, SKIN, next);
        m.put(tbl, 0x10, &[0, 1, 1, 1]);
    }
    id
}

#[test]
fn skin_nesting_resolves_and_is_depth_limited() {
    let mut m = Mem::new();
    let leaf = skin_tag(&mut m, 1, None);
    let mid = skin_tag(&mut m, 2, Some(leaf));
    let (tree, stats) = resolve_skin(&mut m, mid).unwrap();
    assert_eq!(tree.depth(), 2);
    assert_eq!(tree.tables[0].as_ref().map(|t| t.id), Some(leaf));
    assert_eq!(stats.bytes, 2 * Skin::SIZE as u64 + 0x18);

    // A skin whose table names itself recurses until the depth limit.
    let id = TagId(0xe000_0000 | (3 << 16) | 3);
    let cyclic = skin_tag(&mut m, 3, Some(id));
    // Room for every level, so the depth limit (not the budget) is what stops it.
    m.budget = Some(1 << 20);
    assert_eq!(cyclic, id);
    assert!(matches!(
        resolve_skin(&mut m, cyclic),
        Err(Error::Invalid("skin nesting exceeds MAX_SKIN_DEPTH"))
    ));
    // A chain exactly MAX_SKIN_DEPTH deep is accepted.
    let mut next = None;
    for slot in 10..10 + MAX_SKIN_DEPTH as u32 {
        next = Some(skin_tag(&mut m, slot, next));
    }
    let (deep, _) = resolve_skin(&mut m, next.unwrap()).unwrap();
    assert_eq!(deep.depth(), MAX_SKIN_DEPTH);
}

#[test]
fn table_cells_follow_the_engine_grid_and_never_overflow() {
    let t = Table {
        flags: 0,
        skin: TagRef {
            group: SKIN,
            id: None,
        },
        anchor: Point { x: 30_000, y: 0 },
        order: 0,
        cells: 4,
        columns: 2,
        rows: 2,
        row_spacing: 10,
        column_spacing: 3_000,
    };
    assert_eq!(t.cell_grid(1), Some((1, 0)));
    assert_eq!(t.cell_origin(0), Some(Point { x: 30_000, y: 0 }));
    // 30000 + 3000 does not fit in i16: refused instead of wrapping.
    assert_eq!(t.cell_origin(1), None);
    // Cells past rows x columns are skipped by the engine.
    assert_eq!(t.cell_grid(4), None);
    assert_eq!(t.cell_grid(-1), None);
    // Zero counts clamp to one, as 0x002afdbc does.
    let flat = Table {
        rows: 0,
        columns: 0,
        anchor: Point::default(),
        ..t.clone()
    };
    assert_eq!(flat.cell_grid(0), Some((0, 0)));
    assert_eq!(flat.cell_grid(1), None);
    // Extreme spacings and counts never panic.
    let extreme = Table {
        row_spacing: i16::MAX,
        column_spacing: i16::MIN,
        rows: i8::MAX,
        columns: i8::MAX,
        order: 1,
        ..t
    };
    for i in 0..i32::from(i8::MAX) * 3 {
        let _ = extreme.cell_origin(i);
    }
}

/// The crate's synthetic one-tag map, with its tag's group set to `group`.
fn one_tag_map(group: Option<FourCC>) -> map::Map<std::io::Cursor<Vec<u8>>> {
    let mut b = crate::tests::bitmap_map();
    if let Some(g) = group {
        // Tag-index entry group and the class entry it must match.
        b[0x102c..0x1030].copy_from_slice(&g.as_u32().to_le_bytes());
        b[0x1020..0x1024].copy_from_slice(&g.as_u32().to_le_bytes());
    }
    map::Map::from_reader(std::io::Cursor::new(b)).unwrap()
}

#[test]
fn map_check_requires_ui_tags_and_one_wigl() {
    let c = check::check_map(&mut one_tag_map(None));
    assert_eq!(c.failures, ["map has no UI tags"]);

    // One skin and no wigl: the skin is still decoded, the map still fails.
    let c = check::check_map(&mut one_tag_map(Some(SKIN)));
    assert!(
        c.failures
            .iter()
            .any(|f| f == "map has 0 wigl tags, expected exactly 1"),
        "{:?}",
        c.failures
    );
    assert_eq!(c.groups[&SKIN].tags, 1);

    // A lone wigl that does not decode is reported, not skipped.
    let c = check::check_map(&mut one_tag_map(Some(SHARED_GLOBALS)));
    assert!(
        c.failures.iter().any(|f| f.contains("wigl")),
        "{:?}",
        c.failures
    );
}

#[test]
fn map_source_is_built_once_and_decodes_many_tags() {
    let mut m = one_tag_map(Some(SKIN));
    let id = m.tags()[0].id;
    let mut src = MapSource::new(&mut m);
    assert_eq!(src.overlapping_spans(), 0);
    let first = src.decode_tag(id).map(|d| d.stats);
    let second = src.decode_tag(id).map(|d| d.stats);
    assert_eq!(first.is_ok(), second.is_ok());
    assert_eq!(src.owner(src.address(id).unwrap()), Some(id));
}

#[test]
fn acyclic_skin_chain_one_past_the_limit_is_refused() {
    // The bound is part of the documented contract (docs/formats/ui.md).
    assert_eq!(MAX_SKIN_DEPTH, 4);
    let mut m = Mem::new();
    let mut next = None;
    for slot in 1..=5 {
        next = Some(skin_tag(&mut m, slot, next));
    }
    // Ample budget: only the depth limit can refuse this chain.
    m.budget = Some(1 << 20);
    assert!(matches!(
        resolve_skin(&mut m, next.unwrap()),
        Err(Error::Invalid("skin nesting exceeds MAX_SKIN_DEPTH"))
    ));
}

#[test]
fn skin_tree_shares_one_budget() {
    let mut m = Mem::new();
    let leaf = skin_tag(&mut m, 1, None);
    let mid = skin_tag(&mut m, 2, Some(leaf));
    let top = skin_tag(&mut m, 3, Some(mid));
    // Each skin alone needs at most root + one table (0x3c + 0x18 = 84 bytes);
    // the tree needs 84 + 84 + 60 = 228.
    let whole = 2 * (Skin::SIZE as u64 + 0x18) + Skin::SIZE as u64;
    m.budget = Some(whole);
    let (_, stats) = resolve_skin(&mut m, top).unwrap();
    assert_eq!(stats.bytes, whole);
    m.budget = Some(whole - 1);
    assert!(matches!(
        resolve_skin(&mut m, top),
        Err(Error::Budget { kind: "bytes", .. })
    ));
    // Every single skin still fits that budget on its own.
    for id in [leaf, mid, top] {
        let at = m.addresses[&id];
        assert!(decode(&mut m, SKIN, at).is_ok());
    }
}

/// A synthetic map holding `count` valid, empty `wigl` tags (1 or 2).
fn wigl_map(count: u32) -> map::Map<std::io::Cursor<Vec<u8>>> {
    let mut b = crate::tests::bitmap_map();
    let put = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let wigl = SHARED_GLOBALS.as_u32();
    put(&mut b, 0x1020, wigl);
    put(&mut b, 0x1018, count);
    // Tag entries: group, datum, address, size. Payloads at file 0x1200 and
    // 0x1400 (virtual 0x81000000 and 0x81000200).
    for i in 0..count as usize {
        let e = 0x102c + i * 16;
        put(&mut b, e, wigl);
        put(&mut b, e + 4, 0xa000_0000 + (i as u32) * 0x0001_0001);
        put(&mut b, e + 8, 0x8100_0000 + (i as u32) * 0x200);
        put(&mut b, e + 12, 0x200);
    }
    b[0x1200..0x1600].fill(0);
    for i in 0..count as usize {
        let root = 0x1200 + i * 0x200;
        let refs = (0..16)
            .map(|s| 0x90 + s * 8)
            .chain([0x110, 0x118, 0x140, 0x148, 0x150, 0x1b8]);
        for o in refs {
            put(&mut b, root + o, u32::MAX);
            put(&mut b, root + o + 4, u32::MAX);
        }
    }
    map::Map::from_reader(std::io::Cursor::new(b)).unwrap()
}

#[test]
fn two_valid_wigl_tags_fail_the_map_check() {
    // Control: one valid wigl and nothing else passes.
    let c = check::check_map(&mut wigl_map(1));
    assert_eq!(c.failures, Vec::<String>::new());
    assert_eq!(c.groups[&SHARED_GLOBALS].tags, 1);
    // Two wigl tags that each decode cleanly: the count alone must fail it.
    let c = check::check_map(&mut wigl_map(2));
    assert_eq!(c.groups[&SHARED_GLOBALS].tags, 2);
    assert_eq!(c.groups[&SHARED_GLOBALS].failures, 0);
    assert_eq!(c.failures, ["map has 2 wigl tags, expected exactly 1"]);
}
