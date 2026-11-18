//! Checks over a built map's tags that hold on every retail map, for the
//! disc tests to run on converted ones.

use std::io::{Read, Seek};

use map::{FourCC, Map, TagId};
use tags::object::Metadata;

use crate::Result;
use crate::rebuild::{HLMT_MATERIAL_SIZE, HLMT_MATERIALS, PHMO_MATERIALS};

/// `phmo` shape blocks (field, element size), each element's header
/// naming its material at `+4`: spheres, pills, boxes, triangles and
/// polyhedra.
const PHMO_SHAPES: [(u32, u32); 5] = [
    (0x48, 0x80),
    (0x58, 0x50),
    (0x60, 0x90),
    (0x68, 0x60),
    (0x70, 0x100),
];

/// `coll +0x14`: the materials, a name each (4 bytes).
const COLL_MATERIALS: u32 = 0x14;

/// Every `hlmt` of `map` whose materials (`+0x58`) break the rule the
/// engine relies on ([`crate::rebuild::HLMT_MATERIALS`]), one line each:
/// - with a `coll`, element `k` is named as the `coll`'s material `k`,
///   and there are at least as many elements;
/// - with a `phmo`, there is an element for each of its materials and for
///   the largest material index a shape names;
/// - `+0x10` is the index, in the globals' materials, of the `+0xc` name
///   (of the `+0` name when `+0xc` is empty).
///
/// Retail holds the first and third on every `hlmt` of coagulation,
/// beavercreek and 05a; the second fails on four of their 62 with a
/// `phmo` (`tests/hlmt_materials_disc.rs`), so a caller compares against
/// the base map's own lines.
pub fn hlmt_materials<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<String>> {
    let meta = Metadata::read(map)?;
    let w = |a: u32| {
        meta.slice(a, 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4")))
            .unwrap_or(0)
    };
    let strings = map.strings().to_vec();
    let s = |v: u32| -> String {
        if v == 0 {
            return String::new();
        }
        strings
            .get((v & 0xff_ffff) as usize)
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("{v:#x}"))
    };
    let tags = map.tags().to_vec();
    let addr = |id: u32| {
        tags.iter()
            .find(|t| t.id == TagId(id) && id != u32::MAX)
            .and_then(|t| t.location.map(|l| l.address))
    };
    let globals: Vec<String> = tags
        .iter()
        .find(|t| t.group == FourCC(*b"matg"))
        .and_then(|t| t.location)
        .map(|l| {
            let at = l.address + crate::base::GLOBAL_MATERIALS as u32;
            let (n, first) = (w(at), w(at + 4));
            (0..n)
                .map(|i| s(w(first + i * crate::base::GLOBAL_MATERIAL_SIZE as u32)))
                .collect()
        })
        .unwrap_or_default();
    let mut bad = Vec::new();
    for t in tags.iter().filter(|t| t.group == FourCC(*b"hlmt")) {
        let Some(l) = t.location else { continue };
        let label = t.name.clone().unwrap_or_default();
        let a = l.address;
        let (n, first) = (w(a + HLMT_MATERIALS), w(a + HLMT_MATERIALS + 4));
        let element = |k: u32| first + k * HLMT_MATERIAL_SIZE as u32;
        if let Some(c) = addr(w(a + 0xc)) {
            let (cn, cf) = (w(c + COLL_MATERIALS), w(c + COLL_MATERIALS + 4));
            if cn > n {
                bad.push(format!("{label}: {cn} coll materials, {n} hlmt materials"));
            }
            for k in 0..cn.min(n) {
                let (cm, hm) = (s(w(cf + 4 * k)), s(w(element(k))));
                if cm != hm {
                    bad.push(format!(
                        "{label}: material {k} is {hm:?}, the coll's {cm:?}"
                    ));
                }
            }
        }
        if let Some(p) = addr(w(a + 0x24)) {
            let pn = w(p + PHMO_MATERIALS);
            if pn > n {
                bad.push(format!("{label}: {pn} phmo materials, {n} hlmt materials"));
            }
            for (field, size) in PHMO_SHAPES {
                let (sn, sf) = (w(p + field), w(p + field + 4));
                for i in 0..sn {
                    let m = w(sf + i * size + 4) as u16 as u32;
                    if m >= n {
                        bad.push(format!(
                            "{label}: phmo shape at +{field:#x} names material {m}, {n} hlmt materials"
                        ));
                        break;
                    }
                }
            }
        }
        for k in 0..n {
            let e = element(k);
            let (name, global) = (s(w(e)), s(w(e + 0xc)));
            let key = if global.is_empty() { &name } else { &global };
            let index = w(e + 0x10) as u16 as i16;
            let want = globals.iter().position(|g| g == key);
            if want.map(|g| g as i16) != Some(index) {
                bad.push(format!(
                    "{label}: material {k} {name:?} global {key:?} at {want:?}, +0x10 holds {index}"
                ));
            }
        }
    }
    Ok(bad)
}
