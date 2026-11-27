use std::cell::RefCell;
use std::collections::HashMap;

use content::Library;

use super::*;
use crate::testkit;

struct Disc {
    maps: HashMap<String, Vec<u8>>,
    reads: RefCell<Vec<String>>,
}

impl Disc {
    fn new(maps: &[(&str, Vec<u8>)]) -> Self {
        Disc {
            maps: maps.iter().map(|(n, b)| ((*n).to_owned(), b.clone())).collect(),
            reads: RefCell::default(),
        }
    }
    fn reads(&self) -> usize {
        self.reads.borrow().len()
    }
}

impl DiscMaps for Disc {
    fn read_map(&self, name: &str) -> Result<Vec<u8>> {
        self.reads.borrow_mut().push(name.to_owned());
        self.maps
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the disc has no maps\\{name}.map"))
    }
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ff-ensure-{name}-{}", std::process::id()))
}

fn court(hash: u64) -> Vec<Target<'static>> {
    vec![Target {
        map: testkit::NAME,
        config: Config::default(),
        layout_hash: hash,
    }]
}

#[test]
fn ensure_builds_missing_and_stale_kits_only() {
    let dir = scratch("stale");
    let _ = std::fs::remove_dir_all(&dir);
    let disc = Disc::new(&[(testkit::NAME, testkit::court())]);
    assert_eq!(pending_with(&court(1), &dir), [testkit::NAME], "nothing built yet");
    let first = ensure_with(&court(1), &disc, &dir);
    assert!(matches!(first[0].outcome, Outcome::Built { .. }), "{first:?}");
    assert_eq!(disc.reads(), 1);
    assert!(pending_with(&court(1), &dir).is_empty());
    let lib = Library::open(&dir);
    assert_eq!(lib.packages().len(), 1);
    assert_eq!(lib.packages()[0].manifest.package.name, "Firefight \u{2013} court");
    // Current: no build, and the disc is not read.
    let again = ensure_with(&court(1), &disc, &dir);
    assert_eq!(again[0].outcome, Outcome::Current);
    assert_eq!(disc.reads(), 1);
    // A changed layout is stale: built again, over the same package.
    assert_eq!(pending_with(&court(2), &dir), [testkit::NAME]);
    let stale = ensure_with(&court(2), &disc, &dir);
    assert!(matches!(stale[0].outcome, Outcome::Built { .. }), "{stale:?}");
    assert_eq!(disc.reads(), 2);
    assert_eq!(Library::open(&dir).packages().len(), 1, "replaced, not added");
    // A package someone else built (no stamp) is stale too.
    std::fs::remove_file(stamp_path(&dir, testkit::NAME)).unwrap();
    assert!(matches!(
        ensure_with(&court(2), &disc, &dir)[0].outcome,
        Outcome::Built { .. }
    ));
    // So is a stamp with no package.
    std::fs::remove_dir_all(dir.join(package_dir(testkit::NAME))).unwrap();
    assert!(matches!(
        ensure_with(&court(2), &disc, &dir)[0].outcome,
        Outcome::Built { .. }
    ));
    assert_eq!(disc.reads(), 4);
    assert_eq!(
        ensure_with(&court(2), &disc, &dir)[0].outcome,
        Outcome::Current
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn ensure_reports_each_refusal_by_map() {
    let dir = scratch("refusals");
    let _ = std::fs::remove_dir_all(&dir);
    let disc = Disc::new(&[(testkit::NAME, testkit::court()), ("broken", vec![0; 64])]);
    let mut targets = court(1);
    for map in ["absent", "broken"] {
        targets.insert(
            0,
            Target {
                map,
                config: Config::default(),
                layout_hash: 1,
            },
        );
    }
    let reports = ensure_with(&targets, &disc, &dir);
    let maps: Vec<_> = reports.iter().map(|r| r.map.as_str()).collect();
    assert_eq!(maps, ["broken", "absent", testkit::NAME], "in target order");
    let lines: Vec<_> = reports.iter().map(Report::line).collect();
    assert!(lines[0].as_deref().is_some_and(|l| l.starts_with("Broken: ")), "{lines:?}");
    assert_eq!(
        lines[1].as_deref(),
        Some("Absent: the disc has no maps\\absent.map")
    );
    assert_eq!(lines[2], None, "one map's refusal does not stop the next");
    assert!(matches!(reports[2].outcome, Outcome::Built { .. }));
    assert!(
        !pending_with(&targets, &dir).contains(&testkit::NAME),
        "the built one is current"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_headroom_refusal_reads_in_kilobytes_and_the_biggest_part() {
    let text = "Firefight kit exceeds tag headroom by 6144 B; the biggest part is sectors (9000 B), \
                after trimming the sectors to the 10 of 40 generated";
    assert_eq!(overflow_reason(text).as_deref(), Some("6 KB over, sectors"));
    assert_eq!(
        refusal_line("zanzibar", &anyhow::anyhow!("{text}")),
        "Zanzibar: 6 KB over, sectors"
    );
    assert_eq!(
        refusal_line("coagulation", &anyhow::anyhow!("no flag stands.\nmore")),
        "Coagulation: no flag stands"
    );
}

#[test]
fn the_builtin_list_is_the_eleven_maps_the_builder_makes_a_kit_for() {
    assert_eq!(
        MAPS,
        [
            "coagulation", "zanzibar", "ascension", "beavercreek", "burial_mounds",
            "colossus", "foundation", "headlong", "lockout", "midship", "waterworks",
        ]
    );
    let targets = targets();
    assert_eq!(targets.len(), MAPS.len(), "every built-in map has a target");
    for (t, m) in targets.iter().zip(MAPS) {
        assert_eq!(t.map, m);
        // The authored two keep their layout; the rest are generated.
        assert_eq!(t.config.layout.is_some(), m == "coagulation" || m == "zanzibar", "{m}");
    }
    for (m, _) in REFUSED {
        assert!(!MAPS.contains(&m), "{m} is both built in and refused");
    }
}

#[test]
fn a_refused_retail_map_is_named_with_its_reason_and_never_read() {
    assert_eq!(refusal("Cyclotron"), Some("no six drop zones: the bases' area has room for 5"));
    assert_eq!(refusal("waterworks"), None);
    assert_eq!(refusal("midship"), None);
    let dir = scratch("retail-refused");
    let _ = std::fs::remove_dir_all(&dir);
    let disc = Disc::new(&[]);
    let reports = ensure(&disc, &dir);
    let lines: Vec<_> = reports.iter().filter_map(Report::line).collect();
    // Every built-in is refused here (the disc is empty); the refused map
    // comes last and says why, in the panel's form.
    assert!(lines.contains(&"Cyclotron: no six drop zones: the bases' area has room for 5".to_owned()), "{lines:?}");
    assert!(!disc.reads.borrow().iter().any(|m| refusal(m).is_some()), "refused maps are not read");
    let order: Vec<_> = reports.iter().map(|r| r.map.as_str()).collect();
    assert_eq!(&order[..MAPS.len()], MAPS);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The disc's maps, read from the image at `H2_DISC` (`disc/halo2.iso`).
struct RetailDisc(RefCell<vfs::AnyDisc>);

impl DiscMaps for RetailDisc {
    fn read_map(&self, name: &str) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut disc = self.0.borrow_mut();
        let file = vfs::Backend::host_file(&mut *disc, &format!("maps\\{name}.map"))
            .ok_or_else(|| anyhow::anyhow!("the disc has no {name}.map"))?;
        let mut reader = std::fs::File::open(&file.path)?;
        reader.seek(SeekFrom::Start(file.offset))?;
        let mut bytes = vec![0; usize::try_from(file.len)?];
        reader.read_exact(&mut bytes)?;
        Ok(bytes)
    }
}

/// The disc-tier check: Waterworks, 164 KB over a retail arena's bound,
/// builds its kit within the grown one (`map::arena::GROWTH`). Run with
/// `cargo xtask test -p firefight-kit -- --ignored waterworks_fits`.
#[test]
#[ignore = "needs the Halo 2 disc (H2_DISC)"]
fn waterworks_fits_the_grown_tag_arena() {
    let path = std::env::var("H2_DISC").unwrap_or_else(|_| "disc/halo2.iso".into());
    let disc = RetailDisc(RefCell::new(
        vfs::open_disc(std::path::Path::new(&path)).expect("the disc opens"),
    ));
    let dir = scratch("waterworks-fits");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let target = target("waterworks").expect("waterworks is built in");
    let reports = ensure_with(&[target], &disc, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        matches!(reports[0].outcome, Outcome::Built { .. }),
        "{:?}",
        reports[0]
    );
}
