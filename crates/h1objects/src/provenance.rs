//! Tracks fields that depend on native Halo 2 data. A builder records a
//! field path and value while constructing one tag. The converter keeps
//! those notes on its tree. The audit in tests/role_audit_disc.rs checks
//! which values survive later writes from the source and classifies them.
//!
//! References taken from a Halo 2 counterpart are not noted here: they are
//! found in the built tags themselves (any reference to a base-map tag).

use std::cell::RefCell;

use tags::defs::Val;

/// One value a builder took from a role.
#[derive(Clone, Debug, PartialEq)]
pub struct RoleField {
    /// Path in the tag's record (`barrels[0].unknown_4`, the syntax of
    /// [`tags::defs::Record::at`]); `[*]` stands for every element of a
    /// block when the builder does not know the element's index.
    pub path: String,
    /// The role's value as written.
    pub value: Val,
}

thread_local! {
    static FRAMES: RefCell<Vec<Vec<RoleField>>> = const { RefCell::new(Vec::new()) };
}

/// Runs `f` and returns what it noted ([`note`]). Captures nest: notes go
/// to the innermost, so a tag built while another is being built keeps
/// its own.
pub fn capture<T>(f: impl FnOnce() -> T) -> (T, Vec<RoleField>) {
    FRAMES.with(|s| s.borrow_mut().push(Vec::new()));
    let out = f();
    let got = FRAMES.with(|s| s.borrow_mut().pop()).unwrap_or_default();
    (out, got)
}

/// Notes that the field at `path` took `value` from a role. Nothing is kept
/// outside a [`capture`].
pub fn note(path: impl Into<String>, value: &Val) {
    FRAMES.with(|s| {
        if let Some(top) = s.borrow_mut().last_mut() {
            top.push(RoleField {
                path: path.into(),
                value: value.clone(),
            });
        }
    });
}

/// Notes role words written under `scope` (a path prefix ending in `.`,
/// or empty for the root).
pub fn note_words<P: AsRef<str>>(scope: &str, words: &[(P, u32)]) {
    for (p, w) in words {
        note(
            format!("{scope}{}", p.as_ref()),
            &Val::Bytes(w.to_le_bytes().to_vec()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_go_to_the_innermost_capture_and_none_outside() {
        note("lost", &Val::Int(1));
        let ((), outer) = capture(|| {
            note("a", &Val::Int(1));
            let ((), inner) = capture(|| note_words("b[0].", &[("unknown_4", 7u32)]));
            assert_eq!(
                inner,
                vec![RoleField {
                    path: "b[0].unknown_4".into(),
                    value: Val::Bytes(7u32.to_le_bytes().to_vec()),
                }]
            );
            note("c", &Val::Int(2));
        });
        let paths: Vec<_> = outer.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a", "c"]);
    }

    #[test]
    fn damage_encodes_the_source_motor_fade_and_duration() {
        use tags::defs::{self, Defs, Val};
        use tags::effects::function::Function;
        let mut bytes = vec![0; crate::arsenal::H1_JPT_MIN];
        for (at, value) in [(0x5c, 0.75_f32), (0x60, 0.2), (0x70, 0.25), (0x74, 0.5)] {
            bytes[at..at+4].copy_from_slice(&value.to_le_bytes());
        }
        let root = crate::arsenal::damage(&bytes, None).unwrap();
        let r = defs::from_built(Defs::builtin().group(map::FourCC(*b"jpt!")).unwrap(), &root).unwrap();
        let Some(Val::Block(responses)) = r.at("block_6c") else { panic!("no player responses") };
        assert_eq!(responses.len(), 3);
        for (i, response) in responses.iter().enumerate() {
            assert_eq!(response.at("unknown_0"), Some(&Val::Bytes((i as u32).to_le_bytes().to_vec())));
            for (path, time, magnitude, duration) in [("data_28", "unknown_24", 0.75, 0.2f32),
                                                       ("data_34", "unknown_30", 0.25, 0.5)] {
                assert_eq!(response.at(time), Some(&Val::Bytes(duration.to_le_bytes().to_vec())));
                let Some(Val::Data(data)) = response.at(path) else { panic!("{path}") };
                let envelope = Function::parse(data).unwrap();
                assert_eq!(envelope.evaluate(0.0, 0.0), magnitude);
                assert_eq!(envelope.evaluate(1.0, 0.0), 0.0);
            }
        }
    }

    #[test]
    fn damage_radius_and_cutoff_come_from_halo1() {
        let mut b = vec![0; crate::arsenal::H1_JPT_MIN];
        for (i, v) in [0.2f32, 3.5, 0.75].into_iter().enumerate() {
            b[i*4..i*4+4].copy_from_slice(&v.to_le_bytes());
        }
        let (root, notes) = capture(|| crate::arsenal::damage(&b, None));
        assert!(notes.is_empty());
        assert_eq!(&root.unwrap().bytes[..12], &b[..12]);
    }
}
