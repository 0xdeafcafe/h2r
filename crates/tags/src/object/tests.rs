//! Synthetic object-family tests: every byte below is invented, no game data.
use super::*;

const BASE: u32 = 0x8000_0000;
const SCEN: TagId = TagId(0x1111_0001);
const HLMT: TagId = TagId(0x2222_0002);
const MODE: TagId = TagId(0x3333_0003);
const EFFE: TagId = TagId(0x4444_0004);
const COLL: TagId = TagId(0x6666_0006);
const PHMO: TagId = TagId(0x7777_0007);
const JMAD: TagId = TagId(0x8888_0008);

/// String table: index -> text. A StringId packs length << 24 | index.
const STRINGS: [&str; 7] = ["", "default", "body", "base", "damaged", "marker", "head"];
fn sid(i: u32) -> u32 {
    (STRINGS[i as usize].len() as u32) << 24 | i
}
fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn fourcc(s: &[u8; 4]) -> u32 {
    FourCC(*s).as_u32()
}

/// Metadata image: [scen root][hlmt root][blocks...][effe data]. Returns the
/// bytes and the address of each tag.
struct Image {
    bytes: Vec<u8>,
}
impl Image {
    fn alloc(&mut self, n: usize) -> u32 {
        let at = BASE + self.bytes.len() as u32;
        self.bytes.resize(self.bytes.len() + n, 0);
        at
    }
    fn at(&mut self, addr: u32) -> &mut [u8] {
        &mut self.bytes[(addr - BASE) as usize..]
    }
}

struct Fixture {
    ctx: Context,
    scen: u32,
    hlmt: u32,
    effe: u32,
}

/// One scenery whose hlmt has two regions and two variants ("default",
/// "damaged"); variant 0 maps region 0 -> permutation "base".
fn fixture() -> Fixture {
    let mut img = Image { bytes: Vec::new() };
    let scen = img.alloc(0xc4);
    // Attachment block lives inside the scenery's own payload.
    let attach = img.alloc(0x18);
    let scen_end = img.bytes.len() as u32;
    let hlmt = img.alloc(0xfc);
    let regions = img.alloc(2 * 0x10);
    let region_perms = img.alloc(2 * 8);
    let variants = img.alloc(2 * 0x38);
    let vregions = img.alloc(0x14);
    let vperms = img.alloc(0x20);
    let hlmt_end = BASE + img.bytes.len() as u32;
    let effe = img.alloc(0x40);

    {
        let b = img.at(scen);
        b[0..2].copy_from_slice(&6u16.to_le_bytes()); // scenery type index
        put(b, 4, 2.5f32.to_bits());
        put(b, 0x30, sid(1));
        put(b, 0x34, fourcc(b"hlmt"));
        put(b, 0x38, HLMT.0);
        for o in [0x3c, 0x44, 0x54] {
            put(b, o, u32::MAX);
            put(b, o + 4, u32::MAX);
        }
        // A null reference keeping a class absent from the cache is legal.
        put(b, 0x44, fourcc(b"shdr"));
        put(b, 0x4c, fourcc(b"effe"));
        put(b, 0x50, EFFE.0);
        put(b, 0x94, 1);
        put(b, 0x98, attach);
    }
    {
        let b = img.at(attach);
        put(b, 0, fourcc(b"effe"));
        put(b, 4, EFFE.0);
        put(b, 8, sid(5));
    }
    {
        let b = img.at(hlmt);
        put(b, 0, fourcc(b"mode"));
        put(b, 4, MODE.0);
        for o in [8, 0x10, 0x18, 0x20, 0x8c, 0x94, 0xf0] {
            put(b, o, u32::MAX);
            put(b, o + 4, u32::MAX);
        }
        put(b, 0x50, 2);
        put(b, 0x54, variants);
        put(b, 0x70, 2);
        put(b, 0x74, regions);
    }
    for (i, name) in [2u32, 6].into_iter().enumerate() {
        let r = regions + i as u32 * 0x10;
        let b = img.at(r);
        put(b, 0, sid(name));
        put(b, 8, 1);
        put(b, 0xc, region_perms + i as u32 * 8);
        put(img.at(region_perms + i as u32 * 8), 0, sid(3));
    }
    for (i, name) in [1u32, 4].into_iter().enumerate() {
        let v = variants + i as u32 * 0x38;
        let b = img.at(v);
        put(b, 0, sid(name));
        b[4..20].fill(0xff);
        put(b, 0x30, u32::MAX);
        put(b, 0x34, u32::MAX);
        if i == 0 {
            b[4] = 0; // model region 0 -> variant region 0
            put(b, 0x14, 1);
            put(b, 0x18, vregions);
        }
    }
    {
        let b = img.at(vregions);
        put(b, 0, sid(2));
        b[4] = 0;
        put(b, 8, 1);
        put(b, 0xc, vperms);
        let b = img.at(vperms);
        put(b, 0, sid(3));
        put(b, 8, 1.0f32.to_bits());
    }

    let mut catalog = Catalog {
        strings: STRINGS.iter().map(|s| Some((*s).to_owned())).collect(),
        ..Default::default()
    };
    let mut add = |id: TagId, group: &[u8; 4], loc: Option<(u32, u32)>| {
        catalog.tags.insert(
            id.0,
            CatalogTag {
                id,
                group: FourCC(*group),
                name: None,
                location: loc,
            },
        );
    };
    add(SCEN, b"scen", Some((scen, scen_end)));
    add(HLMT, b"hlmt", Some((hlmt, hlmt_end - hlmt)));
    add(MODE, b"mode", None);
    add(EFFE, b"effe", Some((effe, 0x40)));
    add(COLL, b"coll", None);
    add(PHMO, b"phmo", None);
    add(JMAD, b"jmad", None);
    catalog
        .groups
        .insert(FourCC(*b"scen"), (Some(FourCC(*b"obje")), None));
    let ctx = Context {
        catalog,
        meta: Metadata {
            base: BASE,
            bytes: img.bytes,
        },
    };
    Fixture {
        ctx,
        scen,
        hlmt,
        effe,
    }
}
impl Fixture {
    fn poke(&mut self, addr: u32, v: u32) {
        let at = (addr - BASE) as usize;
        self.ctx.meta.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
}

#[test]
fn resolves_object_model_and_default_variant() {
    let f = fixture();
    let r = f.ctx.resolve_object(SCEN).unwrap();
    assert_eq!(r.header.object_type, 6);
    assert_eq!(r.header.bounding_radius, 2.5);
    assert_eq!(r.header.model.id, HLMT);
    assert_eq!(r.header.creation_effect.id, EFFE);
    assert_eq!(r.header.attachments.len(), 1);
    assert_eq!(r.header.attachments[0].marker.as_deref(), Some("marker"));
    let m = r.model.unwrap();
    assert_eq!(m.render_model.as_ref().map(|l| l.id), Some(MODE));
    assert!(m.collision_model.is_none() && m.physics_model.is_none() && m.animation.is_none());
    assert_eq!(
        m.variant,
        VariantChoice::Named {
            index: 0,
            name: Some("default".into())
        }
    );
    assert_eq!(m.regions.len(), 1);
    assert_eq!(m.regions[0].region.as_deref(), Some("body"));
    assert_eq!(m.regions[0].permutations[0].name.as_deref(), Some("base"));
    assert_eq!(m.regions[0].permutations[0].model_permutation, Some(0));
    // The attachment block lies in the scenery's own payload.
    match r.tag.layers[0].get("attachments") {
        Some(Value::Block { aliased, .. }) => assert!(!aliased),
        v => panic!("{v:?}"),
    }
}

#[test]
fn variant_lookup_follows_the_engine() {
    let mut f = fixture();
    f.poke(f.scen + 0x30, 0);
    let m = f.ctx.resolve_object(SCEN).unwrap().model.unwrap();
    assert!(matches!(m.variant, VariantChoice::First { index: 0, .. }));
    f.poke(f.scen + 0x30, sid(4));
    let m = f.ctx.resolve_object(SCEN).unwrap().model.unwrap();
    assert!(matches!(m.variant, VariantChoice::Named { index: 1, .. }));
    assert!(m.regions.is_empty());
    f.poke(f.scen + 0x30, sid(6)); // "head" names a region, not a variant
    let m = f.ctx.resolve_object(SCEN).unwrap().model.unwrap();
    assert_eq!(m.variant, VariantChoice::Missing(StringId(sid(6))));
}

#[test]
fn object_type_must_match_the_class() {
    let mut f = fixture();
    f.poke(f.scen, 7);
    assert!(f.ctx.decode(SCEN).is_err());
}

#[test]
fn live_references_are_class_checked_and_salted() {
    let mut f = fixture();
    f.poke(f.scen + 0x34, fourcc(b"mode"));
    assert!(f.ctx.decode(SCEN).is_err(), "class not allowed for model");
    let mut f = fixture();
    f.poke(f.scen + 0x38, HLMT.0 ^ 0x0001_0000);
    assert!(f.ctx.decode(SCEN).is_err(), "stale salt");
    let mut f = fixture();
    f.poke(f.scen + 0x4c, fourcc(b"jpt!"));
    assert!(f.ctx.decode(SCEN).is_err(), "referenced tag is an effe");
    let mut f = fixture();
    f.poke(f.scen + 0x44, u32::MAX);
    f.poke(f.scen + 0x48, EFFE.0);
    assert!(f.ctx.decode(SCEN).is_err(), "datum without a class");
}

#[test]
fn string_ids_check_length() {
    let mut f = fixture();
    f.poke(f.scen + 0x30, 3 << 24 | 1);
    assert!(f.ctx.decode(SCEN).is_err());
}

#[test]
fn aliased_blocks_decode_and_are_flagged() {
    let mut f = fixture();
    // Point the scenery's attachments at an element copied into the effe's data.
    let copy: Vec<u8> = f.ctx.meta.slice(f.scen + 0xc4, 0x18).unwrap().to_vec();
    let at = (f.effe - BASE) as usize;
    f.ctx.meta.bytes[at..at + 0x18].copy_from_slice(&copy);
    f.poke(f.scen + 0x98, f.effe);
    let t = f.ctx.decode(SCEN).unwrap();
    match t.layers[0].get("attachments") {
        Some(Value::Block {
            aliased, elements, ..
        }) => assert!(*aliased && elements.len() == 1),
        v => panic!("{v:?}"),
    }
}

#[test]
fn budgets_bound_aliased_amplification() {
    let mut f = fixture();
    // 20 predicted resources all reading the same eight bytes inside the effe.
    let at = (f.effe - BASE) as usize;
    f.ctx.meta.bytes[at + 4..at + 8].copy_from_slice(&EFFE.0.to_le_bytes());
    let fill = f.ctx.meta.bytes[at..at + 8].to_vec();
    for i in 1..8 {
        f.ctx.meta.bytes[at + i * 8..at + i * 8 + 8].copy_from_slice(&fill);
    }
    f.poke(f.scen + 0xb4, 8);
    f.poke(f.scen + 0xb8, f.effe);
    assert_eq!(
        f.ctx.decode(SCEN).unwrap().layers[0]
            .elements("predicted_resources")
            .len(),
        8
    );
    let tight = Budget {
        elements: 4,
        ..Budget::default()
    };
    assert!(f.ctx.decode_with(SCEN, tight).is_err());
    let tight = Budget {
        per_block: 4,
        ..Budget::default()
    };
    assert!(f.ctx.decode_with(SCEN, tight).is_err());
}

#[test]
fn blocks_stay_inside_metadata() {
    let mut f = fixture();
    f.poke(f.scen + 0x94, 4000);
    assert!(f.ctx.decode(SCEN).is_err());
    let mut f = fixture();
    f.poke(f.scen + 0x98, BASE - 0x100);
    assert!(f.ctx.decode(SCEN).is_err());
    let mut f = fixture();
    f.poke(f.scen + 0x98, f.scen + 0xc6);
    assert!(f.ctx.decode(SCEN).is_err(), "misaligned");
    let mut f = fixture();
    // An opaque block is bounds-checked by its address too.
    f.poke(f.scen + 0x5c, 1);
    f.poke(f.scen + 0x60, 0x1000);
    assert!(f.ctx.decode(SCEN).is_err());
}

#[test]
fn model_regions_must_agree_with_variants() {
    let mut f = fixture();
    let vregion = f.hlmt + 0xfc + 0x20 + 0x10 + 0x70;
    f.poke(vregion, sid(6)); // variant region renamed "head" but index 0 is "body"
    assert!(f.ctx.resolve_model(HLMT, StringId(0)).is_err());
    let mut f = fixture();
    let variant0 = f.hlmt + 0xfc + 0x20 + 0x10;
    f.poke(variant0 + 4, 0xffff_ffff); // region map no longer points back
    assert!(f.ctx.resolve_model(HLMT, StringId(0)).is_err());
}

#[test]
fn opaque_bytes_are_kept() {
    let mut f = fixture();
    f.poke(f.scen + 0x14, 0xdead_beef);
    let t = f.ctx.decode(SCEN).unwrap();
    let span = t.layers[0]
        .opaque
        .iter()
        .find(|(o, _)| *o == 0x14)
        .expect("span at 0x14");
    assert_eq!(&span.1[..4], &0xdead_beefu32.to_le_bytes());
    // The scenery layer's own bytes are opaque too.
    assert_eq!(t.layer("scen").unwrap().opaque[0].0, 0xbc);
}

#[test]
fn unsupported_group_and_short_tags() {
    let mut f = fixture();
    assert!(matches!(
        f.ctx.decode(EFFE),
        Err(Error::UnsupportedGroup(_))
    ));
    f.ctx.catalog.tags.get_mut(&SCEN.0).unwrap().location = Some((f.scen, 0x40));
    assert!(matches!(f.ctx.decode(SCEN), Err(Error::RecordSize { .. })));
}

#[test]
fn collections_decode_weighted_entries() {
    let mut img = Image { bytes: Vec::new() };
    let root = img.alloc(0xc);
    let entries = img.alloc(0x20);
    put(img.at(root), 0, 2);
    put(img.at(root), 4, entries);
    for i in 0..2u32 {
        let b = img.at(entries + i * 0x10);
        put(b, 0, (1.0f32 + i as f32).to_bits());
        put(b, 4, fourcc(b"scen"));
        put(b, 8, SCEN.0);
        put(b, 0xc, sid(1));
    }
    let mut catalog = Catalog {
        strings: STRINGS.iter().map(|s| Some((*s).to_owned())).collect(),
        ..Default::default()
    };
    let id = TagId(0x5555_0005);
    catalog.tags.insert(
        id.0,
        CatalogTag {
            id,
            group: FourCC(*b"vehc"),
            name: None,
            location: Some((root, 0x2c)),
        },
    );
    catalog.tags.insert(
        SCEN.0,
        CatalogTag {
            id: SCEN,
            group: FourCC(*b"scen"),
            name: None,
            location: None,
        },
    );
    let ctx = Context {
        catalog,
        meta: Metadata {
            base: BASE,
            bytes: img.bytes,
        },
    };
    let t = ctx.decode(id).unwrap();
    let e = t.layers[0].elements("entries");
    assert_eq!(e.len(), 2);
    assert_eq!(e[1].tag_ref("object").unwrap().id, SCEN);
    assert_eq!(e[1].sid("variant").unwrap().1, Some("default"));
}

/// Layout tables: fields sorted, inside their layer, non-overlapping; layer
/// chains contiguous up to the root size; reviewed strides equal element size.
#[test]
fn layout_tables_are_consistent() {
    fn check(l: &Layout) {
        let mut cursor = l.start;
        for f in l.fields {
            assert!(f.offset >= cursor, "{}.{} overlaps", l.name, f.name);
            cursor = f.offset + f.kind.size();
            assert!(cursor <= l.end, "{}.{} past the end", l.name, f.name);
            if let Kind::Block(stride, e) = f.kind {
                assert_eq!(stride, e.end - e.start, "{}.{}", l.name, f.name);
                assert_eq!(e.start, 0);
                check(e);
            }
            if !matches!(f.evidence, Evidence::Engine) || f.note.is_empty() {
                continue;
            }
            assert!(
                f.note.contains("0x"),
                "{}.{}: engine evidence needs an address",
                l.name,
                f.name
            );
        }
    }
    for g in &GROUPS {
        let mut at = 0;
        for l in g.layers {
            assert_eq!(l.start, at, "{} layer {}", g.group, l.name);
            at = l.end;
            check(l);
        }
        assert_eq!(at, g.root_size, "{}", g.group);
        if let Some(t) = g.object_type {
            assert_eq!(OBJECT_TYPES[t as usize].group, g.group);
        }
    }
    assert_eq!(
        GroupDef::find(FourCC(*b"bipd")).unwrap().ancestry(),
        ["unit", "obje"]
    );
    assert!(
        GroupDef::find(FourCC(*b"obje")).is_none(),
        "obje is abstract"
    );
}

/// Address of variant `i` in the fixture's hlmt.
fn variant(f: &Fixture, i: u32) -> u32 {
    f.hlmt + 0xfc + 0x20 + 0x10 + i * 0x38
}

#[test]
fn element_budget_is_shared_across_blocks() {
    let mut f = fixture();
    // Three predicted resources plus the one attachment: four elements in
    // total, no block larger than three.
    let at = (f.effe - BASE) as usize;
    for i in 0..3 {
        f.ctx.meta.bytes[at + i * 8 + 4..at + i * 8 + 8].copy_from_slice(&EFFE.0.to_le_bytes());
    }
    f.poke(f.scen + 0xb4, 3);
    f.poke(f.scen + 0xb8, f.effe);
    let at_limit = Budget {
        elements: 4,
        ..Budget::default()
    };
    assert!(f.ctx.decode_with(SCEN, at_limit).is_ok());
    let below = Budget {
        elements: 3,
        ..Budget::default()
    };
    assert!(
        f.ctx.decode_with(SCEN, below).is_err(),
        "total over blocks, not per block"
    );
}

#[test]
fn nesting_depth_is_enforced() {
    let f = fixture();
    // hlmt root -> variants (1) -> variant regions (2) -> permutations (3).
    let three = Budget {
        depth: 3,
        ..Budget::default()
    };
    assert!(f.ctx.decode_with(HLMT, three).is_ok());
    let two = Budget {
        depth: 2,
        ..Budget::default()
    };
    assert!(f.ctx.decode_with(HLMT, two).is_err());
}

#[test]
fn duplicate_variant_names_select_the_first() {
    let mut f = fixture();
    let second = variant(&f, 1);
    f.poke(second, sid(1)); // both variants are now "default"
    let m = f.ctx.resolve_model(HLMT, StringId(sid(1))).unwrap();
    assert_eq!(m.variant.index(), Some(0));
    assert_eq!(m.regions.len(), 1, "regions come from the first variant");
}

#[test]
fn model_references_are_held_to_their_classes() {
    let mut f = fixture();
    for (off, class, id) in [
        (0x08, b"coll", COLL),
        (0x10, b"jmad", JMAD),
        (0x20, b"phmo", PHMO),
    ] {
        f.poke(f.hlmt + off, fourcc(class));
        f.poke(f.hlmt + off + 4, id.0);
    }
    let m = f.ctx.resolve_model(HLMT, StringId(0)).unwrap();
    assert_eq!(m.collision_model.as_ref().map(|l| l.id), Some(COLL));
    assert_eq!(m.animation.as_ref().map(|l| l.id), Some(JMAD));
    assert_eq!(m.physics_model.as_ref().map(|l| l.id), Some(PHMO));
    assert_eq!(m.render_model.as_ref().map(|l| l.id), Some(MODE));
    // Each slot rejects a live reference of another model class.
    for (off, class, id) in [
        (0x00, b"coll", COLL),
        (0x08, b"phmo", PHMO),
        (0x10, b"coll", COLL),
        (0x20, b"coll", COLL),
    ] {
        let mut g = fixture();
        g.poke(g.hlmt + off, fourcc(class));
        g.poke(g.hlmt + off + 4, id.0);
        assert!(
            g.ctx.decode(HLMT).is_err(),
            "hlmt +{off:#x} accepted {}",
            FourCC(*class)
        );
    }
}

/// Pins the reviewed layout facts so an edit to a table cannot pass silently.
#[test]
fn layout_pins() {
    let types: Vec<_> = OBJECT_TYPES
        .iter()
        .map(|t| (t.group, t.body_size, t.placement))
        .collect();
    assert_eq!(
        types,
        [
            ("bipd", 0x464, Some((0x60, 0x68, 0x54))),
            ("vehi", 0x428, Some((0x70, 0x78, 0x54))),
            ("weap", 0x25c, Some((0x90, 0x98, 0x54))),
            ("eqip", 0x184, Some((0x80, 0x88, 0x38))),
            ("garb", 0x170, None),
            ("proj", 0x1ac, None),
            ("scen", 0x13c, Some((0x50, 0x58, 0x5c))),
            ("mach", 0x1e0, Some((0xa8, 0xb0, 0x48))),
            ("ctrl", 0x1d4, Some((0xb8, 0xc0, 0x44))),
            ("lifi", 0x1e4, Some((0xc8, 0xd0, 0x54))),
            ("ssce", 0x148, Some((0xd8, 0xe0, 0x50))),
            ("bloc", 0x130, Some((0x328, 0x330, 0x4c))),
            ("crea", 0x208, Some((0x368, 0x370, 0x34))),
        ]
    );
    let chains: Vec<_> = GROUPS
        .iter()
        .map(|g| {
            (
                g.group,
                g.root_size,
                g.layers
                    .iter()
                    .map(|l| (l.name, l.start, l.end))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let obje = ("obje", 0, 0xbc);
    let (unit, item, devi) = (
        ("unit", 0xbc, 0x1ec),
        ("item", 0xbc, 0x12c),
        ("devi", 0xbc, 0x11c),
    );
    assert_eq!(
        chains,
        [
            ("bipd", 0x314, vec![obje, unit, ("bipd", 0x1ec, 0x314)]),
            ("vehi", 0x300, vec![obje, unit, ("vehi", 0x1ec, 0x300)]),
            ("weap", 0x31c, vec![obje, item, ("weap", 0x12c, 0x31c)]),
            ("eqip", 0x13c, vec![obje, item, ("eqip", 0x12c, 0x13c)]),
            ("garb", 0x1d4, vec![obje, item, ("garb", 0x12c, 0x1d4)]),
            ("proj", 0x1a4, vec![obje, ("proj", 0xbc, 0x1a4)]),
            ("scen", 0xc4, vec![obje, ("scen", 0xbc, 0xc4)]),
            ("mach", 0x134, vec![obje, devi, ("mach", 0x11c, 0x134)]),
            ("ctrl", 0x140, vec![obje, devi, ("ctrl", 0x11c, 0x140)]),
            ("ssce", 0xcc, vec![obje, ("ssce", 0xbc, 0xcc)]),
            ("bloc", 0xc0, vec![obje, ("bloc", 0xbc, 0xc0)]),
            ("crea", 0x180, vec![obje, ("crea", 0xbc, 0x180)]),
            ("hlmt", 0xfc, vec![("hlmt", 0, 0xfc)]),
            ("itmc", 0xc, vec![("collection", 0, 0xc)]),
            ("vehc", 0xc, vec![("collection", 0, 0xc)]),
            ("obje", 0xbc, vec![obje]),
        ]
    );
    // (layout, field) -> (offset, block stride or 0, allowed classes).
    fn pin(l: &Layout, name: &str) -> (u32, u32, &'static [&'static str]) {
        let f = l
            .fields
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("{}.{name}", l.name));
        match f.kind {
            Kind::Block(s, _) => (f.offset, s, &[]),
            Kind::TagRef(c) => (f.offset, 0, c),
            _ => (f.offset, 0, &[]),
        }
    }
    use layouts as l;
    let none: &[&str] = &[];
    assert_eq!(pin(&l::OBJE, "object_type"), (0x00, 0, none));
    assert_eq!(pin(&l::OBJE, "bounding_radius"), (0x04, 0, none));
    assert_eq!(pin(&l::OBJE, "bounding_offset"), (0x08, 0, none));
    assert_eq!(pin(&l::OBJE, "default_variant"), (0x30, 0, none));
    assert_eq!(pin(&l::OBJE, "model"), (0x34, 0, &["hlmt"][..]));
    assert_eq!(pin(&l::OBJE, "creation_effect"), (0x4c, 0, &["effe"][..]));
    assert_eq!(pin(&l::OBJE, "functions"), (0x64, 0x20, none));
    assert_eq!(pin(&l::OBJE, "attachments"), (0x94, 0x18, none));
    assert_eq!(pin(&l::OBJE, "widgets"), (0x9c, 8, none));
    assert_eq!(pin(&l::OBJE, "change_colors"), (0xac, 0x10, none));
    assert_eq!(pin(&l::OBJE, "predicted_resources"), (0xb4, 8, none));
    assert_eq!(pin(&l::ATTACHMENT, "marker"), (0x08, 0, none));
    assert_eq!(pin(&l::FUNCTION, "output"), (0x08, 0, none));
    assert_eq!(pin(&l::BIPD, "flags"), (0x1f0, 0, none));
    assert_eq!(pin(&l::VEHI, "flags"), (0x1ec, 0, none));
    assert_eq!(pin(&l::VEHI, "vehicle_type"), (0x1f0, 0, none));
    // Weapon aim assist (docs/game/aim-assist.md), read by 0x001a50a0.
    assert_eq!(pin(&l::WEAP, "autoaim_angle"), (0x208, 0, none));
    assert_eq!(pin(&l::WEAP, "autoaim_range"), (0x20c, 0, none));
    assert_eq!(pin(&l::WEAP, "magnetism_angle"), (0x210, 0, none));
    assert_eq!(pin(&l::WEAP, "magnetism_range"), (0x214, 0, none));
    assert_eq!(pin(&l::WEAP, "deviation_angle"), (0x218, 0, none));
    assert_eq!(pin(&l::HLMT, "render_model"), (0x00, 0, &["mode"][..]));
    assert_eq!(pin(&l::HLMT, "collision_model"), (0x08, 0, &["coll"][..]));
    assert_eq!(pin(&l::HLMT, "animation"), (0x10, 0, &["jmad"][..]));
    assert_eq!(pin(&l::HLMT, "physics_model"), (0x20, 0, &["phmo"][..]));
    assert_eq!(pin(&l::HLMT, "variants"), (0x50, 0x38, none));
    assert_eq!(pin(&l::HLMT, "regions"), (0x70, 0x10, none));
    assert_eq!(pin(&l::HLMT, "nodes"), (0x78, 0x5c, none));
    assert_eq!(pin(&l::VARIANT, "region_map"), (0x04, 0, none));
    assert_eq!(pin(&l::VARIANT, "regions"), (0x14, 0x14, none));
    assert_eq!(pin(&l::VARIANT, "objects"), (0x1c, 0x10, none));
    assert_eq!(pin(&l::VARIANT_REGION, "model_region"), (0x04, 0, none));
    assert_eq!(pin(&l::VARIANT_REGION, "permutations"), (0x08, 0x20, none));
    assert_eq!(pin(&l::VARIANT_PERMUTATION, "probability"), (0x08, 0, none));
    assert_eq!(pin(&l::REGION, "permutations"), (0x08, 8, none));
    assert_eq!(pin(&l::COLLECTION, "entries"), (0x00, 0x10, none));
    assert_eq!(pin(&l::COLLECTION_ENTRY, "weight"), (0x00, 0, none));
    assert_eq!(pin(&l::COLLECTION_ENTRY, "object"), (0x04, 0, none));
    assert_eq!(pin(&l::COLLECTION_ENTRY, "variant"), (0x0c, 0, none));
}
