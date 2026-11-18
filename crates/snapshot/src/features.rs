//! The `features` section: each feature's own bytes by feature ID, as the
//! feature plan collects them (`feature::Plan::save_sections`, read back by
//! `load_sections`). This crate only stores them.
use crate::Error;
use crate::codec::{Reader, Writer};
use crate::container::{Codec, Snapshot, SnapshotWriter, Tag};

pub const FEATURES: Tag = Tag::new("features");
const VERSION: u32 = 1;

pub fn save_features(out: &mut SnapshotWriter, sections: &[(&str, Vec<u8>)]) -> Result<(), Error> {
    let mut w = Writer::new();
    w.list(sections, |w, (id, bytes)| {
        w.str(id).blob(bytes);
    });
    out.section(FEATURES, VERSION, Codec::Zstd, w.into_bytes())?;
    Ok(())
}

pub fn load_features(snapshot: &Snapshot<'_>) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let bytes = snapshot.require(FEATURES, VERSION..=VERSION)?;
    let mut r = Reader::new(&bytes, "features");
    let out = r.list(8, |r| Ok((r.str()?.to_owned(), r.blob()?.to_vec())))?;
    r.finish()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildKey, Header};

    #[test]
    fn feature_sections_round_trip_in_order() {
        let build = BuildKey {
            stamp: "s".into(),
            switches: String::new(),
        };
        let mut w = SnapshotWriter::new(Header {
            build: build.clone(),
            name: "f".into(),
            tick: 0,
            presents: 0,
            notes: Vec::new(),
        });
        save_features(
            &mut w,
            &[("lockstep", vec![1; 16]), ("h1-health", Vec::new())],
        )
        .unwrap();
        let file = w.finish().unwrap();
        let snapshot = Snapshot::open(&file, &build).unwrap();
        assert_eq!(
            load_features(&snapshot).unwrap(),
            vec![
                ("lockstep".to_string(), vec![1; 16]),
                ("h1-health".to_string(), Vec::new())
            ]
        );
    }
}
