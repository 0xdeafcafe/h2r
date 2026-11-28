//! Every point's shape against the user's XBE (`guest_layout::point_shapes`):
//! the check the plan runs at install, run here over the whole table so a
//! wrong declaration is found before it turns a feature off.
//!
//! Needs the user's XBE (`H2_XBE`, or `disc/default.xbe`); skips without
//! it. Nothing from the image is kept: the test reads it and compares.
//!
//! ```text
//! H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential \
//!     --test it point_shapes::
//! ```
#![cfg(feature = "differential")]

mod common;

use guest_layout::point_shapes::{self, DECLARED, Verdict};
use guest_layout::points::{self, Point, Sig};
use guest_layout::points::scene_effects::EFFECT_LEAVES;

fn reader<'a>(x: &'a xbe::Xbe<'_>) -> impl Fn(u32, &mut [u8]) -> bool + 'a {
    move |va: u32, buf: &mut [u8]| match x.bytes_at(va, buf.len()) {
        Ok(b) => {
            buf.copy_from_slice(b);
            true
        }
        Err(_) => false,
    }
}

fn checked(x: &xbe::Xbe, point: &Point, addr: u32) -> Verdict {
    point_shapes::verify(point, addr, &reader(x))
}

/// The retail XBE's bytes, or `None` without one.
fn image() -> Option<Vec<u8>> {
    let path = common::xbe_path()?;
    let bytes = std::fs::read(&path).expect("read the XBE");
    assert_eq!(common::sha256(&bytes), common::IMAGE_SHA256, "not the retail image");
    Some(bytes)
}

#[test]
fn every_point_has_its_shape_in_the_image() {
    let Some(bytes) = image() else {
        eprintln!("SKIPPED every_point_has_its_shape_in_the_image: no XBE (set H2_XBE)");
        return;
    };
    let x = xbe::Xbe::parse(&bytes).expect("parse the XBE");

    // The table, the declared sites, and the points features make from the
    // leaf adapters' addresses (opaque, so a function start each).
    let mut all: Vec<&Point> = points::ALL.to_vec();
    all.extend(DECLARED.iter().map(|(p, _)| *p));
    all.extend(engine::sim_anim::manager::bodies().map(|b| points::by_base(b.entry).unwrap_or(&b.point)));
    let leaves: Vec<Point> = guest_layout::abi::OVERRIDES
        .iter()
        .map(|o| Point::new(o.name, o.addr, Sig::Opaque, "docs/engine/README.md"))
        .collect();
    all.extend(leaves.iter());

    let mut wrong = Vec::new();
    let mut held = 0;
    for p in all {
        match checked(&x, p, p.base) {
            Verdict::Holds => held += 1,
            Verdict::Mismatch(why) => wrong.push(why),
            Verdict::Unreadable => wrong.push(format!("{} ({:#010x}) does not read", p.name, p.base)),
            Verdict::Unchecked => {}
        }
    }
    assert!(wrong.is_empty(), "{} of the points are not what they say:\n{}", wrong.len(), wrong.join("\n"));
    assert!(held > 300, "only {held} points checked");
}

/// The immediate effect and flare observers: a site where a call ends is a
/// call's continuation, which install refuses ("... dropped: ... is a
/// call's continuation"), so the observer would never run. This reads
/// only the bytes before each site; install also needs the image's resume
/// table, which every call's continuation is in.
#[test]
fn no_effect_leaf_site_is_a_calls_continuation() {
    let Some(bytes) = image() else {
        eprintln!("SKIPPED no_effect_leaf_site_is_a_calls_continuation: no XBE (set H2_XBE)");
        return;
    };
    let x = xbe::Xbe::parse(&bytes).expect("parse the XBE");
    let read = reader(&x);
    let wrong: Vec<String> = EFFECT_LEAVES
        .into_iter()
        .flat_map(|leaf| [&leaf.begin, &leaf.end])
        .filter_map(|p| point_shapes::continuation(p, p.base, &read))
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
