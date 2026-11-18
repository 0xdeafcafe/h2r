//! `h2 ui`: print a screen's widget tree from its `wgit` definition, or with
//! `--all` validate every UI tag of one map or of the whole disc.
//!
//! Prints widget kinds, rectangles (UI units, origin at screen centre, Y up),
//! bitmap tag paths, animation indices and string IDs with their string-ID
//! names. It never resolves or prints localized text.
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context, Result, anyhow, bail};
use map::{Map, TagId, TagQuery};
use tags::ui::{
    self, AnimationRef, BitmapBlock, MapSource, Rect, Screen, ShaderBitmap, SharedGlobals,
    SkinTree, TagRef, TextBlock, UiTag,
    check::{MapCheck, check_map},
};
use tags::wire::StringId;

const USAGE: &str = "usage: h2 ui <map-name-or-file> <screen tag> [--disc disc.iso]\n       h2 ui preview <screen|list> [--state NAME|all] [--focus N] [--synthetic] [--disc disc.iso] [--out DIR]\n       h2 ui sheet [--synthetic] [--disc disc.iso] [--out DIR]\n       h2 ui --all <map-name-or-file|all> [--disc disc.iso]\n\nPrints the widget tree of a wgit screen: widget kinds, rectangles as\n[top left bottom right] in UI units (origin at the screen centre, Y up),\nbitmap tag paths and string IDs. Localized text is never printed.\nWith --all, decodes every wgit, wgtz, wigl and skin tag of the map (or of\nevery disc map with `all`) and runs the engine index cross-checks; prints\ncounts and exits non-zero on any failure, on a map without UI tags, or on a\nmap that does not hold exactly one wigl.";

pub(super) fn run(args: Vec<String>) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("preview") => return preview(args[1..].to_vec()),
        Some("sheet") => return sheet(args[1..].to_vec()),
        _ => {}
    }
    let mut args = args.into_iter();
    let (mut positional, mut disc, mut all) = (Vec::new(), None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => all = true,
            "--disc" => {
                disc = Some(PathBuf::from(
                    args.next().context("--disc needs an ISO path")?,
                ))
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n{USAGE}"),
            _ => positional.push(a),
        }
    }
    match (all, positional.as_slice()) {
        (true, [target]) => validate(target, disc),
        (false, [map_name, screen]) => {
            let mut map = super::maps::load_map(map_name, disc)?;
            let id = map
                .tag(TagQuery::NameInGroup(screen, ui::SCREEN))
                .with_context(|| format!("no wgit tag named {screen}"))?
                .id;
            let mut out = String::new();
            Printer::new(&mut map)?.screen(id, &mut out)?;
            print!("{out}");
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

/// `--all`: check one map, or every map on the disc with `all`.
fn validate(target: &str, disc: Option<PathBuf>) -> Result<()> {
    let mut total = MapCheck::default();
    let mut maps = 0;
    if target == "all" {
        let disc = disc.map(Ok).unwrap_or_else(super::default_disc)?;
        let mut iso = super::open_iso(&disc)?;
        let entries = iso.entries()?;
        for e in entries
            .iter()
            .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
        {
            let mut map = super::maps::disc_map(&disc, iso.partition_offset(), e)?;
            total.merge(prefixed(&e.path, check_map(&mut map)));
            maps += 1;
        }
        if maps == 0 {
            bail!("no maps on {}", disc.display());
        }
    } else {
        let mut map = super::maps::load_map(target, disc)?;
        total = prefixed(target, check_map(&mut map));
        maps = 1;
    }
    print!("{}", report(maps, &total));
    for f in &total.failures {
        eprintln!("{f}");
    }
    if !total.failures.is_empty() {
        bail!("{} UI failures", total.failures.len());
    }
    Ok(())
}

fn prefixed(map: &str, mut c: MapCheck) -> MapCheck {
    for f in &mut c.failures {
        *f = format!("{map}: {f}");
    }
    c
}

fn report(maps: usize, c: &MapCheck) -> String {
    let mut s = format!(
        "{maps} maps\ngroup   tags  fail   blocks  nonempty  aliased  foreign  elements    refs  widgets\n"
    );
    for (g, n) in &c.groups {
        let _ = writeln!(
            s,
            "{g}  {:>6} {:>5} {:>8} {:>9} {:>8} {:>8} {:>9} {:>7} {:>8}",
            n.tags,
            n.failures,
            n.stats.blocks,
            n.stats.nonempty_blocks,
            n.stats.aliased_blocks,
            n.stats.foreign_aliases,
            n.stats.elements,
            n.stats.references,
            n.widgets
        );
    }
    let _ = writeln!(
        s,
        "max skin depth {}, largest decode {} of {} budget bytes, skipped table cells {}, \
         ignored shape groups {}\n{} failures",
        c.max_skin_depth,
        c.max_decode_bytes,
        c.budget,
        c.skipped_cells,
        c.ignored_shape_groups,
        c.failures.len()
    );
    s
}

struct Printer<'a> {
    src: MapSource<'a, BufReader<File>>,
    shared: SharedGlobals,
    bitmaps: HashMap<TagId, Vec<Option<(i16, i16)>>>,
    skins: HashMap<TagId, Rc<SkinTree>>,
}

/// `[top left bottom right]`.
fn rect(r: Rect) -> String {
    format!("[{} {} {} {}]", r.top, r.left, r.bottom, r.right)
}
fn anim(a: AnimationRef) -> String {
    a.index().map_or(String::new(), |i| format!(" anim {i}"))
}
/// `(width, height)` as UI units; `None` if a dimension does not fit in i16.
fn image_size(d: [u16; 3]) -> Option<(i16, i16)> {
    Some((i16::try_from(d[0]).ok()?, i16::try_from(d[1]).ok()?))
}

impl<'a> Printer<'a> {
    fn new(map: &'a mut Map<BufReader<File>>) -> Result<Self> {
        let wigl: Vec<TagId> = map
            .tags()
            .iter()
            .filter(|t| t.group == ui::SHARED_GLOBALS)
            .map(|t| t.id)
            .collect();
        let [id] = wigl.as_slice() else {
            bail!(
                "map has {} wigl (shared UI globals) tags; expected exactly 1",
                wigl.len()
            )
        };
        let mut src = MapSource::new(map);
        let shared = match src.decode_tag(*id) {
            Ok(d) => match d.tag {
                UiTag::SharedGlobals(g) => *g,
                _ => bail!("wigl {id} did not decode as shared globals"),
            },
            Err(e) => return Err(anyhow!(e).context(format!("decoding wigl {id}"))),
        };
        Ok(Self {
            src,
            shared,
            bitmaps: HashMap::new(),
            skins: HashMap::new(),
        })
    }
    fn name(&self, r: TagRef) -> String {
        match r.id {
            None => "none".into(),
            Some(id) => match self.src.map_ref().tag(id) {
                Ok(t) => format!("{} {}", t.group, t.name.as_deref().unwrap_or("<unnamed>")),
                Err(_) => format!("{id} <unresolved>"),
            },
        }
    }
    /// String ID value plus its string-ID name (an identifier, not the text).
    fn sid(&self, s: StringId) -> String {
        match s.resolve(self.src.map_ref().strings()) {
            Ok(Some("")) => format!("sid {:#010x} <empty>", s.0),
            Ok(Some(name)) => format!("sid {:#010x} {name}", s.0),
            Ok(None) => "sid none".into(),
            Err(_) => format!("sid {:#010x} <bad>", s.0),
        }
    }
    /// Size of every image of a bitmap tag, `None` where it overflows i16.
    fn image_sizes(&mut self, id: TagId) -> Vec<Option<(i16, i16)>> {
        if let Some(v) = self.bitmaps.get(&id) {
            return v.clone();
        }
        let sizes = match tags::DecodedTag::read(self.src.map(), id) {
            Ok(tags::DecodedTag {
                data: tags::TagData::Bitmap(b),
                ..
            }) => b.images.iter().map(|i| image_size(i.dimensions)).collect(),
            _ => Vec::new(),
        };
        self.bitmaps.insert(id, sizes.clone());
        sizes
    }
    /// A skin and its nested table skins, decoded once per run.
    fn skin_tree(&mut self, id: TagId) -> Result<Rc<SkinTree>> {
        if let Some(t) = self.skins.get(&id) {
            return Ok(t.clone());
        }
        let (tree, _) =
            ui::resolve_skin(&mut self.src, id).with_context(|| format!("decoding skin {id}"))?;
        let tree = Rc::new(tree);
        self.skins.insert(id, tree.clone());
        Ok(tree)
    }

    fn screen(&mut self, id: TagId, out: &mut String) -> Result<()> {
        let decoded = self.src.decode_tag(id)?;
        let UiTag::Screen(s) = decoded.tag else {
            bail!("not a screen")
        };
        let path = self.src.map_ref().tag(id)?.name.clone().unwrap_or_default();
        writeln!(
            out,
            "screen {path} ({id}) screen_id {} flags {:#x}",
            s.screen_id, s.flags
        )?;
        writeln!(out, "  strings {}", self.name(s.strings))?;
        self.header(&s, out)?;
        if s.shape_group != 0 {
            let note = if self.shared.shape_group(s.shape_group).is_none() {
                " (not in wigl; ignored by the engine)"
            } else {
                ""
            };
            writeln!(out, "  shape group {}{note}", s.shape_group - 1)?;
        }
        for (i, l) in s.string_lists.iter().enumerate() {
            writeln!(out, "  string list {i}: {} string IDs", l.strings.len())?;
        }
        for b in &s.preload_bitmaps {
            writeln!(out, "  preload {}", self.name(*b))?;
        }
        for (i, p) in s.panes.iter().enumerate() {
            writeln!(out, "  pane {i}{}", anim(p.animation))?;
            for b in &p.buttons {
                let bitmap = if b.bitmap.id.is_some() {
                    format!(" bitmap {}", self.name(b.bitmap))
                } else {
                    String::new()
                };
                writeln!(
                    out,
                    "    button {} font {} {}{bitmap}{}",
                    rect(b.rect),
                    b.font,
                    self.sid(b.string),
                    anim(b.animation)
                )?;
            }
            for l in &p.lists {
                let skin = self.shared.skin(l.skin);
                let skin_name = skin.map_or_else(|| "<out of range>".into(), |r| self.name(r));
                writeln!(
                    out,
                    "    list anchor ({}, {}) items {} skin {} {skin_name}{}",
                    l.anchor.x,
                    l.anchor.y,
                    l.visible_items,
                    l.skin,
                    anim(l.animation)
                )?;
                if let Some(id) = skin.and_then(|r| r.id) {
                    let tree = self.skin_tree(id)?;
                    self.skin(&tree, "      ", out)?;
                }
            }
            for t in &p.text_blocks {
                self.text(t, "    ", out)?;
            }
            for b in &p.bitmaps {
                self.bitmap(b, "    ", out)?;
            }
            for m in &p.model_scenes {
                writeln!(
                    out,
                    "    model scene viewport {} objects {}+{}{}",
                    rect(m.viewport),
                    m.objects.len(),
                    m.secondary_objects.len(),
                    anim(m.animation)
                )?;
            }
            for b in &p.shader_bitmaps {
                self.shader_bitmap(b, "    ", out)?;
            }
            for t in &p.tables {
                writeln!(
                    out,
                    "    table anchor ({}, {}) cells {} ({} cols x {} rows) skin {}",
                    t.anchor.x,
                    t.anchor.y,
                    t.cells,
                    t.columns,
                    t.rows,
                    self.name(t.skin)
                )?;
                if let Some(id) = t.skin.id {
                    let tree = self.skin_tree(id)?;
                    self.skin(&tree, "      ", out)?;
                }
            }
        }
        Ok(())
    }

    fn header(&self, s: &Screen, out: &mut String) -> Result<()> {
        if s.flags & Screen::NO_HEADER != 0 {
            writeln!(out, "  header hidden")?;
            return Ok(());
        }
        let slot = s.header_layout();
        writeln!(
            out,
            "  header {} font {} {}",
            rect(self.shared.header_layouts[slot].header),
            self.shared.header_fonts[slot],
            self.sid(s.header)
        )?;
        Ok(())
    }

    fn text(&self, t: &TextBlock, indent: &str, out: &mut String) -> Result<()> {
        writeln!(
            out,
            "{indent}text {} font {} justify {} {}{}",
            rect(t.rect),
            t.font,
            ui::justification(t.flags),
            self.sid(t.string),
            anim(t.animation)
        )?;
        Ok(())
    }

    fn shader_bitmap(&self, b: &ShaderBitmap, indent: &str, out: &mut String) -> Result<()> {
        writeln!(
            out,
            "{indent}shader bitmap {} {} shader {}",
            rect(b.rect),
            self.name(b.bitmap),
            self.name(b.shader)
        )?;
        Ok(())
    }

    fn bitmap(&mut self, b: &BitmapBlock, indent: &str, out: &mut String) -> Result<()> {
        let sizes = b
            .bitmap
            .id
            .map(|id| self.image_sizes(id))
            .unwrap_or_default();
        // The update uses the selected image when it is in range, else image 0.
        let index = usize::try_from(b.image)
            .ok()
            .filter(|&i| i < sizes.len())
            .unwrap_or(0);
        let size = sizes.get(index).copied().flatten();
        let kind = if b.flags & BitmapBlock::METER != 0 {
            "meter"
        } else {
            "bitmap"
        };
        let mut extra = String::new();
        if b.scroll_rate != [0.0, 0.0] {
            write!(extra, " scroll {:?}", b.scroll_rate)?;
        }
        if b.sprite_fps >= 0.001 {
            write!(extra, " fps {}", b.sprite_fps)?;
        }
        if b.render_depth_bias != 0 {
            write!(extra, " depth {}", b.render_depth_bias)?;
        }
        writeln!(
            out,
            "{indent}{kind} {} {} image {index}{extra}{}",
            rect(b.rect(size)),
            self.name(b.bitmap),
            anim(b.animation)
        )?;
        Ok(())
    }

    fn skin(&mut self, tree: &SkinTree, indent: &str, out: &mut String) -> Result<()> {
        let s = &tree.skin;
        writeln!(out, "{indent}skin widgets, relative to each item origin:")?;
        for t in &s.text_blocks {
            self.text(t, indent, out)?;
        }
        for b in &s.bitmaps {
            self.bitmap(b, indent, out)?;
        }
        for b in &s.shader_bitmaps {
            self.shader_bitmap(b, indent, out)?;
        }
        for (t, nested) in s.tables.iter().zip(&tree.tables) {
            writeln!(
                out,
                "{indent}table cells {} ({} cols x {} rows) skin {}",
                t.cells,
                t.columns,
                t.rows,
                self.name(t.skin)
            )?;
            if let Some(n) = nested {
                self.skin(n, &format!("{indent}  "), out)?;
            }
        }
        Ok(())
    }
}

const PREVIEW_USAGE: &str = "usage: h2 ui preview <screen|list> [--state NAME|all] [--focus N] [--synthetic] [--disc disc.iso] [--out DIR]\n\nDraws one of our screens (boot::ui_kit::screens::NAMES) in its named\nstates (boot::ui_kit::screens::states; default all), or its sample with\nthe focus on row N, from the disc's tags, art and fonts (or, with\n--synthetic, from layout numbers and block text, no disc). Writes\n<screen>-<state>.png and <screen>-<state>.json (every widget's rect) into\nDIR (default $TMPDIR) and prints the kit's checks and the layout checks\n(boot::ui_kit::layout). Nothing is written into\nthe repository. docs/game/ui-builder.md.";

const SHEET_USAGE: &str = "usage: h2 ui sheet [--synthetic] [--disc disc.iso] [--out DIR]\n\nEvery screen of ours in every state, as `h2 ui preview` draws them, and\nsheet.png with all of them on one page, into DIR (default\n$RUSH_KEEP/ui-builder). docs/game/ui-builder.md.";

/// The options `preview` and `sheet` share.
#[derive(Default)]
struct DrawArgs {
    name: Option<String>,
    state: Option<String>,
    focus: Option<usize>,
    synthetic: bool,
    disc: Option<PathBuf>,
    out: Option<PathBuf>,
}

fn draw_args(args: Vec<String>, usage: &str) -> Result<Option<DrawArgs>> {
    let mut args = args.into_iter();
    let mut d = DrawArgs::default();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--state" => d.state = Some(args.next().context("--state needs a name or all")?),
            "--focus" => d.focus = Some(args.next().context("--focus needs a row")?.parse()?),
            "--synthetic" => d.synthetic = true,
            "--disc" => {
                d.disc = Some(PathBuf::from(
                    args.next().context("--disc needs an ISO path")?,
                ))
            }
            "--out" => {
                d.out = Some(PathBuf::from(
                    args.next().context("--out needs a directory")?,
                ))
            }
            "-h" | "--help" => {
                println!("{usage}");
                return Ok(None);
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n{usage}"),
            _ if d.name.is_none() => d.name = Some(a),
            _ => bail!("one screen only\n{usage}"),
        }
    }
    Ok(Some(d))
}

/// `h2 ui preview`: a screen of ours drawn offline in its states
/// (docs/game/ui-builder.md).
fn preview(args: Vec<String>) -> Result<()> {
    use boot::ui_kit::{preview, screens};
    let Some(a) = draw_args(args, PREVIEW_USAGE)? else {
        return Ok(());
    };
    let name = a.name.context(PREVIEW_USAGE)?;
    if name == "list" {
        for n in screens::NAMES {
            let states: Vec<String> = screens::states(n)
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.0)
                .collect();
            println!("{n}: {}", states.join(", "));
        }
        return Ok(());
    }
    let mut states = screens::states(&name)
        .with_context(|| format!("no screen {name:?}; one of {:?}", screens::NAMES))?;
    if let Some(f) = a.focus {
        let mut s = screens::sample(&name).context("no sample")?;
        s.focus = f;
        states = vec![(format!("focus-{f}"), s)];
    } else if let Some(want) = a.state.as_deref().filter(|w| *w != "all") {
        states.retain(|(n, _)| n == want);
        if states.is_empty() {
            let known: Vec<String> = screens::states(&name)
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.0)
                .collect();
            bail!("{name} has no state {want:?}; one of {known:?}");
        }
    }
    let disc = if a.synthetic {
        None
    } else {
        let iso = a.disc.map(Ok).unwrap_or_else(super::default_disc)?;
        Some(preview::Disc::open(&iso)?)
    };
    let from = match &disc {
        Some(d) => preview::From::Disc(d),
        None => preview::From::Synthetic,
    };
    let dir = a.out.unwrap_or_else(std::env::temp_dir);
    for (state, spec) in &states {
        println!("{state}: {}", spec.log_line());
        if let Some(d) = &disc {
            for f in d.check(spec)? {
                println!("{state}: check: {f}");
            }
        }
        let (view, _) = from.draw(spec, state)?;
        let r = boot::ui_kit::layout::check(
            &view,
            from.source(),
            &boot::ui_kit::layout::Rules::for_spec(spec),
        );
        for i in &r.issues {
            println!("{state}: layout: {i}");
        }
        if !r.skipped.is_empty() {
            println!(
                "{state}: layout: not checked without art: {}",
                r.skipped.join(", ")
            );
        }
    }
    for (label, _) in preview::write_states(&from, &dir, &name, &states)? {
        let file = label.replace('/', "-");
        println!("wrote {}", dir.join(format!("{file}.png")).display());
    }
    Ok(())
}

/// `h2 ui sheet`: every screen of ours in every state, and one page of all.
fn sheet(args: Vec<String>) -> Result<()> {
    use boot::ui_kit::preview;
    let Some(a) = draw_args(args, SHEET_USAGE)? else {
        return Ok(());
    };
    if a.name.is_some() || a.state.is_some() || a.focus.is_some() {
        bail!("{SHEET_USAGE}");
    }
    let dir = match a.out {
        Some(d) => d,
        None => PathBuf::from(std::env::var_os("RUSH_KEEP").context("no --out and no $RUSH_KEEP")?)
            .join("ui-builder"),
    };
    let disc = if a.synthetic {
        None
    } else {
        let iso = a.disc.map(Ok).unwrap_or_else(super::default_disc)?;
        Some(preview::Disc::open(&iso)?)
    };
    let from = match &disc {
        Some(d) => preview::From::Disc(d),
        None => preview::From::Synthetic,
    };
    let path = preview::write_sheet(&from, &dir)?;
    println!("wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::FourCC;

    #[test]
    fn rectangles_print_in_wire_order() {
        let r = Rect {
            top: 45,
            left: -255,
            bottom: 13,
            right: -191,
        };
        assert_eq!(rect(r), "[45 -255 13 -191]");
        assert_eq!(anim(AnimationRef(0)), "");
        assert_eq!(anim(AnimationRef(31)), " anim 30");
    }

    #[test]
    fn usage_and_bad_arguments() {
        assert!(run(vec!["--help".into()]).is_ok());
        assert!(run(vec!["preview".into(), "--help".into()]).is_ok());
        assert!(run(vec!["sheet".into(), "--bogus".into()]).is_err());
        assert!(run(vec!["sheet".into(), "a".into()]).is_err());
        assert!(run(vec!["--bogus".into()]).is_err());
        assert!(run(vec!["only-one".into()]).is_err());
        // --all takes exactly one target.
        assert!(run(vec!["--all".into()]).is_err());
        assert!(run(vec!["--all".into(), "a".into(), "b".into()]).is_err());
    }

    #[test]
    fn missing_map_file_is_an_error() {
        let missing = std::env::temp_dir().join("h2-ui-test-missing.map");
        let arg = missing.to_string_lossy().into_owned();
        // Not a file, so it is looked up on a disc that does not exist.
        let disc = vec!["--disc".into(), arg.clone() + ".iso"];
        let mut a = vec!["--all".into(), arg];
        a.extend(disc);
        assert!(run(a).is_err());
    }

    #[test]
    fn image_dimensions_that_overflow_i16_are_dropped() {
        assert_eq!(image_size([64, 32, 1]), Some((64, 32)));
        assert_eq!(image_size([40_000, 32, 1]), None);
        assert_eq!(image_size([64, u16::MAX, 1]), None);
    }

    #[test]
    fn report_lists_failures_count() {
        let mut c = MapCheck::default();
        c.failures
            .push("x: map has 0 wigl tags, expected exactly 1".into());
        let r = report(1, &c);
        assert!(r.starts_with("1 maps\n"));
        assert!(r.ends_with("1 failures\n"));
        assert_eq!(
            prefixed("m", c).failures[0],
            "m: x: map has 0 wigl tags, expected exactly 1"
        );
    }

    #[test]
    fn screen_group_is_wgit() {
        assert_eq!(ui::SCREEN, FourCC(*b"wgit"));
    }
}
