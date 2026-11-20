//! Every point's shape against the user's XBE (`engine::point_shapes`):
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

use engine::point_shapes::{self, DECLARED, Verdict};
use engine::points::{self, Point, Sig};

fn checked(x: &xbe::Xbe, point: &Point, addr: u32) -> Verdict {
    let read = |va: u32, buf: &mut [u8]| match x.bytes_at(va, buf.len()) {
        Ok(b) => {
            buf.copy_from_slice(b);
            true
        }
        Err(_) => false,
    };
    point_shapes::verify(point, addr, &read)
}

#[test]
fn every_point_has_its_shape_in_the_image() {
    let Some(path) = common::xbe_path() else {
        eprintln!("SKIPPED every_point_has_its_shape_in_the_image: no XBE (set H2_XBE)");
        return;
    };
    let bytes = std::fs::read(&path).expect("read the XBE");
    assert_eq!(common::sha256(&bytes), common::IMAGE_SHA256, "not the retail image");
    let x = xbe::Xbe::parse(&bytes).expect("parse the XBE");

    // The table, the declared sites, and the points features make from the
    // leaf adapters' addresses (opaque, so a function start each).
    let mut all: Vec<&Point> = points::ALL.to_vec();
    all.extend(DECLARED.iter().map(|(p, _)| *p));
    let leaves: Vec<Point> = engine::abi::OVERRIDES
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
