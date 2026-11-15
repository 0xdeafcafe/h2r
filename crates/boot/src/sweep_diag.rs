//! `H2_SWEEP_DIAG=<file>`: a one-shot log of the level loading screen, for
//! the yellow band over it (master 63169a0).
//!
//! The loading screen (`0x289e0`) draws, every frame, the background
//! (`globals\loading_screen\loading_screen`, DXT1 1024x512), the three
//! spinners, then the sweep (`..._sweep`, a Y8 = `L8` 2048x4 gradient)
//! through `0x2a8d0` with `SRCBLEND = DESTCOLOR`, `DESTBLEND = ZERO`: the
//! sweep multiplies what is already there. Its combiners (built at
//! `0x2ac13`) are, with white tint:
//!
//! ```text
//! stage 0: t0.rgb = t0.rgb + c1[0].rgb * c0[0].a   (c1[0] = 0x000000FF: blue)
//! stage 1: r0.rgb = t0.rgb * c0[1].rgb, r1.rgb = t0.a * c0[1].a
//! final:   rgb = (r0 * r1) * fc0.a, alpha = r1.a
//! ```
//!
//! so the sweep's source colour has blue >= red = green, and the screen
//! can only turn yellow if what it multiplies (the previous frame, the
//! background and the spinners: the loading screen clears only on its
//! first frame) has lost blue, or if a texel, constant or blend input is
//! not what the guest set.
//!
//! The log waits for the first draw that binds an `L8` 2048x4 texture,
//! then writes the whole next frame: every clear, copy and draw with its
//! textures, blend state and combiner words, and the colour target read
//! back after each step at a few points. Uploads of 2048x4 `L8` textures
//! log their decoded texels (channel sums), which should be grey.

use std::io::Write;

use d3d::backend::ResolvedDraw;
use gfx::D3DFormat;
use gfx::texture::DecodedTexture;

/// Where the diagnostic is in its one shot.
enum Phase {
    /// Waiting for the sweep draw.
    Armed,
    /// Logging this frame (present count + 1 when the sweep was seen).
    Dumping(u64),
    Done,
}

pub struct SweepDiag {
    out: std::io::BufWriter<std::fs::File>,
    phase: Phase,
    draws: u32,
}

/// Points read back from the target, as fractions of width and height.
const POINTS: [(f32, f32); 10] = [
    (0.10, 0.25),
    (0.20, 0.25),
    (0.30, 0.25),
    (0.40, 0.25),
    (0.50, 0.25),
    (0.10, 0.75),
    (0.20, 0.75),
    (0.30, 0.75),
    (0.50, 0.50),
    (0.80, 0.50),
];

/// Whether `draw` binds the loading screen's sweep (`L8` 2048x4).
pub fn binds_sweep(draw: &ResolvedDraw) -> bool {
    draw.texture_info.iter().flatten().any(|t| {
        matches!(t.format, D3DFormat::L8 | D3DFormat::LinL8) && t.width == 2048 && t.height == 4
    })
}

impl SweepDiag {
    /// From `H2_SWEEP_DIAG`.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var_os("H2_SWEEP_DIAG")?;
        match std::fs::File::create(&path) {
            Ok(f) => Some(Self {
                out: std::io::BufWriter::new(f),
                phase: Phase::Armed,
                draws: 0,
            }),
            Err(e) => {
                eprintln!("boot: H2_SWEEP_DIAG {}: {e}", path.to_string_lossy());
                None
            }
        }
    }

    /// Whether frame `frame` is the one being logged.
    pub fn dumping(&self, frame: u64) -> bool {
        matches!(self.phase, Phase::Dumping(f) if f == frame)
    }

    /// Draws logged so far.
    pub fn logged_draws(&self) -> u32 {
        self.draws
    }

    pub fn line(&mut self, s: &str) {
        let _ = writeln!(self.out, "{s}");
    }

    /// A draw in frame `frame`; arms the dump on the first sweep draw.
    pub fn saw_draw(&mut self, frame: u64, draw: &ResolvedDraw) {
        if matches!(self.phase, Phase::Armed) && binds_sweep(draw) {
            self.line(&format!(
                "sweep draw seen in frame {frame}; logging frame {}",
                frame + 1
            ));
            self.phase = Phase::Dumping(frame + 1);
        }
    }

    /// The present that ends frame `frame`.
    pub fn presented(&mut self, frame: u64) {
        if self.dumping(frame) {
            self.line(&format!("present ends frame {frame}"));
            self.phase = Phase::Done;
        }
        let _ = self.out.flush();
    }

    /// Logs an upload of a sweep-sized `L8` texture: the decoded texels'
    /// channel sums (equal for a grey texture) and a few texels.
    pub fn upload(&mut self, texture: &DecodedTexture) {
        let d = &texture.desc;
        if !(matches!(d.format, D3DFormat::L8 | D3DFormat::LinL8)
            && d.width == 2048
            && d.height == 4)
        {
            return;
        }
        let Some(l) = texture.levels.first() else {
            return;
        };
        let mut sums = [0u64; 4];
        for px in l.data.as_chunks::<4>().0 {
            for (s, b) in sums.iter_mut().zip(px) {
                *s += u64::from(*b);
            }
        }
        let texel = |x: usize| {
            l.data
                .get(4 * x..4 * x + 4)
                .map(|p| format!("{p:?}"))
                .unwrap_or_default()
        };
        self.line(&format!(
            "upload {:?} {}x{} host {:?}: channel sums {sums:?}; texels x=640 {} x=700 {} x=1500 {}",
            d.format,
            d.width,
            d.height,
            texture.host,
            texel(640),
            texel(700),
            texel(1500)
        ));
    }

    /// One draw of the logged frame.
    pub fn draw(&mut self, draw: &ResolvedDraw, color: &str, outcome: &str) {
        let n = self.draws;
        self.draws += 1;
        let tex: Vec<String> = draw
            .texture_info
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                t.map(|t| {
                    format!(
                        "t{i}={:?} {}x{} @{:#x}",
                        t.format, t.width, t.height, t.header
                    )
                })
            })
            .collect();
        let r = &draw.state.render;
        self.line(&format!(
            "draw {n} {} {:?} n={} -> {color} {}x{} {:?}: {outcome}\n  tex [{}]\n  blend enable={} src={:?} dst={:?} op={:?} color={:#010x} write={:?} tfactor={:#010x}",
            draw.entry,
            draw.primitive,
            draw.vertex_count,
            draw.target.width,
            draw.target.height,
            draw.target.format,
            tex.join(" "),
            r.blend_enable,
            r.src_blend,
            r.dst_blend,
            r.blend_op,
            r.blend_color,
            r.color_write,
            r.texture_factor,
        ));
        let c = &draw.shader.combiner_state;
        let count = (c[53] & 0xF).clamp(1, 8) as usize;
        let hex = |w: &[u32]| {
            w.iter()
                .map(|w| format!("{w:08x}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        self.line(&format!(
            "  ps={} latch={} count={:#x} texmodes={:#x} final abcd={:08x} efg={:08x} fc0={:08x} fc1={:08x}\n  rgb_in [{}] rgb_out [{}]\n  a_in [{}] a_out [{}]\n  c0 [{}] c1 [{}]",
            if draw.shader.pixel_shader.is_some() { "program" } else { "fixed" },
            draw.shader.final_combiner_latch,
            c[53],
            c[54],
            c[8],
            c[9],
            c[43],
            c[44],
            hex(&c[34..34 + count]),
            hex(&c[45..45 + count]),
            hex(&c[..count]),
            hex(&c[26..26 + count]),
            hex(&c[10..10 + count]),
            hex(&c[18..18 + count]),
        ));
        if let Some(def) = &draw.shader.pixel_shader {
            self.line(&format!("  psdef[53..60] [{}]", hex(&def[53..60])));
        }
    }

    /// The colour target (`LinA8R8G8B8` bytes `B, G, R, A`, `width` x
    /// `height`) at [`POINTS`], as `(r, g, b)`.
    pub fn samples(&mut self, what: &str, bytes: &[u8], width: u32, height: u32) {
        let at = |(fx, fy): (f32, f32)| {
            let (x, y) = ((fx * width as f32) as usize, (fy * height as f32) as usize);
            let i = 4 * (y * width as usize + x);
            match bytes.get(i..i + 4) {
                Some(p) => format!("({:.2},{:.2})=({},{},{})", fx, fy, p[2], p[1], p[0]),
                None => format!("({fx:.2},{fy:.2})=?"),
            }
        };
        let s: Vec<String> = POINTS.iter().map(|&p| at(p)).collect();
        self.line(&format!("  {what}: {}", s.join(" ")));
    }
}
