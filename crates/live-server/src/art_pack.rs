//! Packing extracted game art for the web (docs/live/live-art.md,
//! "Packing"): the last step of `h2 live assets extract`, so both homes
//! (this server's `/art/game/` and the published bucket the Worker
//! redirects to) send the same bytes.
//!
//! - A glb's vertex and index data is compressed with
//!   `EXT_meshopt_compression` (meshoptimizer's codecs, lossless: no
//!   quantisation, no filters) when that makes what is sent (its Brotli
//!   copy) at least 5 % smaller ([`MESHOPT_GAIN`]); the browser decodes it
//!   in WebAssembly. The extension is then required: a reader needs a
//!   meshopt decoder (the web viewer's `viewer/gltf.ts` sets one).
//! - Every file that is not compressed already (glb, JSON) gets a Brotli
//!   copy (`<file>.br`, quality 11) and a gzip one (`<file>.gz`), for a
//!   browser that does not take Brotli (plain HTTP on a LAN). [`crate::art`]
//!   serves and publishes them with their `Content-Encoding`.
//!
//! Packing is idempotent: a glb already compressed is left as it is, and
//! the compressed copies are written again from the file.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The extension's name.
pub const MESHOPT: &str = "EXT_meshopt_compression";
/// A glb is kept meshopt-compressed only when its Brotli copy is under this
/// percentage of the plain glb's Brotli copy.
pub const MESHOPT_GAIN: usize = 95;

/// What [`pack`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackStats {
    /// glbs compressed now, and their bytes before and after.
    pub glbs: usize,
    pub glb_before: u64,
    pub glb_after: u64,
    /// Files given compressed copies, their bytes, and their Brotli copies' bytes.
    pub precompressed: usize,
    pub plain: u64,
    pub brotli: u64,
}

/// Whether a file is worth a compressed copy: everything but pictures,
/// which are compressed already.
pub fn compressible(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("glb" | "gltf" | "json" | "bin" | "txt" | "svg" | "csv")
    )
}

/// Every file under `dir` but the compressed copies.
fn files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let path = entry.path();
            if ty.is_dir() {
                todo.push(path);
            } else if ty.is_file() && !is_copy(&path) {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A `.br` or `.gz` copy of another file.
pub fn is_copy(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "br" || e == "gz")
}

/// The path of `path`'s copy with `ext` (`br`, `gz`) added.
pub fn copy_path(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

/// Packs the art in `dir` in place (module docs), on every core.
pub fn pack(dir: &Path) -> std::io::Result<PackStats> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let files = files(dir)?;
    let next = AtomicUsize::new(0);
    let stats = Mutex::new(PackStats::default());
    let first_err: Mutex<Option<std::io::Error>> = Mutex::new(None);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    match pack_file(path) {
                        Ok(one) => {
                            let mut st = stats.lock().unwrap();
                            st.glbs += one.glbs;
                            st.glb_before += one.glb_before;
                            st.glb_after += one.glb_after;
                            st.precompressed += one.precompressed;
                            st.plain += one.plain;
                            st.brotli += one.brotli;
                        }
                        Err(e) => {
                            let e = std::io::Error::new(
                                e.kind(),
                                format!("packing {}: {e}", path.display()),
                            );
                            first_err.lock().unwrap().get_or_insert(e);
                        }
                    }
                }
            });
        }
    });
    if let Some(e) = first_err.into_inner().unwrap() {
        return Err(e);
    }
    Ok(stats.into_inner().unwrap())
}

fn pack_file(path: &Path) -> std::io::Result<PackStats> {
    let mut st = PackStats::default();
    if !compressible(path) {
        // A picture: no copies (a stale one would be served instead).
        for ext in ["br", "gz"] {
            let _ = std::fs::remove_file(copy_path(path, ext));
        }
        return Ok(st);
    }
    let mut bytes = std::fs::read(path)?;
    let mut br = brotli(&bytes);
    if path.extension().is_some_and(|e| e == "glb") {
        let packed = meshopt_glb(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        // Kept only where it pays on the wire: meshopt's byte-wise deltas
        // of unquantised floats can defeat Brotli's matching (a level's
        // glb is about 60 % bigger as meshopt then Brotli than as Brotli
        // alone), and decoding it is work the browser's own Brotli spares.
        if let Some(packed) = packed {
            let packed_br = brotli(&packed);
            if packed_br.len() * 100 < br.len() * MESHOPT_GAIN {
                st.glbs = 1;
                st.glb_before = bytes.len() as u64;
                st.glb_after = packed.len() as u64;
                std::fs::write(path, &packed)?;
                bytes = packed;
                br = packed_br;
            }
        }
    }
    st.precompressed = 1;
    st.plain = bytes.len() as u64;
    st.brotli = br.len() as u64;
    std::fs::write(copy_path(path, "br"), &br)?;
    std::fs::write(copy_path(path, "gz"), gzip(&bytes)?)?;
    Ok(st)
}

/// `bytes` as Brotli, quality 11 with a 16 MiB window: slow to write (once,
/// at extraction), as fast as any to read.
pub fn brotli(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() / 3);
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 1 << 16, 11, 24);
        w.write_all(bytes).expect("writing to memory");
    }
    out
}

/// `bytes` gzipped at level 9.
pub fn gzip(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(bytes)?;
    z.finish()
}

// ---- coverage ----

/// What the web app asks the art for, by kind, as [`coverage`] finds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Files the manifest (and the files it names) refer to, by kind:
    /// `images` (the stats site's pictures), `maps`, `maps (halo 1)`,
    /// `geometry` (map.glb), `views.json`, `views` (their pictures),
    /// `textures` (the pictures glbs name), `skies`, `models` (bipeds,
    /// weapons, vehicles), `catalog`.
    pub kinds: std::collections::BTreeMap<&'static str, usize>,
    /// Referred-to files that are not in the art directory.
    pub missing: Vec<String>,
}

/// Every file the web app can ask the art in `dir` for, following the
/// manifest as the viewer and the stats site do: the pictures in `images`;
/// per map its `geometry` and `views`, each view's picture (`file`, else
/// `views/<id>.png`), the pictures the glb names (`images[].uri`) and its
/// sky (`asset.extras.sky.glb`, with the sky glb's own pictures); every
/// model's glb and its pictures; `catalog.json`. Paths are relative to
/// `dir`.
pub fn coverage(dir: &Path) -> Result<Coverage, String> {
    use std::collections::BTreeSet;
    let read_json = |rel: &str| -> Result<Value, String> {
        let bytes = std::fs::read(dir.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{rel}: {e}"))
    };
    let manifest = read_json(crate::art::MANIFEST)?;
    let mut cov = Coverage::default();
    let mut want: BTreeSet<String> = BTreeSet::new();
    let mut add = |cov: &mut Coverage, kind: &'static str, rel: String| {
        *cov.kinds.entry(kind).or_default() += 1;
        want.insert(rel);
    };
    let join = |base: &str, rel: &str| match base.rsplit_once('/') {
        Some((d, _)) => format!("{d}/{rel}"),
        None => rel.to_string(),
    };
    // A glb's own picture files and its sky.
    let glb_refs = |rel: &str| -> Vec<(&'static str, String)> {
        let Ok(bytes) = std::fs::read(dir.join(rel)) else {
            return Vec::new();
        };
        let Ok((json, _)) = split_glb(&bytes) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for img in json["images"].as_array().into_iter().flatten() {
            if let Some(uri) = img["uri"].as_str().filter(|u| !u.starts_with("data:")) {
                out.push(("textures", join(rel, uri)));
            }
        }
        if let Some(sky) = json["asset"]["extras"]["sky"]["glb"].as_str() {
            out.push(("skies", join(rel, sky)));
        }
        out
    };
    for img in manifest["images"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(_, v)| v)
    {
        if let Some(f) = img["file"].as_str() {
            add(&mut cov, "images", f.to_string());
        }
    }
    for (_, m) in manifest["maps"].as_object().into_iter().flatten() {
        *cov.kinds.entry("maps").or_default() += 1;
        if m["game"] == "halo1" {
            *cov.kinds.entry("maps (halo 1)").or_default() += 1;
        }
        if let Some(glb) = m["geometry"].as_str() {
            add(&mut cov, "geometry", glb.to_string());
            let mut todo = glb_refs(glb);
            while let Some((kind, rel)) = todo.pop() {
                if kind == "skies" {
                    todo.extend(glb_refs(&rel).into_iter().filter(|(k, _)| *k == "textures"));
                }
                add(&mut cov, kind, rel);
            }
        }
        if let Some(views) = m["views"].as_str() {
            add(&mut cov, "views.json", views.to_string());
            if let Ok(v) = read_json(views) {
                for view in v["views"].as_array().into_iter().flatten() {
                    let file = match view["file"].as_str() {
                        Some(f) => f.to_string(),
                        None => format!("views/{}.png", view["id"].as_str().unwrap_or("")),
                    };
                    add(&mut cov, "views", join(views, &file));
                }
            }
        }
    }
    for group in ["bipeds", "weapons", "vehicles"] {
        for (_, m) in manifest["models"][group].as_object().into_iter().flatten() {
            if let Some(glb) = m["glb"].as_str() {
                add(&mut cov, "models", glb.to_string());
                for (kind, rel) in glb_refs(glb) {
                    add(&mut cov, kind, rel);
                }
            }
        }
    }
    if dir.join("catalog.json").is_file() {
        add(&mut cov, "catalog", "catalog.json".into());
    }
    cov.missing = want
        .into_iter()
        .filter(|rel| !dir.join(rel).is_file())
        .collect();
    Ok(cov)
}

// ---- pictures ----

/// A picture (RGBA, straight alpha) as lossy WebP at `quality` (0 to 100),
/// at libwebp's method 5 (of 0 to 6: within a few per cent of the
/// smallest, at half the time). A picture with any transparency
/// keeps its alpha exactly (a lossless alpha plane) and the colour under
/// clear pixels as it was, so a cut-out cuts where it did and its edges
/// filter to the same colours; an opaque one has no alpha plane. Every
/// browser the web app supports decodes it (`createImageBitmap`).
pub fn webp(rgba: &[u8], width: u32, height: u32, quality: f32) -> Result<Vec<u8>, String> {
    if rgba.len() != width as usize * height as usize * 4 || width == 0 || height == 0 {
        return Err(format!(
            "{width}x{height} picture with {} bytes",
            rgba.len()
        ));
    }
    let opaque = rgba.chunks_exact(4).all(|p| p[3] == 255);
    let mut config = webp::WebPConfig::new().map_err(|()| "libwebp config")?;
    config.quality = quality;
    config.method = 5;
    config.alpha_quality = 100;
    config.alpha_compression = 1;
    config.exact = i32::from(!opaque);
    let rgb: Vec<u8>;
    let encoder = if opaque {
        rgb = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        webp::Encoder::from_rgb(&rgb, width, height)
    } else {
        webp::Encoder::from_rgba(rgba, width, height)
    };
    encoder
        .encode_advanced(&config)
        .map(|m| m.to_vec())
        .map_err(|e| format!("WebP: {e:?}"))
}

/// A WebP picture's size and RGBA pixels (for tests and checks).
pub fn decode_webp(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = webp::Decoder::new(bytes).decode()?;
    let (w, h) = (img.width(), img.height());
    let px: &[u8] = &img;
    let rgba = if img.is_alpha() {
        px.to_vec()
    } else {
        px.chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect()
    };
    Some((w, h, rgba))
}

// ---- glb ----

const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

/// A glb's JSON and binary chunk.
pub fn split_glb(glb: &[u8]) -> Result<(Value, &[u8]), String> {
    let u32_at = |at: usize| -> Result<u32, String> {
        glb.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or_else(|| "a glb cut short".to_string())
    };
    if u32_at(0)? != GLB_MAGIC || u32_at(4)? != 2 {
        return Err("not a glTF 2 binary".into());
    }
    let json_len = u32_at(12)? as usize;
    if u32_at(16)? != CHUNK_JSON {
        return Err("a glb without its JSON chunk first".into());
    }
    let json_bytes = glb.get(20..20 + json_len).ok_or("a glb cut short")?;
    let json: Value = serde_json::from_slice(json_bytes).map_err(|e| format!("glb JSON: {e}"))?;
    let at = 20 + json_len;
    let bin: &[u8] = if glb.len() >= at + 8 && u32_at(at + 4)? == CHUNK_BIN {
        let len = u32_at(at)? as usize;
        glb.get(at + 8..at + 8 + len).ok_or("a glb cut short")?
    } else {
        &[]
    };
    Ok((json, bin))
}

/// A glb from its JSON and binary chunk.
pub fn join_glb(json: &Value, bin: &[u8]) -> Vec<u8> {
    let mut j = serde_json::to_vec(json).expect("JSON");
    while j.len() % 4 != 0 {
        j.push(b' ');
    }
    let pad = (4 - bin.len() % 4) % 4;
    let bin_chunk = if bin.is_empty() {
        0
    } else {
        8 + bin.len() + pad
    };
    let total = 12 + 8 + j.len() + bin_chunk;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(j.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&j);
    if !bin.is_empty() {
        out.extend_from_slice(&((bin.len() + pad) as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(bin);
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    out
}

/// How a buffer view is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Attributes,
    Triangles,
    Indices,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Attributes => "ATTRIBUTES",
            Mode::Triangles => "TRIANGLES",
            Mode::Indices => "INDICES",
        }
    }
}

fn component_size(ty: u64) -> Option<usize> {
    match ty {
        5120 | 5121 => Some(1),
        5122 | 5123 => Some(2),
        5125 | 5126 => Some(4),
        _ => None,
    }
}

fn components(ty: &str) -> Option<usize> {
    Some(match ty {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        "MAT2" => 4,
        "MAT3" => 9,
        "MAT4" => 16,
        _ => return None,
    })
}

/// What each buffer view holds, from the accessors that read it: `None`
/// for a view to leave as it is (pictures, mixed or odd layouts).
fn plan(json: &Value) -> Vec<Option<(Mode, usize)>> {
    let views = json["bufferViews"].as_array().map_or(0, Vec::len);
    let accessors = json["accessors"].as_array().cloned().unwrap_or_default();
    // Accessors used as indices, and whether all their primitives are triangle lists.
    let mut index_of = vec![None::<bool>; accessors.len()];
    for mesh in json["meshes"].as_array().into_iter().flatten() {
        for prim in mesh["primitives"].as_array().into_iter().flatten() {
            if let Some(i) = prim["indices"].as_u64() {
                let tri = prim["mode"].as_u64().unwrap_or(4) == 4;
                if let Some(slot) = index_of.get_mut(i as usize) {
                    *slot = Some(slot.unwrap_or(true) && tri);
                }
            }
        }
    }
    // Per view: (element sizes, any index use, all triangles, any sparse/other use).
    #[derive(Default, Clone)]
    struct Use {
        sizes: Vec<usize>,
        index: bool,
        vertex: bool,
        triangles: bool,
        bad: bool,
    }
    let mut uses = vec![
        Use {
            triangles: true,
            ..Default::default()
        };
        views
    ];
    for (a, acc) in accessors.iter().enumerate() {
        if acc.get("sparse").is_some() {
            // Sparse data names views of its own; leave them all.
            for k in ["indices", "values"] {
                if let Some(v) = acc["sparse"][k]["bufferView"].as_u64() {
                    if let Some(u) = uses.get_mut(v as usize) {
                        u.bad = true;
                    }
                }
            }
        }
        let Some(v) = acc["bufferView"].as_u64() else {
            continue;
        };
        let Some(u) = uses.get_mut(v as usize) else {
            continue;
        };
        let size = acc["componentType"]
            .as_u64()
            .and_then(component_size)
            .zip(acc["type"].as_str().and_then(components))
            .map(|(c, n)| (c, c * n));
        let Some((comp, elem)) = size else {
            u.bad = true;
            continue;
        };
        match index_of[a] {
            Some(tri) => {
                u.index = true;
                u.triangles &= tri;
                u.sizes.push(comp);
            }
            None => {
                u.vertex = true;
                u.sizes.push(elem);
            }
        }
    }
    for img in json["images"].as_array().into_iter().flatten() {
        if let Some(v) = img["bufferView"].as_u64() {
            if let Some(u) = uses.get_mut(v as usize) {
                u.bad = true;
            }
        }
    }
    let bvs = json["bufferViews"].as_array().cloned().unwrap_or_default();
    bvs.iter()
        .zip(uses)
        .map(|(bv, u)| {
            if u.bad || u.sizes.is_empty() || (u.index && u.vertex) {
                return None;
            }
            if bv["buffer"].as_u64() != Some(0) {
                return None;
            }
            let len = bv["byteLength"].as_u64()? as usize;
            let stride = match bv["byteStride"].as_u64() {
                Some(s) => s as usize,
                None if u.sizes.iter().all(|&s| s == u.sizes[0]) => u.sizes[0],
                None => return None,
            };
            if stride == 0 || len % stride != 0 || len == 0 {
                return None;
            }
            if u.index {
                if bv.get("byteStride").is_some() || !(stride == 2 || stride == 4) {
                    return None;
                }
                let count = len / stride;
                let mode = if u.triangles && count % 3 == 0 {
                    Mode::Triangles
                } else {
                    Mode::Indices
                };
                Some((mode, stride))
            } else if stride % 4 == 0 && stride <= 256 {
                Some((Mode::Attributes, stride))
            } else {
                None
            }
        })
        .collect()
}

fn encode(mode: Mode, stride: usize, data: &[u8]) -> Option<Vec<u8>> {
    let count = data.len() / stride;
    match mode {
        Mode::Attributes => {
            let bound = unsafe { meshopt::ffi::meshopt_encodeVertexBufferBound(count, stride) };
            let mut out = vec![0u8; bound];
            // Version 1 (decoders from meshoptimizer 0.23 on; three's is 1.1)
            // at its default level.
            let n = unsafe {
                meshopt::ffi::meshopt_encodeVertexBufferLevel(
                    out.as_mut_ptr(),
                    out.len(),
                    data.as_ptr().cast(),
                    count,
                    stride,
                    2,
                    1,
                )
            };
            (n > 0).then(|| {
                out.truncate(n);
                out
            })
        }
        Mode::Triangles | Mode::Indices => {
            let indices: Vec<u32> = if stride == 2 {
                data.chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]) as u32)
                    .collect()
            } else {
                data.chunks_exact(4)
                    .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                    .collect()
            };
            let vertices = indices.iter().copied().max().map_or(0, |m| m as usize + 1);
            let n = if mode == Mode::Triangles {
                let bound = unsafe {
                    meshopt::ffi::meshopt_encodeIndexBufferBound(indices.len(), vertices)
                };
                let mut out = vec![0u8; bound];
                let n = unsafe {
                    meshopt::ffi::meshopt_encodeIndexBuffer(
                        out.as_mut_ptr(),
                        out.len(),
                        indices.as_ptr(),
                        indices.len(),
                    )
                };
                out.truncate(n);
                out
            } else {
                let bound = unsafe {
                    meshopt::ffi::meshopt_encodeIndexSequenceBound(indices.len(), vertices)
                };
                let mut out = vec![0u8; bound];
                let n = unsafe {
                    meshopt::ffi::meshopt_encodeIndexSequence(
                        out.as_mut_ptr(),
                        out.len(),
                        indices.as_ptr(),
                        indices.len(),
                    )
                };
                out.truncate(n);
                out
            };
            (!n.is_empty()).then_some(n)
        }
    }
}

/// Decodes one view `encode` wrote: for the tests and for checking a
/// packed glb against its source.
pub fn decode_view(mode: &str, count: usize, stride: usize, data: &[u8]) -> Option<Vec<u8>> {
    let mut out = vec![0u8; count * stride];
    let rc = unsafe {
        match mode {
            "ATTRIBUTES" => meshopt::ffi::meshopt_decodeVertexBuffer(
                out.as_mut_ptr().cast(),
                count,
                stride,
                data.as_ptr(),
                data.len(),
            ),
            "TRIANGLES" => meshopt::ffi::meshopt_decodeIndexBuffer(
                out.as_mut_ptr().cast(),
                count,
                stride,
                data.as_ptr(),
                data.len(),
            ),
            "INDICES" => meshopt::ffi::meshopt_decodeIndexSequence(
                out.as_mut_ptr().cast(),
                count,
                stride,
                data.as_ptr(),
                data.len(),
            ),
            _ => return None,
        }
    };
    (rc == 0).then_some(out)
}

/// `glb` with its vertex and index data compressed (`EXT_meshopt_compression`,
/// required), or `None` when there is nothing to compress or it is already.
pub fn meshopt_glb(glb: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let (mut json, bin) = split_glb(glb)?;
    let used = |j: &Value| {
        j["extensionsUsed"]
            .as_array()
            .is_some_and(|a| a.iter().any(|e| e == MESHOPT))
    };
    if used(&json) || bin.is_empty() {
        return Ok(None);
    }
    let buffers = json["buffers"].as_array().map_or(0, Vec::len);
    if buffers != 1 || json["buffers"][0].get("uri").is_some() {
        return Ok(None);
    }
    let plan = plan(&json);
    let align = |v: &mut Vec<u8>| v.resize(v.len().div_ceil(4) * 4, 0);
    let mut new_bin = Vec::with_capacity(bin.len() / 2);
    let mut fallback = 0usize;
    let mut any = false;
    let views = json["bufferViews"]
        .as_array_mut()
        .ok_or("no buffer views")?;
    for (bv, plan) in views.iter_mut().zip(plan) {
        let off = bv["byteOffset"].as_u64().unwrap_or(0) as usize;
        let len = bv["byteLength"]
            .as_u64()
            .ok_or("a buffer view without a length")? as usize;
        let raw = bin
            .get(off..off + len)
            .ok_or("a buffer view past the binary chunk")?;
        let packed = plan.and_then(|(mode, stride)| {
            encode(mode, stride, raw)
                .filter(|e| e.len() < raw.len())
                .map(|e| (mode, stride, e))
        });
        align(&mut new_bin);
        match packed {
            Some((mode, stride, enc)) => {
                any = true;
                let at = new_bin.len();
                new_bin.extend_from_slice(&enc);
                fallback = fallback.div_ceil(4) * 4;
                bv["buffer"] = json!(1);
                bv["byteOffset"] = json!(fallback);
                bv["extensions"][MESHOPT] = json!({
                    "buffer": 0, "byteOffset": at, "byteLength": enc.len(),
                    "byteStride": stride, "count": len / stride, "mode": mode.name(),
                });
                fallback += len;
            }
            None => {
                bv["byteOffset"] = json!(new_bin.len());
                new_bin.extend_from_slice(raw);
            }
        }
    }
    if !any {
        return Ok(None);
    }
    align(&mut new_bin);
    json["buffers"] = json!([
        { "byteLength": new_bin.len() },
        { "byteLength": fallback.div_ceil(4) * 4, "extensions": { MESHOPT: { "fallback": true } } },
    ]);
    for key in ["extensionsUsed", "extensionsRequired"] {
        let list = json[key].as_array().cloned().unwrap_or_default();
        let mut list: Vec<Value> = list.into_iter().filter(|e| e != MESHOPT).collect();
        list.push(json!(MESHOPT));
        json[key] = Value::Array(list);
    }
    Ok(Some(join_glb(&json, &new_bin)))
}

/// A packed glb's binary data as it was before [`meshopt_glb`]: each
/// buffer view's bytes, decoded where compressed. For tests and checks.
pub fn unpacked_views(glb: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let (json, bin) = split_glb(glb)?;
    let mut out = Vec::new();
    for bv in json["bufferViews"].as_array().into_iter().flatten() {
        let ext = &bv["extensions"][MESHOPT];
        if ext.is_object() {
            let at = ext["byteOffset"].as_u64().unwrap_or(0) as usize;
            let len = ext["byteLength"].as_u64().ok_or("no length")? as usize;
            let data = bin.get(at..at + len).ok_or("past the end")?;
            let count = ext["count"].as_u64().ok_or("no count")? as usize;
            let stride = ext["byteStride"].as_u64().ok_or("no stride")? as usize;
            let mode = ext["mode"].as_str().ok_or("no mode")?;
            out.push(decode_view(mode, count, stride, data).ok_or("does not decode")?);
        } else {
            let at = bv["byteOffset"].as_u64().unwrap_or(0) as usize;
            let len = bv["byteLength"].as_u64().ok_or("no length")? as usize;
            out.push(bin.get(at..at + len).ok_or("past the end")?.to_vec());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invented level: a triangle list over a grid of floats with
    /// normals and texture coordinates, an embedded picture and a
    /// skin-like matrix view.
    fn invented_glb() -> Vec<u8> {
        let n = 40usize;
        let mut pos = Vec::new();
        let mut nrm = Vec::new();
        let mut uv = Vec::new();
        for y in 0..n {
            for x in 0..n {
                for v in [x as f32 * 1.5, (x * y) as f32 * 0.01, y as f32 * -1.5] {
                    pos.extend_from_slice(&v.to_le_bytes());
                }
                for v in [0.0f32, 1.0, 0.0] {
                    nrm.extend_from_slice(&v.to_le_bytes());
                }
                for v in [x as f32 / 4.0, y as f32 / 4.0] {
                    uv.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        let mut idx = Vec::new();
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let a = (y * n + x) as u16;
                for i in [
                    a,
                    a + 1,
                    a + n as u16,
                    a + 1,
                    a + n as u16 + 1,
                    a + n as u16,
                ] {
                    idx.extend_from_slice(&i.to_le_bytes());
                }
            }
        }
        let picture = b"\x89PNG not really".to_vec();
        let mut bin = Vec::new();
        let mut views = Vec::new();
        for (data, target) in [
            (&pos, Some(34962)),
            (&nrm, Some(34962)),
            (&uv, Some(34962)),
            (&idx, Some(34963)),
            (&picture, None),
        ] {
            while bin.len() % 4 != 0 {
                bin.push(0);
            }
            let mut v = json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": data.len()});
            if let Some(t) = target {
                v["target"] = json!(t);
            }
            views.push(v);
            bin.extend_from_slice(data);
        }
        let verts = n * n;
        let json = json!({
            "asset": {"version": "2.0", "extras": {"keep": "me"}},
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": views,
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": verts, "type": "VEC3", "min": [0,0,0], "max": [1,1,1]},
                {"bufferView": 1, "componentType": 5126, "count": verts, "type": "VEC3"},
                {"bufferView": 2, "componentType": 5126, "count": verts, "type": "VEC2"},
                {"bufferView": 3, "componentType": 5123, "count": idx.len() / 2, "type": "SCALAR"},
            ],
            "images": [{"bufferView": 4, "mimeType": "image/png"}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2}, "indices": 3}]}],
            "nodes": [{"mesh": 0, "name": "structure"}],
            "scenes": [{"nodes": [0]}],
            "extensionsUsed": ["KHR_texture_transform"],
        });
        join_glb(&json, &bin)
    }

    #[test]
    fn a_glb_round_trips_through_meshopt_bit_for_bit() {
        let glb = invented_glb();
        let before = unpacked_views(&glb).unwrap();
        let packed = meshopt_glb(&glb).unwrap().expect("something to compress");
        assert!(
            packed.len() < glb.len(),
            "{} >= {}",
            packed.len(),
            glb.len()
        );
        let (json, _) = split_glb(&packed).unwrap();
        assert_eq!(json["extensionsRequired"], json!([MESHOPT]));
        assert_eq!(
            json["extensionsUsed"],
            json!(["KHR_texture_transform", MESHOPT])
        );
        assert_eq!(json["asset"]["extras"]["keep"], "me", "the JSON is kept");
        let modes: Vec<_> = json["bufferViews"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                v["extensions"][MESHOPT]["mode"]
                    .as_str()
                    .unwrap_or("raw")
                    .to_string()
            })
            .collect();
        assert_eq!(
            modes,
            ["ATTRIBUTES", "ATTRIBUTES", "ATTRIBUTES", "TRIANGLES", "raw"]
        );
        // The fallback buffer: the views' uncompressed layout, no data.
        assert_eq!(json["buffers"][1]["extensions"][MESHOPT]["fallback"], true);
        assert!(json["buffers"][1].get("uri").is_none());
        for v in json["bufferViews"].as_array().unwrap() {
            assert_eq!(v["byteOffset"].as_u64().unwrap() % 4, 0, "aligned: {v}");
        }
        let after = unpacked_views(&packed).unwrap();
        for i in [0, 1, 2, 4] {
            assert_eq!(after[i], before[i], "view {i} is bit for bit");
        }
        // The index codec may rotate a triangle's corners (same winding,
        // same triangle, same order of triangles).
        let tris = |b: &[u8]| -> Vec<[u16; 3]> {
            b.chunks_exact(6)
                .map(|t| {
                    let v = [0, 2, 4].map(|o| u16::from_le_bytes([t[o], t[o + 1]]));
                    let r = (0..3).min_by_key(|&k| v[k]).unwrap();
                    [v[r], v[(r + 1) % 3], v[(r + 2) % 3]]
                })
                .collect()
        };
        assert_eq!(tris(&after[3]), tris(&before[3]), "the same triangles");
        // Packing again changes nothing.
        assert_eq!(meshopt_glb(&packed).unwrap(), None);
    }

    /// Everything the viewer and the stats site ask for, Halo 2's and
    /// Halo 1's maps, models, textures, views and skies, is found through
    /// the manifest, and a file it names but the directory lacks is caught.
    #[test]
    fn coverage_follows_the_manifest_to_every_file() {
        let dir = std::env::temp_dir().join(format!("h2-art-cover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let w = |rel: &str, bytes: &[u8]| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        let glb_naming = |uris: &[&str], sky: Option<&str>| {
            let mut j = json!({"asset": {"version": "2.0"},
                "images": uris.iter().map(|u| json!({"uri": u, "mimeType": "image/webp"})).collect::<Vec<_>>()});
            if let Some(s) = sky {
                j["asset"]["extras"] = json!({"sky": {"glb": s}});
            }
            join_glb(&j, &[])
        };
        w("manifest.json", serde_json::to_string(&json!({
            "version": 1,
            "images": {"rank/1": {"file": "rank/1.png"}, "map/h1_gulch/preview": {"file": "map/h1_gulch/preview.png"},
                       "emblem/fg/0": {"file": "emblem/fg/0.png"}},
            "maps": {
                "zanzibar": {"geometry": "map/zanzibar/map.glb", "views": "map/zanzibar/views.json"},
                "h1_gulch": {"geometry": "map/h1_gulch/map.glb", "views": "map/h1_gulch/views.json", "game": "halo1", "name": "Gulch"},
            },
            "models": {"version": 2,
                "bipeds": {"spartan": {"glb": "models/biped/spartan.glb"}},
                "weapons": {"smg": {"glb": "models/weapon/smg.glb"}},
                "vehicles": {"warthog": {"glb": "models/vehicle/warthog.glb"}}},
        })).unwrap().as_bytes());
        for f in [
            "rank/1.png",
            "map/h1_gulch/preview.png",
            "emblem/fg/0.png",
            "catalog.json",
        ] {
            w(f, b"x");
        }
        w(
            "map/zanzibar/map.glb",
            &glb_naming(
                &["tex/0000002a.webp", "tex/0000002bm.webp"],
                Some("sky.glb"),
            ),
        );
        w(
            "map/zanzibar/sky.glb",
            &glb_naming(&["tex/00000099.webp"], None),
        );
        w(
            "map/h1_gulch/map.glb",
            &glb_naming(&["tex/00000001.webp"], None),
        );
        for f in [
            "map/zanzibar/tex/0000002a.webp",
            "map/zanzibar/tex/0000002bm.webp",
            "map/zanzibar/tex/00000099.webp",
            "map/h1_gulch/tex/00000001.webp",
            "map/zanzibar/views/top.webp",
            "map/h1_gulch/views/top.png",
        ] {
            w(f, b"x");
        }
        w(
            "map/zanzibar/views.json",
            br#"{"views": [{"id": "top", "file": "views/top.webp"}]}"#,
        );
        w("map/h1_gulch/views.json", br#"{"views": [{"id": "top"}]}"#);
        for f in [
            "models/biped/spartan.glb",
            "models/weapon/smg.glb",
            "models/vehicle/warthog.glb",
        ] {
            w(f, &glb_naming(&[], None));
        }
        let cov = coverage(&dir).unwrap();
        assert_eq!(cov.missing, Vec::<String>::new());
        let k = |n: &str| cov.kinds.get(n).copied().unwrap_or(0);
        assert_eq!(
            (
                k("maps"),
                k("maps (halo 1)"),
                k("geometry"),
                k("views.json"),
                k("views")
            ),
            (2, 1, 2, 2, 2)
        );
        assert_eq!(
            (
                k("textures"),
                k("skies"),
                k("models"),
                k("images"),
                k("catalog")
            ),
            (4, 1, 3, 3, 1)
        );
        // A file named but not there is reported.
        std::fs::remove_file(dir.join("map/h1_gulch/tex/00000001.webp")).unwrap();
        std::fs::remove_file(dir.join("models/vehicle/warthog.glb")).unwrap();
        assert_eq!(
            coverage(&dir).unwrap().missing,
            [
                "map/h1_gulch/tex/00000001.webp",
                "models/vehicle/warthog.glb"
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pictures_go_to_webp_keeping_their_cut() {
        let (w, h) = (64u32, 32u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let a = if (x / 8 + y / 8) % 2 == 0 { 255 } else { 0 };
                rgba.extend([(x * 4) as u8, (y * 8) as u8, 90, a]);
            }
        }
        let bytes = webp(&rgba, w, h, 90.0).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
        let (dw, dh, back) = decode_webp(&bytes).unwrap();
        assert_eq!((dw, dh), (w, h));
        // Alpha exactly as it was; colour close.
        for (a, b) in rgba.chunks_exact(4).zip(back.chunks_exact(4)) {
            assert_eq!(a[3], b[3]);
            if a[3] == 255 {
                for c in 0..3 {
                    assert!(a[c].abs_diff(b[c]) <= 24, "{a:?} {b:?}");
                }
            }
        }
        // Opaque: no alpha plane, every pixel opaque on decode.
        let opaque: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
        let (_, _, back) = decode_webp(&webp(&opaque, w, h, 90.0).unwrap()).unwrap();
        assert!(back.chunks_exact(4).all(|p| p[3] == 255));
        assert!(webp(&rgba[4..], w, h, 90.0).is_err());
    }

    #[test]
    fn odd_layouts_stay_as_they_are() {
        // A view read with two element sizes, and u8 indices: neither fits a codec.
        let bin = vec![0u8; 48];
        let json = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 48}],
            "bufferViews": [{"buffer": 0, "byteLength": 24}, {"buffer": 0, "byteOffset": 24, "byteLength": 24}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 2, "type": "VEC3"},
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC2"},
                {"bufferView": 1, "componentType": 5121, "count": 24, "type": "SCALAR"},
            ],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 2}]}],
        });
        assert_eq!(meshopt_glb(&join_glb(&json, &bin)).unwrap(), None);
    }

    #[test]
    fn packing_a_directory_writes_compressed_copies() {
        let dir = std::env::temp_dir().join(format!("h2-art-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("map/x/tex")).unwrap();
        let manifest = format!("{{\"pad\": \"{}\"}}", "ab".repeat(500));
        std::fs::write(dir.join("manifest.json"), &manifest).unwrap();
        std::fs::write(dir.join("map/x/map.glb"), invented_glb()).unwrap();
        std::fs::write(dir.join("map/x/tex/0000002a.jpg"), b"jpeg").unwrap();
        // A stale copy beside a picture goes.
        std::fs::write(dir.join("map/x/tex/0000002a.jpg.gz"), b"old").unwrap();
        let st = pack(&dir).unwrap();
        assert_eq!(st.precompressed, 2);
        assert!(st.brotli < st.plain);
        assert!(!dir.join("map/x/tex/0000002a.jpg.gz").exists());
        let glb = std::fs::read(dir.join("map/x/map.glb")).unwrap();
        for (ext, decoded) in [
            ("br", {
                let mut out = Vec::new();
                brotli::BrotliDecompress(
                    &mut std::io::Cursor::new(std::fs::read(dir.join("map/x/map.glb.br")).unwrap()),
                    &mut out,
                )
                .unwrap();
                out
            }),
            ("gz", {
                use std::io::Read;
                let mut out = Vec::new();
                flate2::read::GzDecoder::new(
                    &std::fs::read(dir.join("map/x/map.glb.gz")).unwrap()[..],
                )
                .read_to_end(&mut out)
                .unwrap();
                out
            }),
        ] {
            assert_eq!(decoded, glb, "{ext} copy is of the packed glb");
        }
        assert!(dir.join("manifest.json.br").exists());
        // Meshopt only where it makes the Brotli copy smaller.
        let plain_br = brotli(&invented_glb()).len();
        let packed_br = brotli(&meshopt_glb(&invented_glb()).unwrap().unwrap()).len();
        let chosen = packed_br * 100 < plain_br * MESHOPT_GAIN;
        assert_eq!(st.glbs == 1, chosen);
        assert_eq!(
            std::fs::metadata(dir.join("map/x/map.glb.br"))
                .unwrap()
                .len() as usize,
            if chosen { packed_br } else { plain_br }
        );
        // Again: the copies are the same.
        let again = pack(&dir).unwrap();
        assert_eq!(again.glbs, 0);
        assert_eq!(std::fs::read(dir.join("map/x/map.glb")).unwrap(), glb);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
