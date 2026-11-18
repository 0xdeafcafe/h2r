//! Retail proof against the user's own disc (`H2_DISC` or disc/halo2.iso):
//! `cargo test -p mopp --release --test disc -- --ignored --nocapture`.
mod common;
use common::metrics::{Metrics, measure, sample_boxes};
use mopp::geom::structure_surfaces;

const BOXES: usize = 400;
/// Truth shrinks each box by 1 mm.
const EPS: f32 = 1e-3;

/// Every retail program validates by the engine's rules, and our box model
/// of its interpreter returns every structure surface a sampled box truly
/// touches.
#[test]
#[ignore = "needs the disc"]
fn retail_programs_are_supersets() {
    let mut total = Metrics::default();
    for l in common::all_levels().unwrap() {
        let code = mopp::Code::parse(&l.mopp).unwrap();
        mopp::check::validate(&code.bytecode).unwrap();
        let surfaces = structure_surfaces(&l.collision);
        let boxes = sample_boxes(&surfaces, BOXES, 0x5eed ^ l.bsp_index as u64);
        let m = measure(&code, &surfaces, &boxes, EPS);
        println!(
            "retail {:>20} {} surfaces={:>5} truth={:>6} returned={:>6} ratio={:.2} bounds_ratio={:.2} misses={} sphere_misses={} box_misses={}",
            l.name,
            l.bsp_index,
            surfaces.len(),
            m.truth,
            m.returned,
            m.ratio(),
            m.bounds_ratio(),
            m.misses,
            m.sphere_misses,
            m.box_misses
        );
        total.add(&m);
    }
    println!(
        "retail total {total:?} ratio={:.2} bounds_ratio={:.2}",
        total.ratio(),
        total.bounds_ratio()
    );
    assert_eq!(total.misses, 0);
    assert_eq!(total.sphere_misses, 0);
    assert_eq!(total.box_misses, 0);
}

/// Our compiler on every retail BSP's own surfaces: valid by the engine's
/// rules, every structure key present once, and a superset on the same
/// sampled boxes.
#[test]
#[ignore = "needs the disc"]
fn built_programs_are_supersets() {
    let mut total = Metrics::default();
    let (mut ours, mut theirs) = (0usize, 0usize);
    for l in common::all_levels().unwrap() {
        let block = mopp::build_structure(&l.collision);
        let code = mopp::Code::parse(&block).unwrap();
        assert_eq!(block.len() % 16, 0);
        let report = mopp::check::validate(&code.bytecode).unwrap();
        let want: Vec<u32> = (0..l.collision.surfaces.len())
            .map(mopp::geom::structure_key)
            .collect();
        assert_eq!(report.keys, want, "{} {}", l.name, l.bsp_index);
        assert_eq!(report.unreached, 0);
        let surfaces = structure_surfaces(&l.collision);
        let boxes = sample_boxes(&surfaces, BOXES, 0x5eed ^ l.bsp_index as u64);
        let m = measure(&code, &surfaces, &boxes, EPS);
        println!(
            "ours   {:>20} {} surfaces={:>5} truth={:>6} returned={:>6} ratio={:.2} bounds_ratio={:.2} misses={} sphere_misses={} box_misses={} bytes={} (retail {}) depth={}",
            l.name,
            l.bsp_index,
            surfaces.len(),
            m.truth,
            m.returned,
            m.ratio(),
            m.bounds_ratio(),
            m.misses,
            m.sphere_misses,
            m.box_misses,
            block.len(),
            l.mopp.len(),
            report.max_depth
        );
        ours += block.len();
        theirs += l.mopp.len();
        total.add(&m);
    }
    println!(
        "ours total {total:?} ratio={:.2} bounds_ratio={:.2} bytes={ours} retail={theirs}",
        total.ratio(),
        total.bounds_ratio()
    );
    assert_eq!(total.misses, 0);
    assert_eq!(total.sphere_misses, 0);
    assert_eq!(total.box_misses, 0);
}

/// Multiplayer levels whose rebuilt block fits over retail's.
const REPLACE: &[&str] = &[
    "lockout",
    "colossus",
    "cyclotron",
    "headlong",
    "coagulation",
    "zanzibar",
];

/// End to end through map::build: a retail level with our program in its
/// BSP region reads back with a valid checksum and our block, and on the
/// sampled boxes our program returns every structure surface the box
/// touches and every other key (instanced geometry and the rest) that the
/// retail program returns.
#[test]
#[ignore = "needs the disc"]
fn replaced_retail_levels() {
    for name in REPLACE {
        let mut map = common::open_map(name).unwrap();
        let r = match common::replace::replace(&mut map, 0) {
            Ok(r) => r,
            Err(e) => panic!("{name}: {e:#}"),
        };
        let id = common::scenario_bsps(&mut map).unwrap()[0];
        let level = common::load_level(&mut map, name, 0, id).unwrap();
        let retail = mopp::Code::parse(&level.mopp).unwrap();
        let ours = mopp::Code::parse(&r.block).unwrap();
        mopp::check::validate(&ours.bytecode).unwrap();
        let surfaces = structure_surfaces(&level.collision);
        let boxes = sample_boxes(&surfaces, BOXES, 0x5eed);
        let m = measure(&ours, &surfaces, &boxes, EPS);
        let mut other = (0usize, 0usize);
        for b in &boxes {
            let theirs = mopp::query_aabb(&retail, b).unwrap();
            let mine: std::collections::HashSet<u32> =
                mopp::query_aabb(&ours, b).unwrap().into_iter().collect();
            for k in theirs
                .into_iter()
                .filter(|k| k >> 29 != mopp::geom::STRUCTURE_KEY_TYPE)
            {
                other.0 += 1;
                if !mine.contains(&k) {
                    other.1 += 1;
                }
            }
        }
        println!(
            "replaced {name:>12}: block {} of {} bytes, map {} bytes; structure misses {} box misses {} ratio {:.2}; other keys {} missed {}",
            r.block.len(),
            r.retail_count,
            r.map.len(),
            m.misses,
            m.box_misses,
            m.ratio(),
            other.0,
            other.1
        );
        assert_eq!(m.misses, 0, "{name}");
        assert_eq!(m.box_misses, 0, "{name}");
        assert_eq!(other.1, 0, "{name}");
    }
}

/// Code size of our structure programs with each of converter version
/// 72's builder changes undone, over every retail BSP and on lockout:
/// `cargo test -p mopp --release --test disc size_by_change -- --ignored --nocapture`.
#[test]
#[ignore = "needs the disc"]
fn size_by_change() {
    use mopp::build::{Legacy, build_legacy};
    let levels = common::all_levels().unwrap();
    let all = Legacy {
        margin: true,
        inclusive_tops: true,
        zoom: true,
        region: true,
    };
    let cases = [
        ("v72", Legacy::default()),
        ("v71 (all undone)", all),
        (
            "margin only undone",
            Legacy {
                margin: true,
                ..Default::default()
            },
        ),
        (
            "tops only undone",
            Legacy {
                inclusive_tops: true,
                ..Default::default()
            },
        ),
        (
            "zoom only undone",
            Legacy {
                zoom: true,
                ..Default::default()
            },
        ),
        (
            "region only undone",
            Legacy {
                region: true,
                ..Default::default()
            },
        ),
        (
            "all but margin undone",
            Legacy {
                margin: false,
                ..all
            },
        ),
        (
            "all but tops undone",
            Legacy {
                inclusive_tops: false,
                ..all
            },
        ),
        ("all but zoom undone", Legacy { zoom: false, ..all }),
        (
            "all but region undone",
            Legacy {
                region: false,
                ..all
            },
        ),
    ];
    for (name, l) in cases {
        let (mut total, mut lockout) = (0usize, 0usize);
        for lv in &levels {
            let n = build_legacy(&structure_surfaces(&lv.collision), l)
                .to_block()
                .len();
            total += n;
            if lv.name == "lockout" && lv.bsp_index == 0 {
                lockout = n;
            }
        }
        println!("{name:>24}: total {total} bytes, lockout {lockout}");
    }
}
