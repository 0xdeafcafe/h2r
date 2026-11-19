//! Which fields of a converted tag were filled from its **role** (the
//! base map's retail Halo 2 tags of the same kind: majority words, a
//! counterpart's values, or retail values kept in the source) rather than
//! from Halo 1.
//!
//! The builders ([`crate::rebuild`], [`crate::arsenal`],
//! [`crate::particles`], the garbage builder) note every value they take
//! from a role, by its path in the tag's record, as they write it; the
//! converter collects the notes made while it builds one tag's root
//! ([`capture`]) and keeps them with the tag ([`crate::convert::Tree::role_fields`]).
//! A later write from Halo 1 to the same field is not un-noted: the audit
//! (`tests/role_audit_disc.rs`, `docs/halo1/role-audit.md`) compares each
//! note with the built tag and reports only the values still the role's.
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

    /// A builder notes its role words by their paths in the tag, and the
    /// built tag still holds them where Halo 1 wrote nothing.
    #[test]
    fn a_damage_effect_notes_its_role_words() {
        let role = crate::arsenal::Role {
            members: 1,
            root: vec![
                ("unknown_0".into(), 2.5f32.to_bits()),
                ("unknown_c".into(), 3),
            ],
            elements: Vec::new(),
        };
        let b = vec![0u8; crate::arsenal::H1_JPT_MIN];
        let (root, got) = capture(|| crate::arsenal::damage(&b, &role));
        let paths: Vec<_> = got.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["unknown_0", "unknown_c"]);
        let d = tags::defs::Defs::builtin()
            .group(map::FourCC(*b"jpt!"))
            .unwrap();
        let r = tags::defs::from_built(d, &root.unwrap()).unwrap();
        assert_eq!(r.at("unknown_0"), Some(&got[0].value));
    }
}
