//! `h2 sound`: export Halo 2 sound tags to WAV and validate them in bulk.
//! Self-contained so it merges independently of the other map commands.
use crate::safe_out::OutDir;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use audio::adpcm::{self, SamplesPerBlock};
use map::{FourCC, Map, RawPointer, ResourceFile};
use tags::sound::{Compression, Encoding, Gestalt, ResourceReader, Sound};

const USAGE: &str = "usage: h2 sound <map-name-or-file> <tag name> [--out DIR] [--force] [--disc disc.iso]\n       h2 sound --all <map-name-or-file> [--out DIR] [--force] [--check-durations] [--disc disc.iso]\n\nWrites 16-bit PCM WAV files (--out defaults to ./h2-sounds) named\n<tag path>.r<range>p<permutation>.wav. Existing files are never overwritten\nunless --force, and output cannot leave DIR through symlinks. With --all and\nno --out nothing is written: it validates every snd! tag and prints counts;\n--check-durations adds the max_play_time and block-size evidence. Exit status is\nnon-zero if any sound fails. Retail WMAv2 streams export as WAV; their decoder\ntables are loaded from the verified default.xbe on --disc.";

/// The four raw-resource files, as byte windows in the disc image or local files.
pub(super) struct Resources {
    files: [Option<(PathBuf, u64, u64)>; 4],
    xbe: Option<(PathBuf, u64, u32)>,
}
fn slot(f: ResourceFile) -> usize {
    match f {
        ResourceFile::Local => 0,
        ResourceFile::MainMenu => 1,
        ResourceFile::Shared => 2,
        ResourceFile::SinglePlayerShared => 3,
    }
}
impl ResourceReader for Resources {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        let Some((_, _, len)) = &self.files[slot(p.file)] else {
            return Err(tags::Error::MissingResource(p.file));
        };
        if u64::from(p.offset) + u64::from(size) > *len {
            return Err(tags::Error::ResourceBounds {
                file: p.file,
                offset: p.offset,
                size,
                length: *len,
            });
        }
        Ok(())
    }
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.check(p, size)?;
        let (path, base, _) = self.files[slot(p.file)].as_ref().expect("checked");
        let io = |_| tags::Error::Invalid("resource file read failed");
        let mut f = File::open(path).map_err(io)?;
        f.seek(SeekFrom::Start(base + u64::from(p.offset)))
            .map_err(io)?;
        let mut v = vec![0; size as usize];
        f.read_exact(&mut v).map_err(io)?;
        Ok(v)
    }
}

pub(super) fn open(name: &str, disc: Option<PathBuf>) -> Result<(Map<BufReader<File>>, Resources)> {
    let mut res = Resources {
        files: [None, None, None, None],
        xbe: None,
    };
    let local = Path::new(name);
    let disc_path = match disc {
        Some(d) => Some(d),
        None if local.is_file() => super::default_disc().ok(),
        None => Some(super::default_disc()?),
    };
    let mut iso_state = None;
    if let Some(dp) = &disc_path {
        let mut iso = super::open_iso(dp)?;
        let entries = iso.entries()?;
        let part = iso.partition_offset();
        if let Some(e) = entries.iter().find(|e| {
            e.path
                .trim_start_matches('/')
                .eq_ignore_ascii_case("default.xbe")
        }) {
            res.xbe = Some((
                dp.clone(),
                part + u64::from(e.sector) * xiso::SECTOR,
                e.size,
            ));
        }
        for (f, n) in [
            (ResourceFile::MainMenu, "maps/mainmenu.map"),
            (ResourceFile::Shared, "maps/shared.map"),
            (
                ResourceFile::SinglePlayerShared,
                "maps/single_player_shared.map",
            ),
        ] {
            if let Some(e) = entries
                .iter()
                .find(|e| e.path.trim_start_matches('/').eq_ignore_ascii_case(n))
            {
                res.files[slot(f)] = Some((
                    dp.clone(),
                    part + u64::from(e.sector) * xiso::SECTOR,
                    e.size.into(),
                ));
            }
        }
        iso_state = Some((entries, part));
    }
    if local.is_file() {
        let len = fs::metadata(local)?.len();
        res.files[0] = Some((local.to_path_buf(), 0, len));
        let map = Map::from_reader(BufReader::new(File::open(local)?))
            .with_context(|| format!("parsing {name}"))?;
        return Ok((map, res));
    }
    let (entries, part) = iso_state.context("no disc available")?;
    let mut path = name.trim_start_matches('/').to_owned();
    if !path.to_ascii_lowercase().ends_with(".map") {
        path.push_str(".map");
    }
    if !path.contains('/') {
        path = format!("maps/{path}");
    }
    let e = entries
        .iter()
        .find(|e| e.path.trim_start_matches('/').eq_ignore_ascii_case(&path))
        .with_context(|| format!("{path} not on disc"))?;
    let dp = disc_path.expect("disc");
    let base = part + u64::from(e.sector) * xiso::SECTOR;
    res.files[0] = Some((dp.clone(), base, e.size.into()));
    let map = Map::from_region(BufReader::new(File::open(&dp)?), base, e.size.into())
        .with_context(|| format!("parsing {}", e.path))?;
    Ok((map, res))
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let (mut positional, mut out, mut disc, mut all) = (Vec::new(), None, None, false);
    let (mut force, mut check) = (false, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => all = true,
            "--force" => force = true,
            "--check-durations" => check = true,
            "--out" => {
                out = Some(PathBuf::from(
                    args.next().context("--out needs a directory")?,
                ))
            }
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
        (true, [map]) => validate_all(
            map,
            disc,
            out.map(|o| OutDir::new(o, force)).transpose()?,
            check,
        ),
        (false, [map, tag]) if !check => export_one(
            map,
            tag,
            disc,
            OutDir::new(out.unwrap_or_else(|| "h2-sounds".into()), force)?,
        ),
        _ => bail!("{USAGE}"),
    }
}

/// Why a sound could not be decoded.
enum Fail {
    /// A format requiring a different path from the ADPCM decoder.
    Unsupported(String),
    Error(anyhow::Error),
}
impl<E: Into<anyhow::Error>> From<E> for Fail {
    fn from(e: E) -> Self {
        Fail::Error(e.into())
    }
}

/// Channels and sample rate if this sound is plain Xbox ADPCM we can decode.
fn adpcm_format(s: &Sound) -> Result<(usize, u32), Fail> {
    let h = &s.header;
    let rate = h
        .sample_rate
        .hz()
        .ok_or_else(|| Fail::Error(anyhow::anyhow!("unknown sample rate {:?}", h.sample_rate)))?;
    match (h.compression, h.encoding) {
        (Compression::XboxAdpcm, Encoding::Mono) => Ok((1, rate)),
        (Compression::XboxAdpcm, Encoding::Stereo) => Ok((2, rate)),
        (Compression::XboxAdpcm, Encoding::Codec) => Err(Fail::Unsupported(
            "codec stream requires supported ASF WMAv2".into(),
        )),
        (Compression::Wma, Encoding::Codec) => Err(Fail::Unsupported(
            "WMA stream requires supported ASF framing".into(),
        )),
        (c, e) => Err(Fail::Error(anyhow::anyhow!(
            "unsupported compression {c:?} / encoding {e:?}"
        ))),
    }
}

/// Largest RIFF chunk payload: sizes are u32 and include 36 header bytes.
const WAV_MAX_DATA: u64 = u32::MAX as u64 - 36;

fn wav_bytes(pcm: &[i16], channels: u16, rate: u32) -> Result<Vec<u8>> {
    let data = (pcm.len() as u64)
        .checked_mul(2)
        .filter(|d| *d <= WAV_MAX_DATA);
    let data = data.context("PCM data exceeds the 4 GiB WAV/RIFF limit")?;
    let byte_rate = u64::from(rate) * u64::from(channels) * 2;
    let byte_rate = u32::try_from(byte_rate).context("WAV byte rate overflows u32")?;
    let align = channels
        .checked_mul(2)
        .context("WAV block align overflows")?;
    let data = data as u32;
    let mut v = Vec::with_capacity(44 + data as usize);
    v.extend(b"RIFF");
    v.extend((36 + data).to_le_bytes());
    v.extend(b"WAVEfmt ");
    v.extend(16u32.to_le_bytes());
    v.extend(1u16.to_le_bytes());
    v.extend(channels.to_le_bytes());
    v.extend(rate.to_le_bytes());
    v.extend(byte_rate.to_le_bytes());
    v.extend(align.to_le_bytes());
    v.extend(16u16.to_le_bytes());
    v.extend(b"data");
    v.extend(data.to_le_bytes());
    for s in pcm {
        v.extend(s.to_le_bytes());
    }
    Ok(v)
}

/// One unambiguous scheme everywhere: `<tag path>.r<range>p<permutation>.<ext>`.
/// A tag called `foo.r0p0` yields `foo.r0p0.r0p0.wav`, so names cannot collide.
fn out_components(tag: &str, range: usize, perm: usize, ext: &str) -> Vec<String> {
    let mut parts: Vec<String> = tag.split('\\').map(str::to_owned).collect();
    if let Some(last) = parts.last_mut() {
        *last = format!("{last}.r{range}p{perm}.{ext}");
    }
    parts
}

fn wma_pcm(res: &Resources, data: &[u8], tables: &mut Option<wma::Tables>) -> Result<Vec<i16>> {
    if tables.is_none() {
        let (path, base, size) = res
            .xbe
            .as_ref()
            .context("WMA needs --disc with the retail default.xbe")?;
        anyhow::ensure!(*size <= 16 * 1024 * 1024, "XBE exceeds 16 MiB");
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(*base))?;
        let mut bytes = vec![0; *size as usize];
        file.read_exact(&mut bytes)?;
        let image = wma::VerifiedImage::load(&bytes)?;
        *tables = Some(wma::Tables::load(&image)?);
    }
    Ok(wma::decode_asf(tables.as_ref().expect("loaded"), data, 128 * 1024 * 1024)?.pcm)
}

fn codec_stream(sound: &Sound) -> bool {
    matches!(
        (sound.header.compression, sound.header.encoding),
        (Compression::Wma | Compression::XboxAdpcm, Encoding::Codec)
    )
}

fn export_one(map_name: &str, tag: &str, disc: Option<PathBuf>, out: OutDir) -> Result<()> {
    let (mut map, mut res) = open(map_name, disc)?;
    let id = map
        .tag(map::TagQuery::NameInGroup(tag, FourCC(*b"snd!")))?
        .id;
    let header = tags::sound::read_header(&mut map, id)?;
    let gestalt = Gestalt::read(&mut map)?;
    let sound = gestalt.resolve(&header)?;
    println!(
        "{tag}: {:?} {:?} {:?} max {} ms, {} pitch range(s)",
        header.sample_rate,
        header.encoding,
        header.compression,
        header.max_play_time,
        sound.ranges.len()
    );
    let mut wma_tables = None;
    for (ri, r) in sound.ranges.iter().enumerate() {
        for (pi, p) in r.permutations.iter().enumerate() {
            let data = gestalt.read_encoded(p, &mut res)?;
            match adpcm_format(&sound) {
                Ok((ch, rate)) => {
                    let pcm = adpcm::decode(&data, ch)?;
                    let wav = wav_bytes(&pcm, ch as u16, rate)?;
                    let path = out.write(&out_components(tag, ri, pi, "wav"), &wav)?;
                    let frames = pcm.len() / ch;
                    println!(
                        "  wrote {} ({ch} ch, {rate} Hz, {frames} frames, {} ms)",
                        path.display(),
                        frames as u64 * 1000 / u64::from(rate)
                    );
                }
                Err(Fail::Unsupported(_)) if data.starts_with(&asf::HEADER) => {
                    let pcm = wma_pcm(&res, &data, &mut wma_tables)?;
                    let wav = wav_bytes(&pcm, 2, 44100)?;
                    let path = out.write(&out_components(tag, ri, pi, "wav"), &wav)?;
                    println!(
                        "  wrote {} (2 ch, 44100 Hz, {} frames)",
                        path.display(),
                        pcm.len() / 2
                    );
                }
                Err(Fail::Unsupported(why)) => bail!("{tag}: {why}"),
                Err(Fail::Error(e)) => return Err(e.context(tag.to_owned())),
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct Stats {
    /// Sum of |delta| across block boundaries / interior, per reading, channel 0.
    boundary: [f64; 2],
    boundary_n: [u64; 2],
    interior: [f64; 2],
    interior_n: [u64; 2],
}
impl Stats {
    fn add(&mut self, data: &[u8], ch: usize) {
        for (k, mode) in [SamplesPerBlock::Hardware64, SamplesPerBlock::Ima65]
            .into_iter()
            .enumerate()
        {
            let Ok(pcm) = adpcm::decode_with(data, ch, mode) else {
                return;
            };
            let n = mode.count();
            let frames = pcm.len() / ch;
            for f in 1..frames {
                let d = (i32::from(pcm[f * ch]) - i32::from(pcm[(f - 1) * ch])).abs() as f64;
                if f % n == 0 {
                    self.boundary[k] += d;
                    self.boundary_n[k] += 1;
                } else {
                    self.interior[k] += d;
                    self.interior_n[k] += 1;
                }
            }
        }
    }
}

fn validate_all(
    map_name: &str,
    disc: Option<PathBuf>,
    out: Option<OutDir>,
    check: bool,
) -> Result<()> {
    let (mut map, mut res) = open(map_name, disc)?;
    let gestalt = Gestalt::read(&mut map)?;
    let ids: Vec<_> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"snd!"))
        .map(|t| (t.id, t.name.clone().unwrap_or_default()))
        .collect();
    let mut by_kind = BTreeMap::<String, usize>::new();
    let mut failures = BTreeMap::<String, Vec<String>>::new();
    let mut unsupported = BTreeMap::<String, Vec<String>>::new();
    let mut st = Stats::default();
    let mut wma_tables = None;
    let mut wma_permutations = 0u64;
    let (mut perms, mut bytes, mut blocks) = (0u64, 0u64, 0u64);
    let (mut dur_ok, mut dur_ok65, mut dur_checked) = (0, 0, 0);
    let (mut misaligned, mut clipped, mut align_checked, mut max_clip_ratio) =
        (0u64, 0u64, 0u64, 0f64);
    let (mut wrote, mut ok) = (0, 0);
    let mut mismatches: Vec<String> = Vec::new();
    for (id, name) in &ids {
        let result: Result<(), Fail> = (|| {
            let header = tags::sound::read_header(&mut map, *id)?;
            let sound = gestalt.resolve(&header)?;
            let kind = format!(
                "{:?}/{:?}/{}",
                header.compression,
                header.encoding,
                header
                    .sample_rate
                    .hz()
                    .map_or("?".into(), |r| r.to_string())
            );
            *by_kind.entry(kind).or_default() += 1;
            if codec_stream(&sound) {
                for (ri, r) in sound.ranges.iter().enumerate() {
                    for (pi, p) in r.permutations.iter().enumerate() {
                        let data = gestalt.read_encoded(p, &mut res)?;
                        let pcm = wma_pcm(&res, &data, &mut wma_tables)?;
                        wma_permutations += 1;
                        if let Some(out) = &out {
                            out.write(
                                &out_components(name, ri, pi, "wav"),
                                &wav_bytes(&pcm, 2, 44100)?,
                            )?;
                            wrote += 1;
                        }
                    }
                }
                return Ok(());
            }
            let (ch, rate) = match adpcm_format(&sound) {
                Ok(v) => v,
                Err(f) => return Err(f),
            };
            let (mut ms64, mut ms65) = (0u64, 0u64);
            for (ri, r) in sound.ranges.iter().enumerate() {
                for (pi, p) in r.permutations.iter().enumerate() {
                    let data = gestalt.read_encoded(p, &mut res)?;
                    let unit = 36 * ch;
                    align_checked += 1;
                    if data.len() % unit != 0 {
                        misaligned += 1;
                        return Err(anyhow::anyhow!(
                            "encoded size {} is not a multiple of the {unit}-byte block",
                            data.len()
                        )
                        .into());
                    }
                    let pcm = adpcm::decode(&data, ch)?;
                    let nblocks = (data.len() / unit) as u64;
                    perms += 1;
                    bytes += data.len() as u64;
                    blocks += nblocks;
                    ms64 = ms64.max(nblocks * 64 * 1000 / u64::from(rate));
                    ms65 = ms65.max(nblocks * 65 * 1000 / u64::from(rate));
                    let clips = pcm
                        .iter()
                        .filter(|&&s| s == i16::MAX || s == i16::MIN)
                        .count();
                    if clips > 0 {
                        clipped += 1;
                    }
                    max_clip_ratio = max_clip_ratio.max(clips as f64 / pcm.len().max(1) as f64);
                    if check {
                        st.add(&data, ch);
                    }
                    if let Some(out) = &out {
                        let wav = wav_bytes(&pcm, ch as u16, rate)?;
                        out.write(&out_components(name, ri, pi, "wav"), &wav)?;
                        wrote += 1;
                    }
                }
            }
            if check {
                dur_checked += 1;
                dur_ok += usize::from(ms64 == u64::from(header.max_play_time));
                if ms64 != u64::from(header.max_play_time) {
                    mismatches.push(format!(
                        "{name}: tag {} ms, decoded {ms64} ms",
                        header.max_play_time
                    ));
                }
                dur_ok65 += usize::from(ms65 == u64::from(header.max_play_time));
            }
            Ok(())
        })();
        match result {
            Ok(()) => ok += 1,
            Err(Fail::Unsupported(why)) => unsupported.entry(why).or_default().push(name.clone()),
            Err(Fail::Error(e)) => failures
                .entry(format!("{e:#}"))
                .or_default()
                .push(name.clone()),
        }
    }
    let n_unsupported: usize = unsupported.values().map(Vec::len).sum();
    let n_failed: usize = failures.values().map(Vec::len).sum();
    println!(
        "{} snd! tags in {map_name}; {ok} decoded OK, {n_unsupported} unsupported format, {n_failed} FAILED",
        ids.len()
    );
    println!("\nby compression/encoding/rate");
    for (k, n) in &by_kind {
        println!("  {k:<32} {n}");
    }
    println!("\nunsupported formats (not failures)");
    for (why, names) in &unsupported {
        println!("  {} x  {why}   e.g. {}", names.len(), names[0]);
    }
    println!("\nfailure reasons");
    for (why, names) in &failures {
        println!("  {} x  {why}   e.g. {}", names.len(), names[0]);
    }
    println!("\nWMAv2 permutations decoded: {wma_permutations}");
    println!(
        "\nADPCM permutations decoded: {perms} ({bytes} bytes, {blocks} blocks per channel-group)"
    );
    println!(
        "block alignment: {misaligned} of {align_checked} permutations not a whole number of blocks"
    );
    println!(
        "permutations containing full-scale samples: {clipped} (worst fraction {max_clip_ratio:.5})"
    );
    if check {
        println!(
            "max_play_time == longest permutation (floor ms): 64/block {dur_ok}/{dur_checked}, 65/block {dur_ok65}/{dur_checked}"
        );
        for m in mismatches.iter().take(8) {
            println!("  duration mismatch (64/block): {m}");
        }
        for (k, label) in ["64/block", "65/block"].iter().enumerate() {
            let b = st.boundary[k] / st.boundary_n[k].max(1) as f64;
            let i = st.interior[k] / st.interior_n[k].max(1) as f64;
            println!(
                "mean |step| at block boundary vs interior, {label} reading: {b:.1} vs {i:.1} (ratio {:.3}, {} boundaries)",
                b / i.max(1e-9),
                st.boundary_n[k]
            );
        }
    }
    if out.is_some() {
        println!("wrote {wrote} WAV files");
    }
    if n_failed > 0 {
        bail!("{n_failed} of {} sounds failed", ids.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_canonical() {
        let w = wav_bytes(&[1, -2, 3, -4], 2, 44100).unwrap();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize,
            w.len() - 8
        );
        assert_eq!(u16::from_le_bytes(w[22..24].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 44100);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 8);
        assert_eq!(i16::from_le_bytes(w[46..48].try_into().unwrap()), -2);
    }

    #[test]
    fn wav_limits_are_checked_not_truncated() {
        // A header-only bound check: the limit constant leaves room for the 36
        // header bytes, and absurd byte rates are rejected rather than wrapped.
        assert_eq!(WAV_MAX_DATA + 36, u64::from(u32::MAX));
        assert!(wav_bytes(&[0; 4], 2, u32::MAX).is_err());
        assert!(wav_bytes(&[0; 4], u16::MAX, 44100).is_err());
    }

    #[test]
    fn output_names_are_unambiguous() {
        assert_eq!(
            out_components("sound\\ui\\click", 0, 1, "wav"),
            ["sound", "ui", "click.r0p1.wav"]
        );
        // A single-permutation sound and permutation 0 of a sibling named like
        // the suffix can never produce the same file.
        assert_ne!(
            out_components("foo", 0, 0, "wav"),
            out_components("foo.r0p0", 0, 0, "wav")
        );
        assert_eq!(
            out_components("foo.r0p0", 0, 0, "wav"),
            ["foo.r0p0.r0p0.wav"]
        );
    }

    #[test]
    fn unsafe_tag_names_are_refused_by_the_output_dir() {
        let d = std::env::temp_dir().join(format!("h2-sound-names-{}", std::process::id()));
        let out = OutDir::new(&d, true).unwrap();
        assert!(
            out.write(&out_components("sound\\..\\evil", 0, 0, "wav"), b"x")
                .is_err()
        );
        assert!(
            out.write(&out_components("a/b", 0, 0, "wav"), b"x")
                .is_err()
        );
        let _ = fs::remove_dir_all(d);
    }
}
