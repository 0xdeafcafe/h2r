//! The whole Halo 1 import: every campaign level and every multiplayer map
//! (both sandboxes) of a Halo: Combat Evolved disc, converted onto the
//! Halo 2 disc's maps and written into the data root the game reads
//! (docs/game/campaigns.md "Importing", docs/game/h1-multiplayer.md
//! "Importing").
//!
//! - campaign levels: `<data>/campaigns/halo1/<level>.map` and
//!   `campaign.toml`, which the campaign picker lists and
//!   `vfs::CampaignMapsBackend` serves as `maps\<level>.map`;
//! - multiplayer maps: the package `<data>/maps/h1_<map>/` with both
//!   variants, which `boot::new_maps` puts in the map list.
//!
//! [`STAMP_FILE`] in `<data>/campaigns/halo1/` records the disc (SHA-256,
//! size, modification time), [`CONVERTER_VERSION`] and each map's outcome,
//! written after every map, so a rerun (or the next `h2 play`) does only
//! what is missing, and an interrupted import resumes.
//!
//! `h2 play`, `h2 run`, `h2 serve` and `h2 play --follow` run it before
//! booting when `halo1.iso` is next to the Halo 2 disc ([`auto_import`]).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use h1convert::convert::{Sandbox, known, map_name, structure_mopp, variant_file};
use h1map::H1Map;
use h1map::disc::{CAMPAIGN_LEVELS, Disc, map_stem};
use sha2::{Digest, Sha256};
use xiso::Xiso;

use crate::h1_cmd;
use crate::safe_out::OutDir;

/// The converters' output version. **Bump it whenever a change to the
/// Halo 1 converters (h1convert, h1campaign, h1objects, h1ai, h1scripts,
/// h1sounds, h1bitmaps, h1models, h1sky, h1text, or the map builder they
/// use) changes the maps they write**: every import stamped with an older
/// version is converted again at the next `h2 play`, so `--follow` users
/// get the new maps after the merge.
pub const CONVERTER_VERSION: u32 = 14;

/// The stamp's file, in `<data>/campaigns/halo1/`.
pub const STAMP_FILE: &str = "import.stamp";
/// The Halo 1 disc's name beside the Halo 2 disc.
pub const AUTO_NAME: &str = "halo1.iso";
/// The campaign's folder under `<data>/campaigns`.
const CAMPAIGN_KEY: &str = h1campaign::manifest::KEY;
/// The sandboxes every multiplayer map is converted in.
const SANDBOXES: [Sandbox; 2] = [Sandbox::Halo2, Sandbox::Halo1];

/// One map to convert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job<'a> {
    /// A campaign level, `a10` .. `d40`.
    Campaign(&'a str),
    /// A multiplayer map in one sandbox.
    Multiplayer(&'a str, Sandbox),
}

impl Job<'_> {
    /// Its key in the stamp: `a10`, `bloodgulch/h2`.
    pub fn key(&self) -> String {
        match self {
            Job::Campaign(l) => l.to_string(),
            Job::Multiplayer(m, s) => format!("{m}/{}", s.name()),
        }
    }

    /// What its line calls it.
    pub fn label(&self) -> String {
        match self {
            Job::Campaign(l) => format!("campaign {l}"),
            Job::Multiplayer(m, s) => format!(
                "multiplayer {m} ({} sandbox)",
                if *s == Sandbox::Halo1 { "Halo 1" } else { "Halo 2" }
            ),
        }
    }

    /// The map file it writes under the data root.
    pub fn output(&self, data: &Path) -> PathBuf {
        match self {
            Job::Campaign(l) => campaign_dir(data).join(h1campaign::manifest::map_file(l)),
            Job::Multiplayer(m, s) => content::store::maps_dir(data)
                .join(map_name(m))
                .join(variant_file(m, *s)),
        }
    }
}

/// `<data>/campaigns/halo1`.
pub fn campaign_dir(data: &Path) -> PathBuf {
    data.join("campaigns").join(CAMPAIGN_KEY)
}

/// The jobs for a disc whose `maps/` holds `stems`: each campaign level it
/// has, in play order, then each multiplayer map the converter knows, in
/// its table's order, in both sandboxes.
pub fn jobs(stems: &[String]) -> Vec<Job<'static>> {
    let has = |n: &str| stems.iter().any(|s| s.eq_ignore_ascii_case(n));
    let mut out: Vec<Job> = CAMPAIGN_LEVELS
        .iter()
        .filter(|l| has(l))
        .map(|l| Job::Campaign(l))
        .collect();
    for (m, _, _) in h1convert::convert::MAPS.iter().filter(|(m, _, _)| has(m)) {
        out.extend(SANDBOXES.map(|s| Job::Multiplayer(m, s)));
    }
    out
}

/// What was converted from which disc.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    pub converter: u32,
    /// The Halo 1 disc's SHA-256, hex.
    pub sha256: String,
    /// Its size and modification time (seconds), so an unchanged disc is
    /// not hashed again.
    pub size: u64,
    pub mtime: u64,
    /// By job key: `Ok` when written, else why it failed.
    pub done: BTreeMap<String, Result<(), String>>,
}

impl Stamp {
    pub fn to_text(&self) -> String {
        let mut s = format!(
            "# h2 import halo1 (docs/game/campaigns.md \"Importing\"). Delete this file, or\n\
             # run `h2 import halo1 --force`, to convert everything again.\n\
             converter={}\nsha256={}\nsize={}\nmtime={}\n",
            self.converter, self.sha256, self.size, self.mtime
        );
        for (k, r) in &self.done {
            match r {
                Ok(()) => s += &format!("map {k} ok\n"),
                Err(e) => s += &format!("map {k} failed {}\n", e.replace('\n', " ")),
            }
        }
        s
    }

    /// The stamp in `text`, or `None` when it isn't one.
    pub fn parse(text: &str) -> Option<Self> {
        let mut st = Stamp::default();
        let mut seen = 0;
        for line in text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            if let Some(rest) = line.strip_prefix("map ") {
                let (key, rest) = rest.split_once(' ')?;
                let r = if rest == "ok" {
                    Ok(())
                } else {
                    Err(rest.strip_prefix("failed ").unwrap_or(rest).to_owned())
                };
                st.done.insert(key.to_owned(), r);
                continue;
            }
            let (k, v) = line.split_once('=')?;
            match k {
                "converter" => st.converter = v.parse().ok()?,
                "sha256" => st.sha256 = v.to_owned(),
                "size" => st.size = v.parse().ok()?,
                "mtime" => st.mtime = v.parse().ok()?,
                _ => return None,
            }
            seen += 1;
        }
        (seen == 4).then_some(st)
    }

    /// The stamp in `<data>/campaigns/halo1/`, if there is a readable one.
    pub fn read(data: &Path) -> Option<Self> {
        Self::parse(&std::fs::read_to_string(campaign_dir(data).join(STAMP_FILE)).ok()?)
    }

    fn write(&self, data: &Path) -> Result<()> {
        let dir = campaign_dir(data);
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(".{STAMP_FILE}.tmp-{}", std::process::id()));
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, dir.join(STAMP_FILE))?;
        Ok(())
    }
}

/// A disc's size and modification time.
fn size_mtime(iso: &Path) -> Result<(u64, u64)> {
    let m = std::fs::metadata(iso).with_context(|| format!("reading {}", iso.display()))?;
    let mtime = m
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok((m.len(), mtime))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h1map::disc::hex(&h.finalize()))
}

/// The stamp to carry on from for this disc and this converter: `old`'s
/// outcomes when both are the same, else none. The disc is hashed only
/// when its size or modification time differ from `old`'s.
pub fn fresh_stamp(
    old: Option<&Stamp>,
    size: u64,
    mtime: u64,
    hash: impl FnOnce() -> Result<String>,
) -> Result<Stamp> {
    let sha256 = match old {
        Some(o) if o.size == size && o.mtime == mtime && !o.sha256.is_empty() => o.sha256.clone(),
        _ => hash()?,
    };
    let done = match old {
        Some(o) if o.converter == CONVERTER_VERSION && o.sha256 == sha256 => o.done.clone(),
        _ => BTreeMap::new(),
    };
    Ok(Stamp {
        converter: CONVERTER_VERSION,
        sha256,
        size,
        mtime,
        done,
    })
}

/// The jobs still to do: not in the stamp, or written once but whose map
/// file is gone. A map that failed with this converter and this disc is
/// not tried again (`--force` does).
pub fn pending<'a>(jobs: &[Job<'a>], stamp: &Stamp, data: &Path) -> Vec<Job<'a>> {
    jobs.iter()
        .filter(|j| match stamp.done.get(&j.key()) {
            None => true,
            Some(Ok(())) => !j.output(data).is_file(),
            Some(Err(_)) => false,
        })
        .copied()
        .collect()
}

/// Converts one map and writes it.
pub trait Converter {
    fn convert(&mut self, job: &Job) -> Result<()>;
}

/// What a run did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub converted: usize,
    pub failed: usize,
    /// Already done before this run.
    pub skipped: usize,
}

/// Runs `todo` in order, one line each through `say`; a failure (or a
/// panic) in one is recorded and the rest go on. The stamp is saved after
/// each.
pub fn run_jobs(
    todo: &[Job],
    stamp: &mut Stamp,
    conv: &mut dyn Converter,
    save: &mut dyn FnMut(&Stamp) -> Result<()>,
    say: &mut dyn FnMut(String),
) -> Summary {
    let mut sum = Summary::default();
    for (i, job) in todo.iter().enumerate() {
        let t = Instant::now();
        let r = catch_unwind(AssertUnwindSafe(|| conv.convert(job)))
            .unwrap_or_else(|p| Err(anyhow!("panicked: {}", panic_text(&p))));
        let n = format!("[{}/{}]", i + 1, todo.len());
        match &r {
            Ok(()) => {
                sum.converted += 1;
                say(format!("{n} {}: done ({:.1} s)", job.label(), t.elapsed().as_secs_f32()));
            }
            Err(e) => {
                sum.failed += 1;
                say(format!("{n} {}: failed: {e:#}", job.label()));
            }
        }
        stamp
            .done
            .insert(job.key(), r.map_err(|e| format!("{e:#}")));
        if let Err(e) = save(stamp) {
            say(format!("could not save the import stamp: {e:#}"));
        }
    }
    sum
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

/// The real converter: maps from the Halo 1 disc, bases from the Halo 2
/// disc, written under the data root.
struct DiscConverter {
    h1_path: PathBuf,
    iso: Xiso<BufReader<File>>,
    disc: Disc,
    h2: PathBuf,
    data: PathBuf,
    /// Built once, for the first campaign level.
    scripting: Option<Result<Option<h1campaign::Scripting>, String>>,
    /// Read once, for the first campaign level.
    donors: Option<h1campaign::donor::Donors>,
    /// The disc's `ui.map` (the multiplayer maps' cards), read once.
    card_ui: Option<Option<H1Map>>,
}

impl DiscConverter {
    fn read(&mut self, name: &str) -> Result<(Vec<u8>, H1Map)> {
        let e = self
            .disc
            .map(name)
            .with_context(|| format!("no {name}.map on the disc"))?
            .clone();
        let bytes = self.iso.read(&e)?;
        let m = H1Map::from_reader(Cursor::new(bytes.as_slice())).with_context(|| e.path.clone())?;
        Ok((bytes, m))
    }

    fn out(&self, root: PathBuf) -> Result<OutDir> {
        // Ours to replace: whatever is there came from an import or a
        // convert of the same map.
        Ok(OutDir::new(root, true)?.protecting(&self.h1_path))
    }
}

impl Converter for DiscConverter {
    fn convert(&mut self, job: &Job) -> Result<()> {
        match *job {
            Job::Campaign(level) => {
                let (_, h1) = self.read(level)?;
                if self.scripting.is_none() {
                    self.scripting = Some(
                        h1_cmd::campaign_scripting(&self.h1_path, &self.h2).map_err(|e| format!("{e:#}")),
                    );
                }
                if self.donors.is_none() {
                    self.donors = Some(h1_cmd::campaign_donors(&self.h2)?);
                }
                let scripting = match self.scripting.as_ref().expect("set") {
                    Ok(s) => s.as_ref(),
                    Err(e) => bail!("the scripts' tables: {e}"),
                };
                let mut base = h1_cmd::h2_base(&self.h2, h1campaign::base_for(level))?;
                let opts = h1_cmd::campaign_options(scripting, self.donors.as_ref());
                let done = h1campaign::convert(&h1, &mut base, &opts)?;
                let dir = campaign_dir(&self.data);
                h1_cmd::write_campaign_level(&self.out(dir.clone())?, &dir, level, &done.bytes)?;
            }
            Job::Multiplayer(name, sandbox) => {
                let (file, h1) = self.read(name)?;
                let base_name = known(name).map_or("beavercreek", |k| k.1);
                let mut base = h1_cmd::h2_base(&self.h2, base_name)?;
                let p = h1_cmd::prepare_mp(&h1, &file, &mut base, base_name, sandbox)?;
                let mopp = structure_mopp(&p.collision)?;
                // Fresh: nothing of the base pinned.
                let f = p.build_fresh(&mut base, &mopp)?;
                let done = h1convert::convert::Converted { manifest: p.manifest(&f.bytes), name: f.name, bytes: f.bytes };
                if self.card_ui.is_none() {
                    self.card_ui = Some(self.read("ui").ok().map(|(_, m)| m));
                }
                let card = self
                    .card_ui
                    .as_ref()
                    .and_then(|ui| ui.as_ref())
                    .and_then(|ui| h1convert::ui::mp_card(ui, name).ok().flatten());
                let root = content::store::maps_dir(&self.data);
                h1_cmd::write_mp_package(&self.out(root.clone())?, &root, sandbox, &done, card.as_ref())?;
            }
        }
        Ok(())
    }
}

/// What [`import`] is asked to do.
pub struct Request<'a> {
    pub h1_iso: &'a Path,
    pub h2_disc: &'a Path,
    pub data: &'a Path,
    /// Convert everything again, whatever the stamp says.
    pub force: bool,
    /// Say what would be converted; write nothing.
    pub dry_run: bool,
}

/// The import: opens both discs, works out what is missing, converts it.
/// `Err` only when nothing could start (a disc unreadable); a map that
/// fails is a line and a count.
pub fn import(r: &Request, say: &mut dyn FnMut(String)) -> Result<Summary> {
    crate::safe_out::protect(r.h1_iso);
    crate::safe_out::protect(r.h2_disc);
    let f = File::open(r.h1_iso).with_context(|| format!("opening {}", r.h1_iso.display()))?;
    let mut iso = Xiso::open(BufReader::new(f)).with_context(|| r.h1_iso.display().to_string())?;
    let disc = Disc::open(&mut iso)?;
    if !r.h2_disc.is_file() {
        bail!("no Halo 2 disc at {} (give --disc)", r.h2_disc.display());
    }
    let stems: Vec<String> = disc.maps.iter().map(|e| map_stem(e).to_owned()).collect();
    let all = jobs(&stems);
    if all.is_empty() {
        bail!("{} has no Halo 1 campaign or multiplayer maps", r.h1_iso.display());
    }
    let (size, mtime) = size_mtime(r.h1_iso)?;
    let old = if r.force { None } else { Stamp::read(r.data) };
    if old.as_ref().is_none_or(|o| o.size != size || o.mtime != mtime) {
        say(format!(
            "checking {} ({:.1} GB)",
            r.h1_iso.display(),
            size as f64 / 1e9
        ));
    }
    let mut stamp = fresh_stamp(old.as_ref(), size, mtime, || sha256_file(r.h1_iso))?;
    if r.force {
        stamp.done.clear();
    }
    let todo = pending(&all, &stamp, r.data);
    let skipped = all.len() - todo.len();
    if r.dry_run {
        for j in &todo {
            say(format!("would convert {} to {}", j.label(), j.output(r.data).display()));
        }
        return Ok(Summary {
            skipped,
            ..Summary::default()
        });
    }
    if todo.is_empty() {
        return Ok(Summary {
            skipped,
            ..Summary::default()
        });
    }
    say(format!(
        "converting {} of {} maps from {} into {} (converter version {CONVERTER_VERSION})",
        todo.len(),
        all.len(),
        r.h1_iso.display(),
        r.data.display()
    ));
    let mut conv = DiscConverter {
        h1_path: r.h1_iso.to_path_buf(),
        iso,
        disc,
        h2: r.h2_disc.to_path_buf(),
        data: r.data.to_path_buf(),
        scripting: None,
        donors: None,
            card_ui: None,
    };
    let data = r.data.to_path_buf();
    let mut sum = run_jobs(&todo, &mut stamp, &mut conv, &mut |s| s.write(&data), say);
    sum.skipped = skipped;
    Ok(sum)
}

// ---------------------------------------------------------------------------
// Automatic import before the game starts.

/// Whether the automatic import runs: not with `--no-halo1`, and not with
/// `H2_HALO1` set to `0`, `false`, `no` or `off`.
pub fn auto_enabled(no_flag: bool) -> bool {
    auto_enabled_with(no_flag, std::env::var("H2_HALO1").ok().as_deref())
}

fn auto_enabled_with(no_flag: bool, env: Option<&str>) -> bool {
    let off = env.is_some_and(|v| {
        ["0", "false", "no", "off"].contains(&v.trim().to_ascii_lowercase().as_str())
    });
    !no_flag && !off
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// Whether `path` is a Halo 1 disc: reads its directory and XBE header.
fn is_halo1(path: &Path) -> bool {
    let Ok(f) = File::open(path) else { return false };
    let Ok(mut iso) = Xiso::open(BufReader::new(f)) else {
        return false;
    };
    Disc::open(&mut iso).is_ok()
}

/// The Halo 1 disc beside the game disc: `halo1.iso` (any case), else the
/// first other `.iso` (by name) that is a Halo 1 disc. The map pack's
/// `mappack.iso` is never opened.
pub fn discover(game_disc: &Path) -> Option<PathBuf> {
    let dir = match game_disc.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut isos: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("iso")))
        .filter(|p| p.is_file() && !same_file(p, game_disc))
        .collect();
    isos.sort();
    let named = |p: &PathBuf, n: &str| p.file_name().is_some_and(|f| f.eq_ignore_ascii_case(n));
    if let Some(i) = isos.iter().position(|p| named(p, AUTO_NAME)) {
        return Some(isos.swap_remove(i));
    }
    isos.into_iter()
        .filter(|p| !named(p, "mappack.iso"))
        .find(|p| is_halo1(p))
}

/// Before the game starts: finds the Halo 1 disc beside `game_disc` and
/// imports what is missing or stale into `data`, a line per map. Never
/// fails: a problem is a warning and the game starts anyway.
pub fn auto_import(game_disc: &Path, data: &Path) {
    let r = catch_unwind(AssertUnwindSafe(|| try_auto(game_disc, data)))
        .unwrap_or_else(|p| Err(anyhow!("panicked: {}", panic_text(&p))));
    if let Err(e) = r {
        eprintln!("halo1: not imported: {e:#}");
    }
}

fn try_auto(game_disc: &Path, data: &Path) -> Result<()> {
    let Some(h1) = discover(game_disc) else {
        return Ok(());
    };
    // The bases come from the Halo 2 disc image; a bare default.xbe has
    // none.
    let is_iso = File::open(game_disc)
        .ok()
        .is_some_and(|f| Xiso::open(BufReader::new(f)).is_ok());
    if !is_iso {
        bail!(
            "{} is not a disc image, and the conversion needs the Halo 2 disc's maps",
            game_disc.display()
        );
    }
    let name = h1.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut say = |s: String| eprintln!("halo1: {s}");
    let sum = import(
        &Request {
            h1_iso: &h1,
            h2_disc: game_disc,
            data,
            force: false,
            dry_run: false,
        },
        &mut say,
    )?;
    if sum.converted == 0 && sum.failed == 0 {
        println!("halo1: up to date ({} maps from {name})", sum.skipped);
    } else {
        println!(
            "halo1: {} maps converted, {} failed, {} already there (from {name}); \
             Campaign -> Halo: Combat Evolved, and h1_* maps in the multiplayer list",
            sum.converted, sum.failed, sum.skipped
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "h1_import_tests.rs"]
mod tests;
