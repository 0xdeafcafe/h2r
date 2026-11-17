//! Windows side by side: `h2 play --tile I/N` (`h2 coop test` gives each
//! game its column, the host leftmost; docs/live/live-coop.md section 12).

/// Column `index` of `count` across a screen of `screen` logical points
/// (width, height): the largest 4:3 window that fits in the column under
/// a `top` margin (the menu bar), centred in it; `(x, y, width, height)`.
/// `None` for an index outside the count.
pub fn tile(screen: (f64, f64), top: f64, index: u32, count: u32) -> Option<(f64, f64, f64, f64)> {
    if count == 0 || index >= count {
        return None;
    }
    let col = screen.0 / f64::from(count);
    let room = (screen.1 - top).max(120.0);
    // The window's title bar takes about 28 points of the room.
    let w = col.min((room - 28.0).max(90.0) * 4.0 / 3.0).max(160.0);
    let h = (w * 3.0 / 4.0).max(120.0);
    let x = col * f64::from(index) + (col - w).max(0.0) / 2.0;
    Some((x.floor(), top, w.floor(), h.floor()))
}

/// Parses `I/N` (`0/2`, `1/2`): column I of N, from 0.
pub fn parse(s: &str) -> Option<(u32, u32)> {
    let (i, n) = s.split_once('/')?;
    let (i, n) = (i.trim().parse().ok()?, n.trim().parse().ok()?);
    (n > 0 && i < n && n <= 16).then_some((i, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_side_by_side_host_leftmost() {
        // A 1512x982 laptop screen, three games.
        let s = (1512.0, 982.0);
        let a = tile(s, 40.0, 0, 3).unwrap();
        let b = tile(s, 40.0, 1, 3).unwrap();
        let c = tile(s, 40.0, 2, 3).unwrap();
        assert_eq!(a, (0.0, 40.0, 504.0, 378.0));
        assert_eq!(b.0, 504.0);
        assert_eq!(c.0, 1008.0);
        assert!(c.0 + c.2 <= s.0);
        // Two on a short, wide screen: limited by the height, centred.
        let (x, y, w, h) = tile((2560.0, 600.0), 40.0, 1, 2).unwrap();
        assert_eq!((y, w, h), (40.0, 709.0, 532.0));
        assert_eq!(x, 1280.0 + ((1280.0f64 - 709.333).max(0.0) / 2.0).floor());
        assert_eq!(tile(s, 40.0, 3, 3), None);
        assert_eq!(tile(s, 40.0, 0, 0), None);
    }

    #[test]
    fn parses_column_of_count() {
        assert_eq!(parse("0/2"), Some((0, 2)));
        assert_eq!(parse("2/3"), Some((2, 3)));
        assert_eq!(parse("3/3"), None);
        assert_eq!(parse("1/0"), None);
        assert_eq!(parse("a/2"), None);
        assert_eq!(parse("2"), None);
    }
}
