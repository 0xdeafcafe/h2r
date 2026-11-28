//! The Forge kit's bodies (docs/game/forge-design-slices.md, "The monitor
//! body"): the units a player flies as, transplanted from the player's own
//! campaign maps into the kit map.
//!
//! A kit build copies each body of `forge_doc::bodies::SPECS` in order,
//! the Monitor first: its unit tag (and the chair it sits in, for the
//! Prophet) with everything they reference, from the first donor map that
//! holds them, with their sounds merged into the map's gestalt
//! (`firefight_kit::copy_unit`). The bodies go in before the block-out
//! set, so the starter set is what the cut order drops to make room for
//! them (docs/game/forge-blockout.md, "The cut order").
//!
//! A body that does not go in is a [`Row::Refused`] with its reason, and
//! the build goes on: the Forge record names it (`record::BODY_REFUSED`),
//! and the game refuses that body by name, or Forge itself when it is the
//! Monitor. Nothing is swapped for another body.

use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::sync::Arc;

use content_kit::blockout::{Cost, cost_since};
use forge_doc::bodies::{Body, Refused, Spec, TagPath};
use map::build::TagSet;
use map::{FourCC, Map, TagQuery};
use scenario_edit::MapSource;
use tags::schema::Schema;

use crate::Error;

/// What became of one body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// In the kit, copied from `donor`.
    Carried { donor: String, cost: Cost },
    /// Left out, with why and the detail for the build's report.
    Refused { why: Refused, detail: String },
}

/// What [`install`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Each body in [`Body::ALL`] order.
    pub rows: Vec<(Body, Row)>,
    /// The tag memory the base had for them.
    pub headroom: u64,
}

impl Report {
    /// The bodies left out, for the Forge record.
    pub fn refused(&self) -> BTreeMap<Body, Refused> {
        self.rows
            .iter()
            .filter_map(|(body, row)| match row {
                Row::Refused { why, .. } => Some((*body, *why)),
                Row::Carried { .. } => None,
            })
            .collect()
    }
}

/// Copies a body's tags into a set: the donors on this machine's disc
/// ([`Disc`]), or a test's.
pub trait Copier {
    /// Copy `spec`'s chair, then its unit, into `set` from one donor; the
    /// donor's name. Refused when no donor holds them or one cannot be
    /// copied. `set` may be left part-written on a refusal.
    fn copy(&mut self, set: &mut TagSet, spec: &Spec) -> Result<String, (Refused, String)>;
}

/// The donors of the machine's disc, read when a body first asks for one.
pub struct Disc<'a> {
    schema: &'a Schema,
    lookup: &'a mut dyn FnMut(&str) -> Option<MapSource>,
    read: HashMap<String, Option<Arc<[u8]>>>,
}

impl<'a> Disc<'a> {
    pub fn new(schema: &'a Schema, lookup: &'a mut dyn FnMut(&str) -> Option<MapSource>) -> Self {
        Self {
            schema,
            lookup,
            read: HashMap::new(),
        }
    }

    fn bytes(&mut self, donor: &str) -> Option<Arc<[u8]>> {
        let key = donor.to_ascii_lowercase();
        if !self.read.contains_key(&key) {
            let bytes = (self.lookup)(donor).and_then(|s| crate::source_bytes(donor, &s, false).ok());
            self.read.insert(key.clone(), bytes);
        }
        self.read.get(&key).cloned().flatten()
    }
}

fn holds(bytes: &[u8], tag: &TagPath) -> bool {
    Map::from_reader(Cursor::new(bytes))
        .is_ok_and(|map| map.tag(TagQuery::NameInGroup(tag.path, FourCC(tag.group))).is_ok())
}

impl Copier for Disc<'_> {
    fn copy(&mut self, set: &mut TagSet, spec: &Spec) -> Result<String, (Refused, String)> {
        let tags: Vec<&TagPath> = spec.chair.iter().chain([&spec.unit]).collect();
        for &donor in spec.donors {
            let Some(bytes) = self.bytes(donor) else { continue };
            if !tags.iter().all(|t| holds(&bytes, t)) {
                continue;
            }
            let from = [firefight_kit::Donor {
                name: donor,
                bytes: &bytes,
            }];
            for tag in &tags {
                firefight_kit::copy_unit(
                    set,
                    self.schema,
                    &from,
                    spec.body.name(),
                    FourCC(tag.group),
                    tag.path,
                )
                .map_err(|e| (Refused::CannotCopy, format!("{e:#}")))?;
            }
            return Ok(donor.to_owned());
        }
        Err((
            Refused::NoDonor,
            format!(
                "none of {} on this machine holds {}",
                spec.donors.join(", "),
                spec.unit.path
            ),
        ))
    }
}

/// Copies every body into `base` within `headroom` bytes of tag memory
/// (`crate::blockout::headroom`), in order, each into a copy of the set so
/// a refused one leaves nothing. The map with those that went in, or none
/// when none did, and the report.
pub fn install(
    base: &[u8],
    headroom: u64,
    copier: &mut impl Copier,
) -> Result<(Option<Vec<u8>>, Report), Error> {
    let mut map = Map::from_reader(Cursor::new(base))?;
    let mut report = Report {
        headroom,
        ..Report::default()
    };
    // A base the tag set cannot take apart carries no body: each is
    // refused by name, and the build goes on, as the block-out set's
    // `unavailable` does.
    let mut set = match TagSet::from_map(&mut map) {
        Ok(set) => set,
        Err(e) => {
            let detail = format!("the base map cannot carry new tags: {e}");
            report.rows = Body::ALL
                .into_iter()
                .map(|body| {
                    let why = Refused::CannotCopy;
                    (body, Row::Refused { why, detail: detail.clone() })
                })
                .collect();
            return Ok((None, report));
        }
    };
    content_kit::blockout::make_index_room(&mut set);
    let mut used = 0;
    let mut carried = false;
    for body in Body::ALL {
        let spec = body.spec();
        let mut trial = set.clone();
        let (slots, raw) = (trial.slots.len(), trial.raw.len());
        let row = match copier.copy(&mut trial, spec) {
            Err((why, detail)) => Row::Refused { why, detail },
            Ok(donor) => {
                let cost = cost_since(&trial, slots, raw);
                if used + cost.tag_memory > headroom {
                    Row::Refused {
                        why: Refused::NoRoom,
                        detail: format!(
                            "it takes {} B of tag memory ({} tags), and {} B of the map's {} B \
                             headroom are left",
                            cost.tag_memory,
                            cost.tags,
                            headroom - used,
                            headroom
                        ),
                    }
                } else {
                    used += cost.tag_memory;
                    set = trial;
                    carried = true;
                    Row::Carried { donor, cost }
                }
            }
        };
        report.rows.push((body, row));
    }
    if !carried {
        return Ok((None, report));
    }
    let (bytes, _) = set.build()?;
    Ok((Some(bytes), report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::build::{Block, TagPayload};

    /// Copies an invented tag of `size` bytes for each body it is given a
    /// size for, and refuses the rest as having no donor.
    struct Fake(Vec<(Body, Result<usize, Refused>)>);

    impl Copier for Fake {
        fn copy(&mut self, set: &mut TagSet, spec: &Spec) -> Result<String, (Refused, String)> {
            match self.0.iter().find(|(b, _)| *b == spec.body).map(|(_, r)| *r) {
                Some(Ok(size)) => {
                    let block = Block::new(vec![0; size]);
                    // As `tags::schema::transplant` does: the group is
                    // declared before its tag.
                    set.add_group(FourCC(spec.unit.group), None, None);
                    set.add_tag(FourCC(spec.unit.group), spec.unit.path, TagPayload::Built(block))
                        .map_err(|e| (Refused::CannotCopy, e.to_string()))?;
                    Ok("invented_donor".into())
                }
                Some(Err(why)) => Err((why, "invented".into())),
                None => Err((Refused::NoDonor, "invented".into())),
            }
        }
    }

    fn base() -> Vec<u8> {
        content_kit::blockout::fixture::fixture()
    }

    #[test]
    fn each_body_goes_in_or_is_refused_by_name_and_order() {
        let base = base();
        let mut fake = Fake(vec![
            (Body::Monitor, Ok(0x400)),
            (Body::Sentinel, Err(Refused::CannotCopy)),
            (Body::Prophet, Ok(0x10_0000)),
        ]);
        let (bytes, report) = install(&base, 0x2000, &mut fake).unwrap();
        assert_eq!(report.rows.len(), 3);
        assert!(matches!(&report.rows[0], (Body::Monitor, Row::Carried { donor, .. }) if donor == "invented_donor"));
        assert!(matches!(&report.rows[1], (Body::Sentinel, Row::Refused { why: Refused::CannotCopy, .. })));
        match &report.rows[2] {
            (Body::Prophet, Row::Refused { why: Refused::NoRoom, detail }) => {
                assert!(detail.contains("headroom are left"), "{detail}");
            }
            other => panic!("the Prophet does not fit: {other:?}"),
        }
        assert_eq!(
            report.refused(),
            BTreeMap::from([(Body::Sentinel, Refused::CannotCopy), (Body::Prophet, Refused::NoRoom)])
        );
        // The Monitor is in the map it gives, by its own path.
        let bytes = bytes.expect("a map");
        let map = Map::from_reader(Cursor::new(bytes.as_slice())).unwrap();
        let monitor = Body::Monitor.spec().unit;
        assert!(map.tag(TagQuery::NameInGroup(monitor.path, FourCC(monitor.group))).is_ok());
        let prophet = Body::Prophet.spec().unit;
        assert!(map.tag(TagQuery::NameInGroup(prophet.path, FourCC(prophet.group))).is_err());
    }

    /// A base the tag set cannot take apart (its index is not at the retail
    /// base) refuses every body by name, and the build goes on.
    #[test]
    fn a_base_that_cannot_take_tags_refuses_every_body() {
        let base = scenario_edit::fixture::fixture_at_end();
        let mut fake = Fake(vec![(Body::Monitor, Ok(0x400))]);
        let (bytes, report) = install(&base, 0x2000, &mut fake).unwrap();
        assert_eq!(bytes, None);
        assert_eq!(report.rows.len(), 3);
        for (_, row) in &report.rows {
            match row {
                Row::Refused { why: Refused::CannotCopy, detail } => {
                    assert!(detail.contains("cannot carry new tags"), "{detail}");
                }
                other => panic!("refused: {other:?}"),
            }
        }
    }

    #[test]
    fn no_body_leaves_the_base_as_it_is() {
        let base = base();
        let (bytes, report) = install(&base, 0x2000, &mut Fake(vec![])).unwrap();
        assert_eq!(bytes, None);
        assert!(
            report
                .rows
                .iter()
                .all(|(_, r)| matches!(r, Row::Refused { why: Refused::NoDonor, .. }))
        );
        assert_eq!(report.refused().len(), 3);
    }
}
