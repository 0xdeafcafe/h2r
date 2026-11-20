//! The Resolution picker takes a snapshot of the drawable on open. The
//! selected Native value continues to follow the drawable as it changes.

use settings::Resolution;

/// Space in the model's locale table for the current row value and its NUL.
/// The row rewrites this slot when the drawable or selection changes.
pub(crate) const VALUE_SLOT: &str = "                                ";

pub(crate) fn label(choice: Resolution, native: (u32, u32)) -> String {
    match choice {
        Resolution::Native => format!("Native ({}x{})", native.0, native.1),
        _ => choice.to_string(),
    }
}

/// The narrow value column shows the physical Native size. The picker's
/// help has room for the automatic raster and the full-size alternative.
pub(crate) fn help(native: (u32, u32)) -> String {
    let (w, h) = Resolution::Native.size(native);
    if (w, h) == native {
        return crate::setting_defs::RESOLUTION.help.into();
    }
    format!(
        "Native renders at {w}x{h}. Pick {}x{} for full resolution.",
        native.0, native.1
    )
}

pub(crate) fn choices(native: (u32, u32), current: Resolution) -> Vec<Resolution> {
    let (w, h) = native;
    let mut sizes = Vec::new();
    if w > 0 && h > 0 {
        // Use familiar widths on 16:10 and 4:3 displays (1280x800,
        // 1600x1200). Wide displays keep common scan-line counts.
        if u64::from(w) * 9 >= u64::from(h) * 16 {
            for height in [720, 1080, 1440, 2160, 2880, 4320] {
                if height > h {
                    break;
                }
                let width = (u64::from(w) * u64::from(height) + u64::from(h / 2)) / u64::from(h);
                sizes.extend(Resolution::fixed(width as u32, height));
            }
        } else {
            for width in [
                640, 800, 1024, 1280, 1600, 1920, 2048, 2560, 3200, 3840, 5120,
            ] {
                if width > w {
                    break;
                }
                let height = (u64::from(h) * u64::from(width) + u64::from(w / 2)) / u64::from(w);
                sizes.extend(Resolution::fixed(width, height as u32));
            }
        }
        sizes.extend(Resolution::fixed(w, h));
    }
    // An explicit launch size may belong to another display. Keep it
    // selectable with the correct focus after a move or resize.
    if current != Resolution::Native {
        sizes.push(current);
    }
    sizes.sort_by_key(|r| r.size(native));
    sizes.dedup();
    sizes.insert(0, Resolution::Native);
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_display_has_native_then_five_standard_sizes() {
        let native = (5120, 2880);
        let labels: Vec<_> = choices(native, Resolution::Native)
            .into_iter()
            .map(|r| label(r, native))
            .collect();
        assert_eq!(
            labels,
            [
                "Native (5120x2880)",
                "1280x720",
                "1920x1080",
                "2560x1440",
                "3840x2160",
                "5120x2880"
            ]
        );
        assert_eq!(
            help(native),
            "Native renders at 3840x2160. Pick 5120x2880 for full resolution."
        );
        assert_eq!(help((3440, 1440)), crate::setting_defs::RESOLUTION.help);
        assert!(label(Resolution::Native, native).len() < VALUE_SLOT.len());
    }

    #[test]
    fn sizes_fit_each_drawable_and_keep_the_selected_custom_value() {
        for native in [
            (3440, 1440),
            (5120, 1440),
            (2560, 1600),
            (1024, 768),
            (801, 603),
            (300, 800),
        ] {
            let sizes = choices(native, Resolution::Native);
            assert_eq!(sizes[0], Resolution::Native);
            assert_eq!(sizes.last().unwrap().size(native), native);
            for size in &sizes[1..] {
                let (w, h) = size.size(native);
                assert!(w <= native.0 && h <= native.1);
                let error =
                    (i64::from(w) * i64::from(native.1) - i64::from(h) * i64::from(native.0)).abs();
                assert!(error <= i64::from(native.0.max(native.1)) / 2);
            }
        }
        let custom = Resolution::Fixed(1278, 800);
        assert_eq!(choices((800, 600), custom).last(), Some(&custom));
    }
}
