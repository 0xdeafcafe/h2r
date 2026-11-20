use super::*;

// 0x2963f0 selects exact or "any" at type, class and mode in that
// order. 0x1db170/0x1dbc80 search independent action/overlay blocks.
fn lookups(r: &Raw32) -> impl Fn(&str, &str, &str, &str, bool) -> Option<i16> + '_ {
    let ids: std::collections::HashMap<_, _> = r
        .strings
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            s.as_deref()
                .map(|s| (s, map::build::string_id(i, s).unwrap() as i32))
        })
        .collect();
    let find = move |field, stride, name: &str| {
        let wanted = *ids.get(name)?;
        let base = r.w(field + 4);
        let (mut lo, mut hi) = (0, r.w(field));
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let at = base + mid * stride;
            match (r.w(at) as i32).cmp(&wanted) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(at),
            }
        }
        None
    };
    move |mode, class, kind, key, overlay| {
        for m in [mode, "any"] {
            let Some(m) = find(r.base + 0x34, 20, m) else {
                continue;
            };
            for c in [class, "any"] {
                let Some(c) = find(m + 4, 20, c) else {
                    continue;
                };
                for t in [kind, "any"] {
                    if let Some(t) = find(c + 4, 52, t)
                        && let Some(a) = find(t + if overlay { 12 } else { 4 }, 8, key)
                    {
                        return Some((r.w(a + 4) >> 16) as i16);
                    }
                }
            }
        }
        None
    }
}

#[test]
fn borrowed_idle_variant_and_talk_resolve_on_separate_channels() {
    let (img, at) = synthetic_graph();
    let mut g = h1::Graph::read(&img, at).unwrap();
    g.animations[0].name = "alert pistol idle%1".into();
    g.animations[1].name = "stand talk".into();
    let mut slots = vec![-1; 11];
    slots[10] = 1;
    g.units = vec![h1::UnitSlots {
        label: "stand".into(),
        slots,
        classes: vec![h1::ClassSlots {
            label: "unarmed".into(),
            slots: vec![0],
            ..Default::default()
        }],
        ..Default::default()
    }];
    let convert = |g: &h1::Graph| {
        jmad::convert_graph_for(g, None, &Default::default(), jmad::GraphRole::Biped(&[]))
    };
    let r = Raw32::new(convert(&g).unwrap().0);
    let lookup = lookups(&r);
    assert_eq!(lookup("combat", "unarmed", "any", "idle", false), Some(0));
    assert_eq!(lookup("patrol", "rifle", "ar", "idle", false), Some(0));
    assert_eq!(lookup("combat", "pistol", "hp", "talk", true), Some(1));
    assert_eq!(lookup("combat", "pistol", "hp", "talk", false), None);
    g.units[0].classes[0].slots[0] = 1;
    assert!(convert(&g).unwrap_err().to_string().contains("Base clip 1"));
    g.units[0].classes[0].slots[0] = -1;
    assert!(
        convert(&g)
            .unwrap_err()
            .to_string()
            .contains("standing idle")
    );
    g.animations[0].name = "idle".into();
    let r = Raw32::new(convert(&g).unwrap().0);
    let lookup = lookups(&r);
    assert_eq!(lookup("combat", "unarmed", "any", "idle", false), Some(0));
}

#[test]
#[ignore = "reads all ten campaign maps from H1_ISO"]
fn campaign_biped_slots_and_talk_frames() {
    let mut iso = xiso::Xiso::open(std::io::BufReader::new(
        std::fs::File::open(std::env::var_os("H1_ISO").expect("set H1_ISO")).unwrap(),
    ))
    .unwrap();
    let directory = h1map::disc::Disc::open(&mut iso).unwrap();
    let mut names = std::collections::BTreeSet::new();
    let mut jaw = std::collections::BTreeSet::new();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let entry = directory.map(level).unwrap();
        let m = h1map::H1Map::from_reader(Cursor::new(iso.read(entry).unwrap())).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for b in m
            .tags()
            .iter()
            .filter(|b| b.group == h1map::Group::new(b"bipd"))
        {
            let object = m.tag_data(b).unwrap();
            let graph = m
                .tag(h1map::TagRef::read(object, 0x38).unwrap().id)
                .unwrap();
            if !seen.insert(graph.id.0) {
                continue;
            }
            let model = m
                .tag(h1map::TagRef::read(object, 0x28).unwrap().id)
                .unwrap();
            names.insert(graph.name.clone());
            let (g, _, converted) = crate::models::graph_from_map(&m, graph, Some(model))
                .unwrap_or_else(|e| panic!("{level} {}: {e}", b.name));
            let r = Raw32::new(converted);
            let nodes = r.w(r.base + 0x10);
            assert_eq!(r.w(nodes + 8) as i16, -1, "{} root parent", graph.name);
            for i in 0..r.w(r.base + 0xc) {
                let flags = r.w(nodes + i * 32 + 8) >> 16;
                assert_ne!(flags & 1, 0);
                let mask = if flags & 32 == 0 { 0x7c } else { 0x5c };
                assert_ne!(r.w(r.base + mask + 4 * (i / 32)) & (1 << (i % 32)), 0);
            }
            let lookup = lookups(&r);
            let default = lookup("any", "any", "any", "idle", false).expect(&graph.name);
            assert_eq!(g.animations[default as usize].kind, h1::AnimType::Base);
            if level == "a10" && graph.name == r"characters\marine\marine" {
                assert_eq!(lookup("combat", "pistol", "ne", "idle", false), Some(151));
                assert_eq!(
                    lookup("pchair_driver", "unarmed", "any", "idle", false),
                    Some(97)
                );
            }
            if level == "a10" && graph.name == r"characters\elite\elite" {
                assert_eq!(lookup("combat", "rifle", "pr", "idle", false), Some(92));
            }
            for u in &g.units {
                let mode = if u.label == "alert" {
                    "patrol".to_owned()
                } else {
                    crate::models::names::mode_name(&u.label)
                };
                for c in &u.classes {
                    for kind in
                        std::iter::once("any").chain(c.types.iter().map(|(name, _)| name.as_str()))
                    {
                        for (slot, key) in [
                            (0, "idle"),
                            (8, "walk_front"),
                            (9, "walk_back"),
                            (10, "walk_left"),
                            (11, "walk_right"),
                            (8, "move_front"),
                            (9, "move_back"),
                            (10, "move_left"),
                            (11, "move_right"),
                        ] {
                            let i =
                                lookup(&mode, &c.label, kind, key, false).unwrap_or_else(|| {
                                    panic!("{level} {} {mode}/{}:{key}", b.name, c.label)
                                });
                            let clip = &g.animations[i as usize];
                            assert!(
                                clip.frame_count > 0 && clip.kind == h1::AnimType::Base,
                                "{}",
                                clip.name
                            );
                            if let Some(&expected) = c.slots.get(slot).filter(|&&s| s >= 0) {
                                assert_eq!(
                                    i, expected,
                                    "{level} {} {mode}/{}:{kind}:{key}",
                                    graph.name, c.label
                                );
                            }
                        }
                    }
                }
                if let Some(&i) = u.slots.get(10).filter(|&&s| s >= 0) {
                    assert_eq!(lookup(&mode, "unarmed", "any", "talk", true), Some(i));
                }
            }
            if level == "a10" {
                let mut reader = Map::from_reader(Cursor::new(&r.map)).unwrap();
                let id = reader
                    .tags()
                    .iter()
                    .find(|t| t.group == map::FourCC(*b"jmad"))
                    .unwrap()
                    .id;
                let jmad = AnimationGraph::read(&mut reader, id).unwrap();
                let resource = jmad.read_resource(0, &mut Raw(r.map.clone())).unwrap();
                let mut idles = std::collections::BTreeSet::from([default]);
                for c in g.units.iter().flat_map(|u| &u.classes) {
                    if let Some(&i) = c.slots.first().filter(|&&i| i >= 0) {
                        idles.insert(i);
                    }
                }
                for i in idles {
                    let clip = jmad.clip_from_shared(i as usize, &resource).unwrap();
                    let [angle, translation, scale] =
                        compare_animation(&g.animations[i as usize], &clip);
                    assert!(
                        angle < 0.0002 && translation == 0.0 && scale == 0.0,
                        "{} idle {i}",
                        graph.name
                    );
                }
                let Some(i) = g
                    .units
                    .iter()
                    .find(|u| u.label == "stand")
                    .and_then(|u| u.slots.get(10))
                    .copied()
                    .filter(|&i| i >= 0)
                else {
                    continue;
                };
                assert_eq!(lookup("combat", "unarmed", "any", "talk", true), Some(i));
                let i = i as usize;
                let clip = jmad.clip_from_shared(i, &resource).unwrap();
                let source = &g.animations[i];
                assert_eq!(source.kind, h1::AnimType::Overlay);
                assert_eq!(source.frame_count, 1);
                let [angle, translation, scale] = compare_animation(source, &clip);
                assert!(angle < 0.0002 && translation == 0.0 && scale == 0.0);
                if let Some(n) = g.nodes.iter().position(|n| n.name == "frame jawbone") {
                    let t = &source.frames[0][n];
                    assert!(
                        (source.has(0, n) && t.rotation[..3].iter().any(|v| v.abs() > 0.001))
                            || (source.has(1, n) && t.translation.iter().any(|v| v.abs() > 0.001))
                    );
                    jaw.insert(graph.name.clone());
                }
            }
        }
    }
    assert_eq!(names.len(), 25);
    assert!(jaw.contains(r"characters\captain\captain"));
    assert!(jaw.contains(r"characters\cortana\cortana"));
}
