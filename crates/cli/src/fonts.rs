//! `h2 font`: render text with a packed font from `/maps/fonts`, or validate them all.
use std::fs::File;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use font::{Font, RenderOptions};

use crate::safe_out::OutDir;

const USAGE: &str = "usage: h2 font [disc.iso] <font> <text> --out <file.png> [--color RRGGBB[AA]] [--bg RRGGBB[AA]] [--glyph-colors] [--icons xbox|xbox360|playstation|keyboard] [--bindings FILE] [--force] [--disc disc.iso]\n       h2 font --all [disc.iso]\n\n<font> is a file under /maps/fonts, e.g. conduit-13 or jpn_DFGHSMGoth_anti_con-13.\n<text> is UTF-8 on the command line and laid out as UTF-16; \\r or \\n start a new line.\nThe PNG has a transparent background; existing files are not replaced unless --force.\n--all parses and fully validates every font file and exits non-zero on any failure.";

fn read_fonts(
    disc: &std::path::Path,
) -> Result<(xiso::Xiso<std::io::BufReader<File>>, Vec<xiso::Entry>)> {
    let mut iso = super::open_iso(disc)?;
    let fonts = iso
        .entries()?
        .into_iter()
        .filter(|e| {
            let p = e.path.to_ascii_lowercase();
            !e.is_dir && p.starts_with("/maps/fonts/") && !p.ends_with(".txt")
        })
        .collect();
    Ok((iso, fonts))
}

fn parse_color(s: &str) -> Result<[u8; 4]> {
    let h = s.trim_start_matches('#');
    if !(h.len() == 6 || h.len() == 8) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("--color wants RRGGBB or RRGGBBAA, got {s:?}");
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
    Ok([b(0), b(2), b(4), if h.len() == 8 { b(6) } else { 255 }])
}

#[derive(Debug, PartialEq)]
struct Args {
    positional: Vec<String>,
    out: Option<PathBuf>,
    disc: Option<PathBuf>,
    all: bool,
    force: bool,
    glyph_colors: bool,
    icons: Option<font::icons::IconChoice>,
    bindings: Option<PathBuf>,
    color: [u8; 4],
    bg: [u8; 4],
    help: bool,
}

fn parse_args(args: Vec<String>) -> Result<Args> {
    let mut args = args.into_iter();
    let mut r = Args {
        positional: Vec::new(),
        out: None,
        disc: None,
        all: false,
        force: false,
        glyph_colors: false,
        icons: None,
        bindings: None,
        color: [255, 255, 255, 255],
        bg: [0, 0, 0, 0],
        help: false,
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => r.all = true,
            "--force" => r.force = true,
            "--glyph-colors" => r.glyph_colors = true,
            "--icons" => {
                let choice = args
                    .next()
                    .context("--icons needs a set")?
                    .parse::<font::icons::IconChoice>()
                    .map_err(anyhow::Error::msg)?;
                if choice == font::icons::IconChoice::Auto {
                    bail!("font previews need a fixed icon set; auto requires live input");
                }
                r.icons = Some(choice);
            }
            "--bindings" => {
                r.bindings = Some(args.next().context("--bindings needs a file")?.into())
            }
            "--out" => r.out = Some(args.next().context("--out needs a file name")?.into()),
            "--disc" => r.disc = Some(args.next().context("--disc needs an ISO path")?.into()),
            "--color" => r.color = parse_color(&args.next().context("--color needs a value")?)?,
            "--bg" => r.bg = parse_color(&args.next().context("--bg needs a value")?)?,
            "-h" | "--help" => r.help = true,
            // After "--" everything is positional, so text can start with a dash.
            "--" => r.positional.extend(args.by_ref()),
            _ => r.positional.push(a),
        }
    }
    Ok(r)
}

/// `--all` over zero fonts is a failure, not a vacuous success.
fn ensure_found(n: usize, disc: &std::path::Path) -> Result<()> {
    if n == 0 {
        bail!(
            "no font files found under /maps/fonts on {}",
            disc.display()
        );
    }
    Ok(())
}

/// Turn the two-character escapes `\\n` and `\\r` into line breaks.
fn unescape(text: &str) -> String {
    text.replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\\r", "\r")
}

/// Draw the straight-alpha bitmap over a solid background, in place.
fn fill_background(rgba: &mut [u8], bg: [u8; 4]) {
    if bg[3] == 0 {
        return;
    }
    for p in rgba.as_chunks_mut::<4>().0.iter_mut() {
        let (sa, ba) = (u32::from(p[3]), u32::from(bg[3]));
        let oa = sa + ba * (255 - sa) / 255;
        for k in 0..3 {
            let v = u32::from(p[k]) * sa + u32::from(bg[k]) * ba * (255 - sa) / 255;
            p[k] = (v / oa) as u8;
        }
        p[3] = oa as u8;
    }
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let Args {
        mut positional,
        out,
        mut disc,
        all,
        force,
        glyph_colors,
        icons,
        bindings,
        color,
        bg,
        help,
    } = parse_args(args)?;
    if help {
        println!("{USAGE}");
        return Ok(());
    }
    if all && (icons.is_some() || bindings.is_some()) {
        bail!("--all validates original disc fonts; choose a font to preview icons");
    }
    if bindings.is_some() && icons.is_none() {
        bail!("--bindings requires --icons");
    }
    let want = if all { 0 } else { 2 };
    // A leading existing .iso/.xbe is the disc (so `h2 font <iso> <font> <text>` works).
    if positional.len() > want
        && disc.is_none()
        && super::is_disc(std::path::Path::new(&positional[0]))
    {
        disc = Some(PathBuf::from(positional.remove(0)));
    }
    if positional.len() != want {
        bail!("{USAGE}");
    }
    let disc = match disc {
        Some(d) => d,
        None => super::default_disc()?,
    };
    let (mut iso, fonts) = read_fonts(&disc)?;
    if all {
        ensure_found(fonts.len(), &disc)?;
        let mut failures = 0;
        for e in &fonts {
            let bytes = iso.read(e)?;
            match Font::parse(&bytes).and_then(|f| f.validate().map(|n| (n, f))) {
                Ok((n, f)) => println!(
                    "ok    {:<44} {n:>5} glyphs  {} kern pairs  ascent {} descent {} leading {}",
                    e.path,
                    f.header.kerning.len(),
                    f.header.ascent,
                    f.header.descent,
                    f.header.leading
                ),
                Err(err) => {
                    failures += 1;
                    println!("FAIL  {:<44} {err}", e.path);
                }
            }
        }
        println!("{} fonts, {failures} failed", fonts.len());
        if failures > 0 {
            bail!("{failures} font(s) failed validation");
        }
        return Ok(());
    }
    let (name, text) = (&positional[0], &positional[1]);
    let name = name.trim_start_matches('/');
    let name = name.strip_prefix("maps/fonts/").unwrap_or(name);
    let entry = fonts
        .iter()
        .find(|e| e.path["/maps/fonts/".len()..].eq_ignore_ascii_case(name))
        .with_context(|| {
            let names: Vec<_> = fonts
                .iter()
                .map(|e| &e.path["/maps/fonts/".len()..])
                .collect();
            format!(
                "no font {name:?} on the disc; available: {}",
                names.join(", ")
            )
        })?;
    let out = out.context("--out <file.png> is required")?;
    let file = out
        .file_name()
        .and_then(|n| n.to_str())
        .context("--out must name a file")?
        .to_owned();
    let dir = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut bytes = iso.read(entry)?;
    let mut units: Vec<u16> = unescape(text).encode_utf16().collect();
    if let Some(choice) = icons {
        let bindings = match bindings {
            Some(path) => input::bindings::Bindings::from_toml(
                &std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?,
            )?,
            None => input::bindings::Bindings::default(),
        };
        let icons = boot::icons::Icons::with_parts(
            boot::icons::IconOptions {
                choice,
                gamepads: false,
            },
            Some(&bindings),
            None,
        );
        if icons.enabled() {
            bytes = icons
                .font_rewrite()
                .patch(&bytes)
                .context("font cannot safely accept the icon sets")?;
            units.iter_mut().for_each(|u| *u = icons.shown(*u));
        }
    }
    let font = Font::parse(&bytes).with_context(|| format!("parsing {}", entry.path))?;
    let bmp = font::render(
        &font,
        &units,
        RenderOptions {
            color,
            glyph_colors,
        },
    )
    .context("rendering")?;
    let mut png = Vec::new();
    let mut bmp = bmp;
    fill_background(&mut bmp.rgba, bg);
    assets::png::write_rgba(&mut png, bmp.width, bmp.height, &bmp.rgba)?;
    let path = OutDir::new(dir, force)?.write(&[file], &png)?;
    println!(
        "{}: {}x{} -> {}",
        entry.path,
        bmp.width,
        bmp.height,
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_color_forms() {
        assert_eq!(parse_color("ff8000").unwrap(), [255, 128, 0, 255]);
        assert_eq!(parse_color("#00112233").unwrap(), [0, 17, 34, 51]);
        for bad in [
            "", "fff", "ff80000", "gg0000", "ff80 0", "+f8000", "ffff-fff",
        ] {
            assert!(parse_color(bad).is_err(), "{bad:?}");
        }
        // non-ASCII must be an error, not a slicing panic
        assert!(parse_color("\u{e9}\u{e9}\u{e9}").is_err());
    }

    #[test]
    fn parse_args_options() {
        let a = parse_args(v(&[
            "x.iso",
            "conduit-13",
            "hi",
            "--out",
            "o.png",
            "--bg",
            "202020",
            "--color",
            "ffffff80",
            "--force",
            "--glyph-colors",
        ]))
        .unwrap();
        assert_eq!(a.positional, v(&["x.iso", "conduit-13", "hi"]));
        assert_eq!(a.out, Some(PathBuf::from("o.png")));
        assert_eq!(a.bg, [32, 32, 32, 255]);
        assert_eq!(a.color, [255, 255, 255, 128]);
        assert!(a.force && a.glyph_colors && !a.all);
        // defaults: transparent background, white text
        let d = parse_args(v(&["f", "t"])).unwrap();
        assert_eq!((d.bg, d.color), ([0, 0, 0, 0], [255; 4]));
        // missing values and bad colours are errors
        assert!(parse_args(v(&["--out"])).is_err());
        assert!(parse_args(v(&["--bg"])).is_err());
        assert!(parse_args(v(&["--bg", "zz"])).is_err());
        // text may start with a dash after `--`
        let t = parse_args(v(&["f", "--", "--all"])).unwrap();
        assert_eq!(t.positional, v(&["f", "--all"]));
        assert!(!t.all);
        assert!(parse_args(v(&["-h"])).unwrap().help);
    }

    #[test]
    fn escapes_and_empty_selection() {
        assert_eq!(unescape("a\\nb\\rc\\r\\nd"), "a\nb\rc\nd");
        assert_eq!(unescape("plain"), "plain");
        assert!(ensure_found(0, std::path::Path::new("x.iso")).is_err());
        assert!(ensure_found(1, std::path::Path::new("x.iso")).is_ok());
    }

    #[test]
    fn background_compositing() {
        let mut px = vec![0, 0, 0, 0, 10, 20, 30, 255, 0, 0, 0, 128];
        fill_background(&mut px, [255, 255, 255, 255]);
        assert_eq!(&px[0..4], &[255, 255, 255, 255]);
        assert_eq!(&px[4..8], &[10, 20, 30, 255]);
        assert_eq!(px[11], 255);
        assert!((126..=129).contains(&px[8]));
        // zero-alpha pixels with a zero-alpha background must not divide by zero
        let mut z = vec![5, 6, 7, 0];
        fill_background(&mut z, [1, 2, 3, 0]);
        let mut t = vec![1, 2, 3, 0];
        fill_background(&mut t, [9, 9, 9, 0]);
        assert_eq!(t, vec![1, 2, 3, 0]);
    }
    #[test]
    fn preview_requires_a_fixed_set_and_accepts_bindings() {
        for set in ["xbox", "xbox360", "playstation", "keyboard"] {
            let args = parse_args(v(&["--icons", set, "--bindings", "keys.toml"])).unwrap();
            assert!(args.icons.is_some());
            assert_eq!(args.bindings, Some(PathBuf::from("keys.toml")));
        }
        for flags in [
            v(&["--icons"]),
            v(&["--icons", "auto"]),
            v(&["--icons", "unknown"]),
            v(&["--bindings"]),
        ] {
            assert!(parse_args(flags).is_err());
        }
    }
}
