//! The development-build watermark: two lines of small text along the
//! bottom edge of the game image, drawn by the shell at present time and
//! never written into guest memory.
//!
//! ```text
//! h2recomp  ·  development build  ·  a1987da master  ·  built 2026-11-15 14:02 UTC
//! fps 28.4  ·  35.2 ms (cpu 24 / gfx 5 / wait 6)  ·  draws 1104  ·  map 01a_tutorial  ·  metal, render thread on
//! ```
//!
//! The first line is baked at compile time (`build.rs`: commit, branch,
//! build time). The second is refreshed a few times a second from the
//! frame stats' rolling window (`timeline::phase`), or from the shell's
//! present count when frame stats are off. Both lines are rasterised on
//! the CPU into one texture only when the text changes, and drawn as one
//! alpha-blended quad. The letters are the public-domain `font8x8` set the
//! banner uses; the layout and wording are our own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::overlay::Sprite;
use crate::scale::Viewport;

/// What the binary was built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildInfo {
    /// Short commit hash, or `unknown`.
    pub sha: &'static str,
    /// Branch, `detached`, or `unknown`.
    pub branch: &'static str,
    /// Build time, seconds since the Unix epoch.
    pub unix: u64,
}

impl BuildInfo {
    /// The values `build.rs` baked into this crate.
    pub fn baked() -> Self {
        Self {
            sha: env!("H2_BUILD_SHA"),
            branch: env!("H2_BUILD_BRANCH"),
            unix: env!("H2_BUILD_UNIX").parse().unwrap_or(0),
        }
    }

    /// The centred build label: name, then commit, branch and build time.
    pub fn label(&self) -> Vec<String> {
        vec![
            "h2recomp development build".into(),
            format!("{} {}{SEP}{}", self.sha, self.branch, utc_minute(self.unix)),
        ]
    }
}

/// Between fields.
const SEP: &str = "  \u{b7}  ";

/// `secs` since the epoch as `YYYY-MM-DD HH:MM UTC`.
pub fn utc_minute(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days to civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// The live numbers on the second line. Phase splits and draws are there
/// only while frame stats are collected.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LiveStats {
    pub fps: f64,
    /// Mean wall time per frame.
    pub frame_ms: f64,
    /// Game thread: guest code, runtime, kernel, HLE (`cpu`).
    pub cpu_ms: Option<f64>,
    /// Graphics work on whichever thread does it: push-buffer decode,
    /// wgpu encode, submit and GPU round trips (`gpu-wait`). With the
    /// render thread on, most of it overlaps the game thread.
    pub gfx_ms: Option<f64>,
    /// The game thread waiting for the render thread (`render-wait`).
    pub wait_ms: Option<f64>,
    /// Draws per frame.
    pub draws: Option<f64>,
}

impl LiveStats {
    /// From the frame stats' rolling window.
    pub fn from_summary(s: &timeline::phase::Summary) -> Self {
        use timeline::phase::{Counter, Phase};
        let p = |ph: Phase| s.phase_ms[ph as usize];
        Self {
            fps: s.fps,
            frame_ms: if s.fps > 0.0 { 1000.0 / s.fps } else { 0.0 },
            cpu_ms: Some(p(Phase::Guest) + p(Phase::Render) + p(Phase::D3d)),
            gfx_ms: Some(p(Phase::Decode) + p(Phase::Encode) + p(Phase::Submit) + p(Phase::Wait)),
            wait_ms: Some(p(Phase::RenderWait)),
            draws: Some(s.count_mean[Counter::Draws as usize]),
        }
    }

    /// From a present count alone: `frames` presented in `secs`.
    pub fn from_rate(frames: u64, secs: f64) -> Option<Self> {
        if frames == 0 || secs <= 0.0 {
            return None;
        }
        let fps = frames as f64 / secs;
        Some(Self {
            fps,
            frame_ms: 1000.0 / fps,
            ..Default::default()
        })
    }
}

/// The live-run panel, one fact a line. `renderer` is the backend and how
/// it runs (`metal, render thread on`).
pub fn stats_lines(stats: Option<&LiveStats>, map: Option<&str>, renderer: &str) -> Vec<String> {
    let mut f: Vec<String> = Vec::new();
    match stats {
        Some(s) => {
            f.push(format!("{:.1} fps{SEP}{:.1} ms", s.fps, s.frame_ms));
            if let (Some(c), Some(g), Some(w)) = (s.cpu_ms, s.gfx_ms, s.wait_ms) {
                f.push(format!("cpu {c:.0}{SEP}gfx {g:.0}{SEP}wait {w:.0}"));
            }
            if let Some(d) = s.draws {
                f.push(format!("draws {d:.0}"));
            }
        }
        None => f.push("-- fps".into()),
    }
    if let Some(m) = map {
        f.push(format!("map {m}"));
    }
    if !renderer.is_empty() {
        f.push(renderer.into());
    }
    f
}

/// `text` cut to `max` characters, ending in `..` when cut.
pub fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.into();
    }
    if max <= 2 {
        return ".".repeat(max);
    }
    text.chars().take(max - 2).chain("..".chars()).collect()
}

type MapFn = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Shared handle to the window's watermark: on or off (F3), what it says.
/// Cloning shares the on/off state.
#[derive(Clone)]
pub struct Watermark {
    visible: Arc<AtomicBool>,
    build: BuildInfo,
    note: String,
    map: Option<MapFn>,
}

impl std::fmt::Debug for Watermark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watermark")
            .field("visible", &self.visible())
            .field("build", &self.build)
            .field("note", &self.note)
            .finish()
    }
}

impl Default for Watermark {
    fn default() -> Self {
        Self::new()
    }
}

impl Watermark {
    /// Shown, with this binary's [`BuildInfo::baked`].
    pub fn new() -> Self {
        Self {
            visible: Arc::new(AtomicBool::new(true)),
            build: BuildInfo::baked(),
            note: String::new(),
            map: None,
        }
    }

    pub fn with_build(mut self, build: BuildInfo) -> Self {
        self.build = build;
        self
    }

    /// Said after the backend name (`render thread on`).
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    /// Asked for the current map at each refresh; must be cheap.
    pub fn with_map(mut self, map: impl Fn() -> Option<String> + Send + Sync + 'static) -> Self {
        self.map = Some(Arc::new(map));
        self
    }

    pub fn visible(&self) -> bool {
        self.visible.load(Ordering::Relaxed)
    }

    pub fn set_visible(&self, on: bool) {
        self.visible.store(on, Ordering::Relaxed);
    }

    /// Flips it; returns the new state.
    pub fn toggle(&self) -> bool {
        !self.visible.fetch_xor(true, Ordering::Relaxed)
    }

    /// The on/off switch itself, shared with whoever else flips it (the
    /// debug menu's `perf.overlay`).
    pub fn switch(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.visible)
    }

    /// Both panels, with `backend` (`metal`) as the renderer.
    pub fn panels(&self, stats: Option<&LiveStats>, backend: &str) -> Panels {
        let map = self.map.as_ref().and_then(|m| m());
        let renderer = match (backend.is_empty(), self.note.is_empty()) {
            (false, false) => format!("{backend}, {}", self.note),
            (false, true) => backend.into(),
            (true, _) => self.note.clone(),
        };
        Panels {
            label: self.build.label(),
            live: stats_lines(stats, map.as_deref(), &renderer),
        }
    }
}

/// What the watermark shows: a small build label centred near the bottom of
/// the image, and the live run's numbers in its lower right.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Panels {
    pub label: Vec<String>,
    pub live: Vec<String>,
}

/// How a panel's lines line up, and where the panel sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// Centred, near the bottom edge.
    Centre,
    /// Right-aligned, in the lower right corner.
    Right,
}

/// Pixels per font pixel for a target `height` pixels tall: 1 up to 1599,
/// 2 on a Retina-sized window, at most 4.
pub fn scale_for(height: u32) -> u32 {
    (height / 800).clamp(1, 4)
}

/// Glyph cell, in font pixels.
pub(crate) const CELL: u32 = 8;
/// Line pitch, in font pixels.
pub(crate) const PITCH: u32 = 10;
/// Room for the shadow around the text, in font pixels.
const PAD: u32 = 2;
/// Distance from the image's left and bottom edges, in font pixels.
const MARGIN: u32 = 4;

const TEXT_RGB: [f32; 3] = [236.0, 240.0, 244.0];
const TEXT_ALPHA: f32 = 0.72;
const SHADOW_ALPHA: f32 = 0.6;

pub(crate) fn glyph(c: char) -> [u8; 8] {
    use font8x8::UnicodeFonts;
    font8x8::BASIC_FONTS
        .get(c)
        .or_else(|| font8x8::LATIN_FONTS.get(c))
        .or_else(|| font8x8::BASIC_FONTS.get('?'))
        .unwrap_or([0; 8])
}

/// `lines` as straight-alpha RGBA at `scale`: (width, height, pixels).
/// Light text, mostly opaque, over a soft dark shadow one font pixel down
/// and right; transparent elsewhere.
pub fn rasterize(lines: &[String], scale: u32) -> (u32, u32, Vec<u8>) {
    rasterize_aligned(lines, scale, None)
}

/// [`rasterize`] with each line lined up inside the block: `None` is left.
pub fn rasterize_aligned(lines: &[String], scale: u32, align: Option<Align>) -> (u32, u32, Vec<u8>) {
    let s = scale.max(1);
    let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u32;
    let lines: Vec<String> = lines
        .iter()
        .map(|l| {
            let gap = cols as usize - l.chars().count();
            match align {
                None => l.clone(),
                Some(Align::Right) => " ".repeat(gap) + l,
                Some(Align::Centre) => " ".repeat(gap / 2) + l,
            }
        })
        .collect();
    let rows = lines.len() as u32;
    if cols == 0 || rows == 0 {
        return (0, 0, Vec::new());
    }
    let w = (cols * CELL + 2 * PAD) * s;
    let h = ((rows - 1) * PITCH + CELL + 2 * PAD) * s;
    let mut mask = vec![false; (w * h) as usize];
    for (r, line) in lines.iter().enumerate() {
        for (i, c) in line.chars().enumerate() {
            for (gy, bits) in glyph(c).iter().enumerate() {
                for gx in 0..CELL {
                    if bits & (1 << gx) == 0 {
                        continue;
                    }
                    let x0 = (PAD + i as u32 * CELL + gx) * s;
                    let y0 = (PAD + r as u32 * PITCH + gy as u32) * s;
                    for dy in 0..s {
                        for dx in 0..s {
                            mask[((y0 + dy) * w + x0 + dx) as usize] = true;
                        }
                    }
                }
            }
        }
    }
    let lit = |x: i64, y: i64| {
        x >= 0 && y >= 0 && x < w as i64 && y < h as i64 && mask[(y * w as i64 + x) as usize]
    };
    let mut px = vec![0u8; (w * h * 4) as usize];
    let si = s as i64;
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            // Shadow: the mask moved one font pixel down-right, blurred
            // with a 3x3 binomial kernel one font pixel wide.
            let (sx, sy) = (x - si, y - si);
            let mut acc = 0u32;
            for (ky, wy) in [(-1i64, 1u32), (0, 2), (1, 1)] {
                for (kx, wx) in [(-1i64, 1u32), (0, 2), (1, 1)] {
                    if lit(sx + kx * si, sy + ky * si) {
                        acc += wx * wy;
                    }
                }
            }
            let a_s = SHADOW_ALPHA * acc as f32 / 16.0;
            let a_t = if lit(x, y) { TEXT_ALPHA } else { 0.0 };
            let a = a_t + a_s * (1.0 - a_t);
            if a <= 0.0 {
                continue;
            }
            let o = ((y * w as i64 + x) * 4) as usize;
            // Straight alpha: the shadow is black, so only the text
            // contributes colour.
            for (d, c) in px[o..o + 3].iter_mut().zip(TEXT_RGB) {
                *d = (c * a_t / a).round() as u8;
            }
            px[o + 3] = (a * 255.0).round() as u8;
        }
    }
    (w, h, px)
}

/// Where the watermark goes inside `area` (the game image) at `scale`, and
/// `lines` cut to fit its width.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub lines: Vec<String>,
    pub scale: u32,
    /// Top-left of the texture in the target.
    pub at: (u32, u32),
}

/// Where a panel of `lines` goes inside `area` (the game image), lined up
/// as `align`, [`MARGIN`] font pixels from the bottom (and right) edge;
/// lines are cut to the panel's third of the image (the middle third for
/// the label, the right third for the live panel).
pub fn place(lines: &[String], area: Viewport, align: Align) -> Placement {
    let scale = scale_for(area.height);
    let margin = MARGIN * scale;
    // Each panel keeps to its third of the image, so they never overlap.
    let share = area.width / 3;
    let room = share.saturating_sub(2 * margin) / scale;
    let cols = (room.saturating_sub(2 * PAD) / CELL) as usize;
    let lines: Vec<String> = lines.iter().map(|l| fit(l, cols)).collect();
    let rows = lines.len() as u32;
    let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u32;
    let w = (widest * CELL + 2 * PAD) * scale;
    let h = (rows.saturating_sub(1) * PITCH + CELL + 2 * PAD) * scale;
    let x = match align {
        Align::Centre => area.x + area.width.saturating_sub(w) / 2,
        Align::Right => (area.x + area.width).saturating_sub(margin.saturating_sub(PAD * scale) + w),
    };
    let y = (area.y + area.height).saturating_sub(margin.saturating_sub(PAD * scale) + h);
    Placement {
        lines,
        scale,
        at: (x.max(area.x), y.max(area.y)),
    }
}

/// Draws the watermark over an already composited target: one sprite a
/// panel.
pub struct WatermarkRenderer {
    label: PanelSprite,
    live: PanelSprite,
}

struct PanelSprite {
    sprite: Sprite,
    /// The text and scale last rasterised.
    drawn: Option<(Vec<String>, u32)>,
}

impl PanelSprite {
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        lines: &[String],
        align: Align,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        dst_size: (u32, u32),
        area: Viewport,
    ) {
        if lines.is_empty() {
            return;
        }
        let p = place(lines, area, align);
        let key = (p.lines, p.scale);
        if self.drawn.as_ref() != Some(&key) {
            let (w, h, px) = rasterize_aligned(&key.0, key.1, Some(align));
            self.sprite.upload(device, queue, (w, h), &px);
            self.drawn = Some(key);
        }
        self.sprite
            .draw(device, encoder, dst, dst_format, dst_size, p.at);
    }
}

impl WatermarkRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            label: PanelSprite {
                sprite: Sprite::new(device, "app watermark label"),
                drawn: None,
            },
            live: PanelSprite {
                sprite: Sprite::new(device, "app watermark live"),
                drawn: None,
            },
        }
    }

    /// Records the passes drawing `panels` over `area` (the game image) in
    /// `dst`. Re-rasterises a panel only when its text changed.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        panels: &Panels,
        dst: &wgpu::TextureView,
        dst_format: wgpu::TextureFormat,
        dst_size: (u32, u32),
        area: Viewport,
    ) {
        let area = if area.is_empty() {
            Viewport {
                x: 0,
                y: 0,
                width: dst_size.0,
                height: dst_size.1,
            }
        } else {
            area
        };
        self.label.draw(
            device, queue, encoder, &panels.label, Align::Centre, dst, dst_format, dst_size, area,
        );
        self.live.draw(
            device, queue, encoder, &panels.live, Align::Right, dst, dst_format, dst_size, area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: BuildInfo = BuildInfo {
        sha: "a1987da",
        branch: "debug-watermark",
        unix: 1_794_752_520, // 2026-11-15 14:22 UTC
    };

    #[test]
    fn dates_are_utc_civil_dates() {
        assert_eq!(utc_minute(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc_minute(951_825_600), "2000-02-29 12:00 UTC");
        assert_eq!(utc_minute(BUILD.unix), "2026-11-15 14:22 UTC");
        assert_eq!(utc_minute(4_102_444_799), "2099-12-31 23:59 UTC");
    }

    #[test]
    fn the_build_label_names_the_commit_branch_and_time() {
        assert_eq!(
            BUILD.label(),
            vec![
                "h2recomp development build".to_string(),
                "a1987da debug-watermark  \u{b7}  2026-11-15 14:22 UTC".to_string()
            ]
        );
        let baked = BuildInfo::baked();
        assert!(!baked.sha.is_empty() && !baked.branch.is_empty());
        assert!(baked.unix > 1_700_000_000, "{}", baked.unix);
    }

    fn summary() -> timeline::phase::Summary {
        let mut count_mean = [0.0; timeline::phase::COUNTERS];
        count_mean[timeline::phase::Counter::Draws as usize] = 1104.4;
        timeline::phase::Summary {
            frames: 140,
            fps: 28.4,
            p50_ms: 33.0,
            p99_ms: 40.0,
            max_ms: 51.0,
            // cpu, decode, encode, submit, gpu-wait, pace, render-wait,
            // render, d3d (guest time is cpu + render + d3d)
            phase_ms: [18.2, 0.4, 2.6, 1.0, 0.8, 0.3, 6.4, 4.0, 2.0],
            count_mean,
        }
    }

    #[test]
    fn the_live_panel_with_frame_stats() {
        let s = LiveStats::from_summary(&summary());
        assert_eq!(
            stats_lines(Some(&s), Some("01a_tutorial"), "metal, render thread on"),
            vec![
                "28.4 fps  \u{b7}  35.2 ms",
                "cpu 24  \u{b7}  gfx 5  \u{b7}  wait 6",
                "draws 1104",
                "map 01a_tutorial",
                "metal, render thread on",
            ]
        );
    }

    #[test]
    fn the_live_panel_without_frame_stats_or_frames() {
        let s = LiveStats::from_rate(5, 2.5).unwrap();
        assert_eq!(
            stats_lines(Some(&s), None, "metal"),
            vec!["2.0 fps  \u{b7}  500.0 ms", "metal"]
        );
        assert_eq!(LiveStats::from_rate(0, 1.0), None);
        assert_eq!(stats_lines(None, Some("mainmenu"), ""), vec!["-- fps", "map mainmenu"]);
    }

    #[test]
    fn the_watermark_joins_backend_note_and_map() {
        let wm = Watermark::new()
            .with_build(BUILD)
            .with_note("render thread off")
            .with_map(|| Some("mainmenu".into()));
        let p = wm.panels(None, "metal");
        assert_eq!(p.label, BUILD.label());
        assert_eq!(p.live, vec!["-- fps", "map mainmenu", "metal, render thread off"]);
        assert!(wm.visible());
        let shared = wm.clone();
        assert!(!shared.toggle());
        assert!(!wm.visible());
        assert!(wm.toggle());
    }

    #[test]
    fn long_lines_are_cut_to_the_panel_width() {
        assert_eq!(fit("abcdef", 6), "abcdef");
        assert_eq!(fit("abcdefg", 6), "abcd..");
        assert_eq!(fit("abc", 1), ".");
        let lines = vec!["x".repeat(200), "short".into()];
        let area = Viewport {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        };
        let p = place(&lines, area, Align::Right);
        assert_eq!(p.scale, 1);
        // A panel takes at most a third of the image.
        let cols = (640 / 3 - 2 * MARGIN - 2 * PAD) / CELL;
        assert_eq!(p.lines[0].chars().count(), cols as usize);
        assert!(p.lines[0].ends_with(".."));
        assert_eq!(p.lines[1], "short");
    }

    #[test]
    fn the_label_is_centred_and_the_live_panel_sits_lower_right() {
        let lines = vec!["abcd".to_string(), "ab".to_string()];
        // A pillarboxed 4:3 image in a 2560x1600 window: scale 2.
        let area = Viewport {
            x: 213,
            y: 0,
            width: 2133,
            height: 1600,
        };
        let c = place(&lines, area, Align::Centre);
        assert_eq!(c.scale, 2);
        let (w, h, _) = rasterize_aligned(&c.lines, c.scale, Some(Align::Centre));
        assert_eq!(h, (PITCH + CELL + 2 * PAD) * 2);
        let mid = c.at.0 + w / 2;
        assert!(mid.abs_diff(area.x + area.width / 2) <= 1, "centre {mid}");
        assert_eq!(c.at.1 + h - PAD * 2, area.height - MARGIN * 2);
        let r = place(&lines, area, Align::Right);
        let (w, h, _) = rasterize_aligned(&r.lines, r.scale, Some(Align::Right));
        // Text ends MARGIN font pixels in from the right and bottom edges
        // (the texture's PAD of shadow room sits outside that).
        assert_eq!(r.at.0 + w - PAD * 2, area.x + area.width - MARGIN * 2);
        assert_eq!(r.at.1 + h - PAD * 2, area.height - MARGIN * 2);
        assert_eq!(scale_for(960), 1);
        assert_eq!(scale_for(1920), 2);
        assert_eq!(scale_for(10_000), 4);
    }

    #[test]
    fn short_lines_line_up_right_or_centre() {
        let lines = vec!["IIII".to_string(), "I".to_string()];
        let col_lit = |align, col: u32| {
            let (w, h, px) = rasterize_aligned(&lines, 1, Some(align));
            // The second line's row band.
            (PAD + PITCH..(PAD + PITCH + CELL).min(h)).any(|y| {
                (PAD + col * CELL..PAD + (col + 1) * CELL)
                    .any(|x| px[((y * w + x) * 4 + 3) as usize] as f32 >= TEXT_ALPHA * 255.0 - 1.0)
            })
        };
        assert!(col_lit(Align::Right, 3) && !col_lit(Align::Right, 0));
        assert!(col_lit(Align::Centre, 1) && !col_lit(Align::Centre, 3));
    }

    #[test]
    fn rasterised_text_is_light_over_a_soft_shadow() {
        let (w, h, px) = rasterize(&["I".into()], 1);
        assert_eq!((w, h), (CELL + 2 * PAD, CELL + 2 * PAD));
        let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..][..4];
        // Corners are clear.
        assert_eq!(at(0, 0), &[0, 0, 0, 0]);
        let text = (TEXT_ALPHA * 255.0).round() as u8;
        // Text: light, at least the text's own opacity (more where it
        // overlaps its shadow, which also darkens it a little).
        let lit: Vec<&[u8]> = px
            .chunks(4)
            .filter(|p| p[3] >= text && p[0] > 180 && p[2] > 180)
            .collect();
        assert!(lit.len() >= 6, "{} text pixels", lit.len());
        // The shadow: black, partly transparent, next to the strokes.
        let shadow = px
            .chunks(4)
            .filter(|p| p[0] == 0 && p[3] > 0 && p[3] < text)
            .count();
        assert!(shadow > lit.len(), "{shadow} shadow pixels");
        // The shadow falls down-right: the last row and column have some,
        // the first row and column none.
        assert!((0..w).all(|x| at(x, 0)[3] == 0));
        assert!((0..h).all(|y| at(0, y)[3] == 0));
        assert!((0..w).any(|x| at(x, h - 1)[3] > 0) || (0..w).any(|x| at(x, h - 2)[3] > 0));
        assert_eq!(rasterize(&[], 1).0, 0);
    }

    #[test]
    fn the_middle_dot_has_a_glyph() {
        assert_ne!(glyph('\u{b7}'), [0; 8]);
        assert_ne!(glyph('\u{b7}'), glyph('?'));
        assert_eq!(glyph('\u{2603}'), glyph('?'));
    }

    /// Renders a frame of a known pattern, draws the watermark over it,
    /// and reads the target back.
    fn render(size: (u32, u32), area: Viewport, panels: &Panels) -> Option<Vec<u8>> {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            eprintln!("no GPU adapter; skipping");
            return None;
        };
        let (w, h) = size;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // The "game frame": a gradient, so a changed pixel anywhere shows.
        let frame: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % w) as u8, (i / w) as u8, 0x80, 0xff])
            .collect();
        ctx.queue.write_texture(
            tex.as_image_copy(),
            &frame,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let view = tex.create_view(&Default::default());
        let mut wm = WatermarkRenderer::new(&ctx.device);
        // Twice, as the shell does every present: the second reuses the
        // texture.
        for _ in 0..2 {
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            wm.draw(
                &ctx.device,
                &ctx.queue,
                &mut enc,
                panels,
                &view,
                format,
                size,
                area,
            );
            ctx.queue.submit([enc.finish()]);
        }
        let out = gfx::readback::read_texture(&ctx, &tex, wgpu::TextureAspect::All, 4, size);
        Some(frame.iter().zip(&out).map(|(a, b)| a ^ b).collect())
    }

    #[test]
    fn draws_along_the_bottom_and_leaves_the_rest_of_the_frame_alone() {
        let (w, h) = (640u32, 480u32);
        let panels = Watermark::new()
            .with_build(BUILD)
            .panels(LiveStats::from_rate(30, 1.0).as_ref(), "metal");
        let Some(diff) = render((w, h), Viewport::default(), &panels) else {
            return;
        };
        let changed = |x: u32, y: u32| diff[((y * w + x) * 4) as usize..][..4] != [0; 4];
        let rows: Vec<u32> = (0..h).filter(|&y| (0..w).any(|x| changed(x, y))).collect();
        assert!(!rows.is_empty(), "nothing drawn");
        // Two lines a panel plus their shadow, within the bottom margin's reach.
        let block = (PITCH + CELL + 2 * PAD) + MARGIN;
        assert!(rows[0] >= h - block, "drawn from row {}", rows[0]);
        assert!(*rows.last().unwrap() < h - MARGIN + PAD);
        // The label around the middle, the live panel at the right, nothing
        // at the left edge.
        let cols: Vec<u32> = (0..w).filter(|&x| (0..h).any(|y| changed(x, y))).collect();
        assert!(cols[0] > w / 4, "drawn from column {}", cols[0]);
        assert!(cols.iter().any(|&x| x.abs_diff(w / 2) < 8), "nothing centred");
        assert!(*cols.last().unwrap() >= w - MARGIN - PAD, "nothing at the right");
    }

    #[test]
    fn follows_the_game_image_inside_letterboxing() {
        // A 4:3 image pillarboxed in a 1000x480 target.
        let (w, h) = (1000u32, 480u32);
        let area = Viewport {
            x: 180,
            y: 0,
            width: 640,
            height: 480,
        };
        let panels = Panels {
            label: vec!["x".repeat(300), "y".repeat(300)],
            live: vec!["z".repeat(300); 5],
        };
        let Some(diff) = render((w, h), area, &panels) else {
            return;
        };
        let changed = |x: u32, y: u32| diff[((y * w + x) * 4) as usize..][..4] != [0; 4];
        let mut any = false;
        for y in 0..h {
            for x in 0..w {
                if changed(x, y) {
                    any = true;
                    assert!(
                        x >= area.x && x < area.x + area.width && y >= h - 80,
                        "pixel ({x}, {y}) outside the image's bottom"
                    );
                }
            }
        }
        assert!(any);
    }

}
