//! The comparison uses one exposure and one scale for all three images.
//! Pages with the most light removed appear first. Every mip is inspectable.
use super::{
    Outcome,
    input::Mip,
    solve::{Fit, PageSplit, Stats, Vertex},
};
use anyhow::Result;
use std::fmt::Write as _;

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn percent(n: usize, total: usize) -> f64 {
    if total == 0 {
        0.
    } else {
        100. * n as f64 / total as f64
    }
}

#[derive(Default)]
struct Totals {
    pages: usize,
    changed: usize,
    covered: usize,
    clamped: usize,
    band: usize,
    unfilled: usize,
    closure: u8,
    play: u8,
}

impl Totals {
    fn record(&mut self, s: Stats, changed: usize) {
        self.pages += 1;
        self.changed += changed;
        self.covered += s.covered;
        self.clamped += s.clamped;
        self.band += s.edge;
        self.unfilled += s.unfilled;
        self.closure = self.closure.max(s.max_closure_error);
        self.play = self.play.max(s.max_play_error);
    }

    fn row(&self, name: &str) -> String {
        format!(
            "<tr><th>{name}</th><td>{}</td><td>{}</td><td>{}</td><td>{:.2}%</td><td>{:.2}%</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            self.pages,
            self.covered,
            self.changed,
            percent(self.clamped, self.covered),
            percent(self.band, self.covered),
            self.unfilled,
            self.closure,
            self.play
        )
    }
}

pub(super) struct Report {
    title: String,
    source: String,
    world: usize,
    scenery: usize,
    pub warnings: Vec<String>,
    // (is a lower mip, total removed RGB energy, page entry)
    pages: Vec<(bool, u64, String)>,
    base: Totals,
    mips: Totals,
    runs: Totals,
    vertices: Vec<String>,
}

impl Report {
    pub fn new(
        name: &str,
        bsp: usize,
        checksum: u32,
        light: &str,
        world: usize,
        scenery: usize,
    ) -> Self {
        Self {
            title: format!("{} · BSP {bsp} · {checksum:08x}", escape(name)),
            source: escape(light),
            world,
            scenery,
            warnings: vec![],
            pages: vec![],
            base: Totals::default(),
            mips: Totals::default(),
            runs: Totals::default(),
            vertices: vec![],
        }
    }

    pub fn page(
        &mut self,
        stem: &str,
        mip: &Mip,
        split: &PageSplit,
        fitted: bool,
        write: &mut impl FnMut(&str, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let mut rgba = Vec::with_capacity(mip.retail.len() * 4);
        let mut png =
            |suffix: &str, preserve_alpha: bool, color: &dyn Fn(usize) -> [u8; 3]| -> Result<()> {
                rgba.clear();
                for i in 0..mip.retail.len() {
                    rgba.extend_from_slice(&color(i));
                    rgba.push(if preserve_alpha { mip.alpha[i] } else { 255 });
                }
                let mut data = Vec::new();
                assets::png::write_rgba_fast(&mut data, mip.width, mip.height, &rgba)?;
                write(&format!("{stem}-{suffix}.png"), &data)
            };
        png("retail", false, &|i| mip.retail[i])?;
        png("visibility", false, &|i| [split.texels[i].visibility; 3])?;
        png("alpha", false, &|i| [mip.alpha[i]; 3])?;
        let has_alpha = mip.alpha.iter().any(|&a| a != 255);
        if has_alpha {
            png("retail-rgba", true, &|i| mip.retail[i])?;
        }
        let mut views = vec![("retail", "Retail")];
        if fitted {
            png("on", false, &|i| split.texels[i].lamp_on)?;
            png("off", false, &|i| split.texels[i].lamp_off)?;
            png("direct", false, &|i| split.texels[i].direct)?;
            png("closure-i", false, &|i| split.texels[i].closure_i)?;
            png("baked", false, &|i| split.texels[i].baked)?;
            if has_alpha {
                png("on-rgba", true, &|i| split.texels[i].lamp_on)?;
                png("off-rgba", true, &|i| split.texels[i].lamp_off)?;
            }
            views.extend([("on", "Reconstructed · on"), ("off", "Reconstructed · off")]);
        } else {
            views.push(("visibility", "Traced visibility"));
        }
        let mut energy = 0u64;
        let mut changed = 0;
        for p in &split.texels {
            let delta: u64 = (0..3)
                .map(|c| u64::from(p.lamp_on[c].abs_diff(p.lamp_off[c])))
                .sum();
            energy += delta;
            changed += usize::from(delta != 0);
        }
        let t = if mip.level == 0 {
            &mut self.base
        } else {
            &mut self.mips
        };
        t.record(split.stats, changed);
        let mut entry = format!(
            "<summary>{stem} · {} × {} · {changed} texels change</summary><div class=triptych>",
            mip.width, mip.height
        );
        for (suffix, title) in views {
            write!(
                entry,
                "<figure><figcaption>{title}</figcaption><a href='{stem}-{suffix}.png'><img loading=lazy alt='{title}, {stem}' src='{stem}-{suffix}.png'></a></figure>"
            )?;
        }
        let s = split.stats;
        write!(
            entry,
            "</div><p>{} covered · {} clamped · {} edge texels · {} without two fill samples · max closure error {} · max on/retail error {}.</p>",
            s.covered, s.clamped, s.edge, s.unfilled, s.max_closure_error, s.max_play_error
        )?;
        let mut links = vec![("visibility", "V"), ("alpha", "Retail alpha")];
        if fitted {
            links.extend([
                ("direct", "Unshadowed D"),
                ("closure-i", "Closure I"),
                ("baked", "Closure B"),
            ]);
        }
        for (file, label) in links {
            write!(entry, "<a href='{stem}-{file}.png'>{label}</a> &ensp;")?;
        }
        entry.push_str("<p>Display pages with original alpha &ensp;");
        for (file, label) in [
            ("retail", "Retail RGBA"),
            ("on", "Lamp on RGBA"),
            ("off", "Lamp off RGBA"),
        ] {
            if file == "retail" || fitted {
                let rgba = if has_alpha { "-rgba" } else { "" };
                write!(
                    entry,
                    "<a href='{stem}-{file}{rgba}.png'>{label}</a> &ensp;"
                )?;
            }
        }
        entry.push_str("</p></details>");
        self.pages.push((mip.level != 0, energy, entry));
        Ok(())
    }

    pub fn vertices(
        &mut self,
        index: usize,
        label: &str,
        vertices: &[Vertex],
        split: &PageSplit,
        fitted: bool,
        write: &mut impl FnMut(&str, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let name = format!("vertices-{index}.csv");
        let mut csv = String::from("vertex,x,y,z,nx,ny,nz,retail_rgb_hex,visibility_byte");
        if fitted {
            csv.push_str(",on_rgb_hex,off_rgb_hex,direct_rgb_hex,closure_i_rgb_hex,baked_rgb_hex");
        }
        csv.push('\n');
        let mut changed = 0;
        let hex = |rgb: [u8; 3]| u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]]);
        for (i, (v, p)) in vertices.iter().zip(&split.texels).enumerate() {
            let [x, y, z] = v.position;
            let [nx, ny, nz] = v.normal;
            write!(
                csv,
                "{i},{x},{y},{z},{nx},{ny},{nz},{:06x},{}",
                hex(v.retail),
                p.visibility
            )?;
            if fitted {
                for rgb in [p.lamp_on, p.lamp_off, p.direct, p.closure_i, p.baked] {
                    write!(csv, ",{:06x}", hex(rgb))?;
                }
            }
            csv.push('\n');
            changed += usize::from(p.lamp_on != p.lamp_off);
        }
        write(&name, csv.as_bytes())?;
        self.runs.record(split.stats, changed);
        self.vertices.push(format!(
            "<li><a href='{name}'>{}</a> · {} vertices · {} change</li>",
            escape(label),
            vertices.len(),
            changed
        ));
        Ok(())
    }

    pub fn finish(
        mut self,
        fits: [Option<Fit>; 2],
        write: &mut impl FnMut(&str, &[u8]) -> Result<()>,
    ) -> Result<Outcome> {
        let (reviewable, summary) = if fits[1].is_none() {
            (
                false,
                "STOP · No usable shadow-edge pair for the selected source with scenery.",
            )
        } else if self.base.changed == 0 {
            (
                false,
                "STOP · The selected light changes no base-level display texels.",
            )
        } else if self
            .base
            .closure
            .max(self.mips.closure)
            .max(self.runs.closure)
            > 1
        {
            (false, "STOP · Closure exceeds one stored channel step.")
        } else if !self.warnings.is_empty() {
            (
                false,
                "STOP · Coverage is incomplete. Inspect the omissions before judging this BSP.",
            )
        } else {
            (
                true,
                "Ready for owner review · Inspect the on/off pages before accepting this BSP.",
            )
        };
        let mut html = String::from(
            r#"<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>Lighting split</title>
<style>
:root{color-scheme:dark}body{max-width:88rem;margin:auto;padding:1.5rem;font:1rem/1.5 system-ui;background:#16191c;color:#eee}
h1{font-size:1.55rem}h2{margin-top:2rem}strong{color:#ffe3a6}a{color:#abd7ff}table{border-collapse:collapse}
td,th{padding:.3rem .7rem;text-align:right;border-bottom:1px solid #555}th:first-child{text-align:left}
details{padding:.5rem 0;border-top:1px solid #555}summary{cursor:pointer}.triptych{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:.6rem}
figure{margin:0}img{width:100%;height:auto;image-rendering:pixelated}figcaption{margin:.35rem 0}
.scroll{overflow:auto}p{max-width:85ch}code{font-size:1.05em}
</style><main>"#,
        );
        write!(
            html,
            "<h1>{}</h1><p>{}</p><p><strong>{summary}</strong></p>",
            self.title, self.source
        )?;
        html.push_str("<p>Compare retail with reconstructed on at the same scale. Look for ridges or doubled edges. In off, check that the selected direct light disappears while independent fill and shadow detail survive. Bounce from that source stays in the residual. Click an image for its native texels.</p>");
        write!(
            html,
            "<h2>Source fit</h2><p>{} BSP and placement triangles · {} scenario scenery triangles. The images use the scenery-inclusive fit.</p>",
            self.world, self.scenery
        )?;
        html.push_str("<table><tr><th>Occluders</th><th>k in RGB steps</th><th>Edge pairs</th><th>Median edge error</th></tr>");
        for (label, fit) in ["BSP and instances", "+ scenario scenery"]
            .into_iter()
            .zip(fits)
        {
            match fit {
                Some(f) => {
                    let [r, g, b] = f.k;
                    write!(
                        html,
                        "<tr><th>{label}</th><td>{r:.2}, {g:.2}, {b:.2}</td><td>{}</td><td>{:.2}</td></tr>",
                        f.pairs, f.median_error
                    )?;
                }
                None => write!(
                    html,
                    "<tr><th>{label}</th><td colspan=3>No usable edge pair</td></tr>"
                )?,
            }
        }
        html.push_str("</table><p>Each pair is a pair of neighbouring base-mip texels on the same chart. Error is the absolute discrepancy in their predicted colour difference, averaged across RGB in steps of 1/255.</p>");
        if !self.warnings.is_empty() {
            html.push_str("<h2>Coverage omissions</h2><ul>");
            self.warnings.sort();
            self.warnings.dedup();
            for warning in self.warnings {
                write!(html, "<li>{}</li>", escape(&warning))?;
            }
            html.push_str("</ul>");
        }
        html.push_str("<h2>Split checks</h2><div class=scroll><table><tr><th>Receivers</th><th>Pages / runs</th><th>Covered</th><th>Changed on/off</th><th>Clamped</th><th>Edge band</th><th>Unfilled edges</th><th>Max closure error</th><th>Max on/retail error</th></tr>");
        for row in [
            self.base.row("Base pages"),
            self.mips.row("Stored lower mips"),
            self.runs.row("Vertex runs"),
        ] {
            html.push_str(&row);
        }
        html.push_str("</table></div><p>Closure I + B preserves retail by capping the fitted baked direct at the available RGB. Clamped share measures where the model overshoots. Reconstructed on uses edge-filled I + D × V. Its error exposes the visible effect of the fit and fill. The three previews show stored RGB at identical exposure. The corresponding display pages preserve the original alpha.</p>");
        html.push_str("<h2>Lightmap pages</h2><p>Base pages are ordered by total RGB change between on and off. Lower mips follow.</p>");
        self.pages
            .sort_by_key(|(lower, energy, _)| (*lower, std::cmp::Reverse(*energy)));
        for (i, (_, _, page)) in self.pages.into_iter().enumerate() {
            html.push_str(if i < 3 { "<details open>" } else { "<details>" });
            html.push_str(&page);
        }
        if !self.vertices.is_empty() {
            html.push_str("<h2>Per-vertex colours</h2><ul>");
            for row in self.vertices {
                html.push_str(&row);
            }
            html.push_str("</ul>");
        }
        html.push_str("</main></html>\n");
        write("index.html", html.as_bytes())?;
        Ok(Outcome {
            summary: summary.into(),
            reviewable,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::solve::Split;
    use std::collections::BTreeMap;

    #[test]
    fn a_partial_alpha_page_has_three_display_products_and_an_honest_missing_fit_report() {
        let mip = Mip {
            level: 0,
            width: 1,
            height: 1,
            retail: vec![[135, 40, 27]],
            alpha: vec![0],
        };
        let pixel = Split {
            closure_i: [15, 40, 27],
            baked: [120, 0, 0],
            lamp_off: [15, 40, 27],
            direct: [120, 0, 0],
            lamp_on: mip.retail[0],
            visibility: 255,
        };
        let split = PageSplit {
            texels: vec![pixel],
            stats: Stats {
                covered: 1,
                ..Default::default()
            },
        };
        let fit = Fit {
            k: [120., 0., 0.],
            pairs: 1,
            median_error: 0.,
        };
        for evidence in [Some(fit), None] {
            let mut report = Report::new("map <invented>", 2, 123, "sky & light", 1, 0);
            let mut files = BTreeMap::new();
            let mut write = |name: &str, bytes: &[u8]| {
                assert!(files.insert(name.to_owned(), bytes.to_owned()).is_none());
                Ok(())
            };
            report
                .page("page", &mip, &split, evidence.is_some(), &mut write)
                .unwrap();
            let result = report.finish([evidence; 2], &mut write).unwrap();
            assert_eq!(result.reviewable, evidence.is_some());
            let html = std::str::from_utf8(&files["index.html"]).unwrap();
            assert!(html.contains("map &lt;invented&gt;"));
            assert!(html.contains("sky &amp; light"));
            assert!(html.contains("page-retail-rgba.png"));
            assert_ne!(files["page-retail-rgba.png"], files["page-retail.png"]);
            if evidence.is_some() {
                assert_eq!(files["page-retail-rgba.png"], files["page-on-rgba.png"]);
                assert_ne!(files["page-on-rgba.png"], files["page-off-rgba.png"]);
            } else {
                assert!(!files.contains_key("page-on-rgba.png"));
                assert!(html.contains("No usable shadow-edge pair"));
            }
        }
    }
}
