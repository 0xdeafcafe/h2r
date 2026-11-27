//! A part's source index spans in its native index buffer
//! (docs/graphics/native-lights.md, "Visible subparts"). The emitter
//! `0x0004daa0` draws a record's visible subparts as spans of the part's
//! source indices: each its own strip in strip topology, consecutive ones
//! as one list otherwise (`scene_extract::layout::world_ranges::emissions`).
//! The native buffers hold the part's triangles in source order with the
//! degenerate ones dropped (`tags::geometry::strip_triangles`,
//! `list_triangles`) and, for the world, those naming a vertex past the
//! section's (`world::part_indices`). A [`TriangleMap`] counts the kept
//! triangles before each source position, so a span's triangles are one
//! native range.
//!
//! - **List.** Position `k` is the triangle of source indices `3k..3k+3`.
//!   A span must start and end on a triangle.
//! - **Strip.** Position `k` is the window `k..k+3`, wound by `k`'s
//!   parity. A sub-strip from `s` winds its windows by their parity from
//!   `s`, so a span starting at an odd position would draw every triangle
//!   the other way round: it has no range.

use std::ops::Range;

/// Kept triangles before each source position of one part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TriangleMap {
    strip: bool,
    /// `kept[k]`: triangles kept before position `k`; one longer than the
    /// positions.
    kept: Vec<u32>,
    /// Source indices of the part.
    len: u32,
}

impl TriangleMap {
    /// The map of the part's source indices `raw` read as a strip or a
    /// list. With `vertices`, a triangle naming a vertex at or past it is
    /// dropped too, as the world's buffer drops it.
    pub fn new(raw: &[u16], strip: bool, vertices: Option<usize>) -> Self {
        let kept_triangle = |t: &[u16]| {
            let (a, b, c) = (t[0], t[1], t[2]);
            a != b && b != c && a != c && vertices.is_none_or(|n| t.iter().all(|&i| usize::from(i) < n))
        };
        let mut kept = vec![0];
        let mut n = 0;
        let mut push = |keep: bool| {
            n += u32::from(keep);
            kept.push(n);
        };
        if strip {
            raw.windows(3).for_each(|w| push(kept_triangle(w)));
        } else {
            raw.chunks_exact(3).for_each(|t| push(kept_triangle(t)));
        }
        Self { strip, kept, len: raw.len() as u32 }
    }

    /// Triangles the native buffer holds for the part.
    pub fn triangles(&self) -> u32 {
        self.kept.last().copied().unwrap_or(0)
    }

    /// Whether the map describes a native range of `native` indices.
    pub fn fits(&self, native: &Range<u32>) -> bool {
        native.end.checked_sub(native.start) == Some(3 * self.triangles())
    }

    /// The native indices, from the part's native start, that draw the
    /// source span `span` (from the part's first index). `None` for a
    /// span past the part, a list span off a triangle, or a strip span
    /// starting at an odd position. A strip span under three indices
    /// draws nothing.
    pub fn span(&self, span: Range<u32>) -> Option<Range<u32>> {
        if span.start > span.end || span.end > self.len {
            return None;
        }
        let at = |k: u32| self.kept.get(k as usize).map(|&n| 3 * n);
        if self.strip {
            if span.start % 2 != 0 {
                return None;
            }
            if span.end - span.start < 3 {
                let k = at(span.start.min(self.kept.len() as u32 - 1))?;
                return Some(k..k);
            }
            Some(at(span.start)?..at(span.end - 2)?)
        } else {
            if span.start % 3 != 0 || span.end % 3 != 0 {
                return None;
            }
            Some(at(span.start / 3)?..at(span.end / 3)?)
        }
    }

    /// The native ranges, offset by `native_start`, of the absolute source
    /// spans `spans` of a part whose first index is `first`; empty ranges
    /// left out. `None` when one has no range.
    pub fn ranges(&self, first: u32, native_start: u32, spans: &[Range<u32>]) -> Option<Vec<Range<u32>>> {
        let mut out = Vec::with_capacity(spans.len());
        for s in spans {
            let rel = s.start.checked_sub(first)?..s.end.checked_sub(first)?;
            let r = self.span(rel)?;
            if r.is_empty() {
                continue;
            }
            out.push(native_start + r.start..native_start + r.end);
        }
        Some(out)
    }
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;

    /// The native triangles of `raw` as the loaders make them.
    fn native(raw: &[u16], strip: bool) -> Vec<[u16; 3]> {
        if strip { tags::geometry::strip_triangles(raw) } else { tags::geometry::list_triangles(raw) }
    }

    /// A span's triangles drawn as their own strip or list.
    fn drawn(raw: &[u16], span: Range<u32>, strip: bool) -> Vec<[u16; 3]> {
        native(&raw[span.start as usize..span.end as usize], strip)
    }

    fn flat(t: &[[u16; 3]]) -> Vec<u16> {
        t.iter().flatten().copied().collect()
    }

    #[test]
    fn a_strip_with_degenerates_maps_each_even_sub_strip_to_its_own_native_triangles() {
        // Two stitched sub-strips: 0..6 and 6..12, the stitch's windows
        // degenerate (5, 5) and a degenerate inside the second.
        let raw = [0, 1, 2, 3, 4, 5, 5, 6, 6, 7, 8, 8, 9, 10];
        let map = TriangleMap::new(&raw, true, None);
        let all = flat(&native(&raw, true));
        assert!(map.fits(&(0..all.len() as u32)));
        for span in [0..6, 6..12, 0..14, 8..14, 2..5] {
            let r = map.span(span.clone()).unwrap_or_else(|| panic!("{span:?}"));
            assert_eq!(
                all[r.start as usize..r.end as usize],
                flat(&drawn(&raw, span.clone(), true)),
                "{span:?}: the sub-strip's own triangles, wound the same"
            );
        }
        assert_eq!(map.span(1..6), None, "an odd start winds the other way");
        assert!(map.span(4..6).unwrap().is_empty(), "under three indices: nothing");
        assert_eq!(map.span(0..15), None, "past the part");
    }

    #[test]
    fn a_list_with_degenerates_and_far_vertices_maps_whole_triangles_only() {
        let raw = [0, 1, 2, 3, 3, 4, 5, 6, 7, 8, 9, 20, 10, 11, 12];
        // The world also drops triangle 3, naming vertex 20 of 13.
        let map = TriangleMap::new(&raw, false, Some(13));
        let kept: Vec<[u16; 3]> = native(&raw, false).into_iter().filter(|t| t.iter().all(|&i| i < 13)).collect();
        let all = flat(&kept);
        assert!(map.fits(&(0..all.len() as u32)));
        assert_eq!(map.triangles(), 3);
        for (span, want) in [(0..6, vec![0, 1, 2]), (3..9, vec![5, 6, 7]), (9..15, vec![10, 11, 12]), (3..6, vec![])] {
            let r = map.span(span.clone()).unwrap();
            assert_eq!(all[r.start as usize..r.end as usize], want, "{span:?}");
        }
        assert_eq!(map.span(1..6), None, "off a triangle");
        assert_eq!(
            map.ranges(100, 40, &[100..106, 103..106, 109..115]),
            Some(vec![40..43, 46..49]),
            "absolute spans, the empty one left out"
        );
        assert_eq!(map.ranges(100, 40, &[99..106]), None, "before the part");
        assert!(!TriangleMap::new(&raw, false, None).fits(&(0..all.len() as u32)), "a map that is not the buffer's");
    }
}
