//! Halo 1 maps, read-only, through `h1map`: a Halo 1 disc image or a
//! folder of Halo 1 maps as a source, and what the editor shows of one map
//! (its structure BSPs' triangles and its multiplayer placements).
//!
//! Nothing here writes. Converting a map to Halo 2 is `h1convert`'s job
//! (docs/game/h1-multiplayer.md).
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use h1map::scenario::{FlagKind, GameType, Multiplayer};
use h1map::{H1Map, TagId, TagRef};
use xiso::{Entry, Xiso};

/// Where Halo 1 maps come from.
#[derive(Debug, Clone)]
pub enum H1Source {
    /// A Halo 1 disc image (its XBE is Halo: Combat Evolved's).
    Disc { path: PathBuf, maps: Vec<Entry> },
    /// Every Halo 1 `*.map` directly in a folder.
    Folder { dir: PathBuf, maps: Vec<String> },
}

/// Whether `bytes` start like a Halo 1 Xbox map (cache version 5).
pub fn is_h1_map_header(bytes: &[u8]) -> bool {
    bytes.len() >= h1map::HEADER_LEN && h1map::parse_header(&bytes[..h1map::HEADER_LEN]).is_ok()
}

fn is_h1_map_file(path: &Path) -> bool {
    let mut head = vec![0; h1map::HEADER_LEN];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && is_h1_map_header(&head)
}

impl H1Source {
    /// Opens `path` as a Halo 1 source: a folder holding Halo 1 maps, a
    /// single Halo 1 `.map` (its folder), or a Halo 1 disc image. Fails for
    /// anything else, so a caller can try a Halo 2 source next.
    pub fn open(path: &Path) -> Result<Self> {
        if path.is_dir() {
            let maps = folder_maps(path);
            if maps.is_empty() {
                bail!("no Halo 1 maps in {}", path.display());
            }
            return Ok(Self::Folder {
                dir: path.to_owned(),
                maps,
            });
        }
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("map"))
        {
            if !is_h1_map_file(path) {
                bail!("{} is not a Halo 1 map", path.display());
            }
            let dir = path.parent().unwrap_or(Path::new(".")).to_owned();
            let maps = folder_maps(&dir);
            return Ok(Self::Folder { dir, maps });
        }
        let mut iso = Xiso::open(BufReader::new(File::open(path)?))
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let disc = h1map::disc::Disc::open(&mut iso)?;
        Ok(Self::Disc {
            path: path.to_owned(),
            maps: disc.maps,
        })
    }

    pub fn path(&self) -> &Path {
        match self {
            Self::Disc { path, .. } => path,
            Self::Folder { dir, .. } => dir,
        }
    }

    /// The maps by name (no `.map`), sorted.
    pub fn map_names(&self) -> Vec<String> {
        match self {
            Self::Disc { maps, .. } => maps
                .iter()
                .map(|e| h1map::disc::map_stem(e).to_owned())
                .collect(),
            Self::Folder { maps, .. } => maps.clone(),
        }
    }

    /// The map's file as stored (compressed).
    pub fn read_raw(&self, name: &str) -> Result<Vec<u8>> {
        match self {
            Self::Disc { path, maps } => {
                let e = maps
                    .iter()
                    .find(|e| h1map::disc::map_stem(e).eq_ignore_ascii_case(name))
                    .with_context(|| format!("no map {name} on the disc"))?;
                let mut iso = Xiso::open(BufReader::new(File::open(path)?))
                    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
                iso.read(e).map_err(|e| anyhow::anyhow!("{name}: {e}"))
            }
            Self::Folder { dir, .. } => {
                let p = dir.join(format!("{name}.map"));
                std::fs::read(&p).with_context(|| format!("read {}", p.display()))
            }
        }
    }

    /// The map, decompressed.
    pub fn open_map(&self, name: &str) -> Result<H1Map> {
        Ok(H1Map::from_reader(&self.read_raw(name)?[..])?)
    }
}

/// The Halo 1 maps in a folder, by name.
pub fn folder_maps(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("map")))
        .filter(|p| is_h1_map_file(p))
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_owned))
        .collect();
    out.sort_by_key(|n| n.to_ascii_lowercase());
    out
}

/// Where to look for Halo 1 maps without being asked: `$H1_ISO`, then
/// `$H1_MAP_DIR`, then the repository's `disc/halo1.iso` when it exists.
pub fn default_source() -> Option<PathBuf> {
    ["H1_ISO", "H1_MAP_DIR"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .find(|p| p.exists())
        .or_else(|| {
            Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo1.iso"))
                .filter(|p| p.is_file())
        })
}

/// Find a converted map's Halo 1 source: map `name` in the first of
/// `sources` that has it, checked against `sha256` (hex, of the stored
/// file) when given. Returns the map and the source path it came from.
pub fn find_source(
    sources: &[&H1Source],
    name: &str,
    sha256: Option<&str>,
) -> Result<(H1Map, PathBuf)> {
    let mut why = Vec::new();
    for s in sources {
        if !s.map_names().iter().any(|n| n.eq_ignore_ascii_case(name)) {
            continue;
        }
        let raw = s.read_raw(name)?;
        if let Some(want) = sha256 {
            let got = content::Sha256::of(&raw).to_string();
            if !got.eq_ignore_ascii_case(want) {
                why.push(format!(
                    "{} in {} has another SHA-256",
                    name,
                    s.path().display()
                ));
                continue;
            }
        }
        return Ok((H1Map::from_reader(&raw[..])?, s.path().to_owned()));
    }
    if why.is_empty() {
        bail!("no Halo 1 map {name}: open a Halo 1 disc or folder, or set H1_MAP_DIR");
    }
    bail!("{}", why.join("; "))
}

/// A triangle of a Halo 1 structure BSP.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct H1Tri {
    pub p: [[f32; 3]; 3],
    /// Face normal.
    pub n: [f32; 3],
}

/// The kinds of Halo 1 placement the editor shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum H1Kind {
    StartingLocation,
    NetgameFlag,
    NetgameEquipment,
    Vehicle,
}

impl H1Kind {
    pub const ALL: [H1Kind; 4] = [
        H1Kind::StartingLocation,
        H1Kind::NetgameFlag,
        H1Kind::NetgameEquipment,
        H1Kind::Vehicle,
    ];
    pub fn label(self) -> &'static str {
        match self {
            H1Kind::StartingLocation => "Player starting locations",
            H1Kind::NetgameFlag => "Netgame flags",
            H1Kind::NetgameEquipment => "Netgame equipment",
            H1Kind::Vehicle => "Vehicles",
        }
    }
}

/// One Halo 1 placement, with its fields as text for a read-only
/// inspector.
#[derive(Clone, Debug, PartialEq)]
pub struct H1Placement {
    pub kind: H1Kind,
    /// Index in its block.
    pub index: usize,
    pub position: [f32; 3],
    /// Yaw, radians.
    pub facing: f32,
    pub label: String,
    pub fields: Vec<(&'static str, String)>,
}

/// What the editor shows of a Halo 1 map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct H1View {
    pub name: String,
    pub tris: Vec<H1Tri>,
    pub placements: Vec<H1Placement>,
    /// What could not be read (the rest is still shown).
    pub notes: Vec<String>,
}

impl H1View {
    pub fn of(&self, kind: H1Kind) -> impl Iterator<Item = &H1Placement> {
        self.placements.iter().filter(move |p| p.kind == kind)
    }
    pub fn get(&self, kind: H1Kind, index: usize) -> Option<&H1Placement> {
        self.of(kind).find(|p| p.index == index)
    }
}

/// Reads what the editor shows of `map`: every structure BSP's rendered
/// triangles and the multiplayer placements. A part that cannot be read is
/// left out with a note.
pub fn view(name: &str, map: &H1Map) -> H1View {
    let mut v = H1View {
        name: name.to_owned(),
        ..H1View::default()
    };
    match map.structure_bsps() {
        Ok(bsps) => {
            for b in bsps {
                let r = map
                    .structure_bsp(&b)
                    .and_then(|s| Ok((s.render_surfaces()?, s.lightmaps()?)));
                match r {
                    Ok((surfaces, lightmaps)) => {
                        v.tris.extend(bsp_triangles(&surfaces, &lightmaps))
                    }
                    Err(e) => v.notes.push(format!("structure BSP {}: {e}", b.name)),
                }
            }
        }
        Err(e) => v.notes.push(format!("structure BSPs: {e}")),
    }
    match map.multiplayer() {
        Ok(mp) => {
            v.placements = placements(&mp, &|r: &TagRef| tag_label(map, r.id));
        }
        Err(e) => v.notes.push(format!("multiplayer placements: {e}")),
    }
    v
}

fn tag_label(map: &H1Map, id: TagId) -> String {
    match map.tag(id) {
        Ok(t) => t.name.rsplit('\\').next().unwrap_or(&t.name).to_owned(),
        Err(_) => "none".to_owned(),
    }
}

/// The triangles of one BSP: each lightmap material's render surfaces,
/// indexing into that material's vertices.
pub fn bsp_triangles(surfaces: &[[u16; 3]], lightmaps: &[h1map::bsp::Lightmap]) -> Vec<H1Tri> {
    let mut out = Vec::new();
    for m in lightmaps.iter().flat_map(|l| &l.materials) {
        let first = m.first_surface as usize;
        let Some(tris) = surfaces.get(first..first.saturating_add(m.surface_count as usize)) else {
            continue;
        };
        for t in tris {
            let Some(p) = t
                .iter()
                .map(|&i| m.vertices.get(i as usize).map(|v| v.position))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let p = [p[0], p[1], p[2]];
            out.push(H1Tri {
                p,
                n: face_normal(&p),
            });
        }
    }
    out
}

fn face_normal(p: &[[f32; 3]; 3]) -> [f32; 3] {
    let a = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let b = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let c = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    if l > 0.0 {
        [c[0] / l, c[1] / l, c[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn game_types(t: &[GameType; 4]) -> String {
    let named: Vec<&str> = t
        .iter()
        .filter(|g| **g != GameType::NONE)
        .map(|g| g.name())
        .collect();
    if named.is_empty() {
        "none".into()
    } else {
        named.join(", ")
    }
}

fn flag_name(k: FlagKind) -> String {
    match k {
        FlagKind::CtfFlag => "CTF flag".into(),
        FlagKind::CtfVehicle => "CTF vehicle".into(),
        FlagKind::OddballBall => "oddball".into(),
        FlagKind::RaceTrack => "race track".into(),
        FlagKind::RaceVehicle => "race vehicle".into(),
        FlagKind::VegasBank => "vegas bank".into(),
        FlagKind::TeleportFrom => "teleporter entrance".into(),
        FlagKind::TeleportTo => "teleporter exit".into(),
        FlagKind::HillFlag => "hill".into(),
        FlagKind::Other(v) => format!("kind {v}"),
    }
}

fn xyz(p: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", p[0], p[1], p[2])
}

fn deg(r: f32) -> String {
    format!("{:.1}°", r.to_degrees())
}

/// The multiplayer placements, labelled; `tag` names a tag reference.
pub fn placements(mp: &Multiplayer, tag: &dyn Fn(&TagRef) -> String) -> Vec<H1Placement> {
    let mut out = Vec::new();
    for (i, s) in mp.starting_locations.iter().enumerate() {
        out.push(H1Placement {
            kind: H1Kind::StartingLocation,
            index: i,
            position: s.position,
            facing: s.facing,
            label: format!("spawn {i} (team {})", s.team),
            fields: vec![
                ("position", xyz(s.position)),
                ("facing", deg(s.facing)),
                ("team", s.team.to_string()),
                ("bsp", s.bsp.to_string()),
                ("game types", game_types(&s.game_types)),
            ],
        });
    }
    for (i, f) in mp.flags.iter().enumerate() {
        out.push(H1Placement {
            kind: H1Kind::NetgameFlag,
            index: i,
            position: f.position,
            facing: f.facing,
            label: format!("{} {i}", flag_name(f.kind)),
            fields: vec![
                ("position", xyz(f.position)),
                ("facing", deg(f.facing)),
                ("kind", flag_name(f.kind)),
                ("team / channel", f.team.to_string()),
            ],
        });
    }
    for (i, e) in mp.equipment.iter().enumerate() {
        let item = tag(&e.item);
        out.push(H1Placement {
            kind: H1Kind::NetgameEquipment,
            index: i,
            position: e.position,
            facing: e.facing,
            label: format!("{i}: {item}"),
            fields: vec![
                ("position", xyz(e.position)),
                ("facing", deg(e.facing)),
                ("item", item),
                ("respawn time", format!("{} s", e.spawn_time)),
                ("team", e.team.to_string()),
                ("game types", game_types(&e.game_types)),
                ("flags", format!("{:#x}", e.flags)),
            ],
        });
    }
    for (i, v) in mp.vehicles.iter().enumerate() {
        let palette = usize::try_from(v.palette)
            .ok()
            .and_then(|p| mp.vehicle_palette.get(p))
            .map(tag)
            .unwrap_or_else(|| "none".into());
        out.push(H1Placement {
            kind: H1Kind::Vehicle,
            index: i,
            position: v.position,
            facing: v.rotation[0],
            label: format!("{i}: {palette}"),
            fields: vec![
                ("position", xyz(v.position)),
                (
                    "rotation",
                    format!(
                        "{}, {}, {}",
                        deg(v.rotation[0]),
                        deg(v.rotation[1]),
                        deg(v.rotation[2])
                    ),
                ),
                ("vehicle", palette),
            ],
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use h1map::bsp::{Lightmap, Material, RenderVertex};
    use h1map::scenario::{NetgameEquipment, NetgameFlag, StartingLocation};
    use std::io::Cursor;

    fn vertex(p: [f32; 3]) -> RenderVertex {
        RenderVertex {
            position: p,
            frame: [0; 3],
            texcoord: [0.0; 2],
        }
    }

    fn null_ref() -> TagRef {
        TagRef {
            group: h1map::Group::NONE,
            name_address: 0,
            name_len: 0,
            id: TagId::NONE,
        }
    }

    #[test]
    fn each_material_draws_its_own_surfaces_with_its_own_vertices() {
        let quad = Material {
            shader: null_ref(),
            first_surface: 1,
            surface_count: 2,
            vertices: [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]]
                .map(vertex)
                .to_vec(),
            lightmap_vertices: Vec::new(),
        };
        // Surface 0 belongs to no material; 1 and 2 are the quad; 3 points
        // past the material's vertices and is skipped.
        let surfaces = [[9, 9, 9], [0, 1, 2], [0, 2, 3], [0, 1, 7]];
        let lm = Lightmap {
            bitmap: -1,
            materials: vec![quad],
        };
        let tris = bsp_triangles(&surfaces, &[lm]);
        assert_eq!(tris.len(), 2);
        assert_eq!(tris[1].p[2], [0.0, 1.0, 0.0]);
        assert_eq!(tris[0].n, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn placements_are_labelled_and_described() {
        let mp = Multiplayer {
            skies: Vec::new(),
            starting_locations: vec![StartingLocation {
                position: [1.0, 2.0, 3.0],
                facing: std::f32::consts::FRAC_PI_2,
                team: 1,
                bsp: 0,
                game_types: [
                    GameType::ALL,
                    GameType::NONE,
                    GameType::NONE,
                    GameType::NONE,
                ],
            }],
            flags: vec![NetgameFlag {
                position: [0.0; 3],
                facing: 0.0,
                kind: FlagKind::CtfFlag,
                team: 0,
                weapon_group: null_ref(),
            }],
            equipment: vec![NetgameEquipment {
                flags: 0,
                game_types: [GameType::NONE; 4],
                team: 0,
                spawn_time: 30,
                position: [5.0, 5.0, 0.0],
                facing: 0.0,
                item: null_ref(),
            }],
            vehicle_palette: Vec::new(),
            vehicles: Vec::new(),
        };
        let p = placements(&mp, &|_| "rocket launcher".into());
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].label, "spawn 0 (team 1)");
        assert!(p[0].fields.contains(&("facing", "90.0°".into())));
        assert_eq!(p[1].label, "CTF flag 0");
        assert_eq!(p[2].label, "0: rocket launcher");
        assert!(p[2].fields.contains(&("respawn time", "30 s".into())));
    }

    #[test]
    fn a_halo1_disc_lists_its_maps_and_a_halo2_one_is_refused() {
        let dir = std::env::temp_dir().join(format!("h2-editor-h1src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let iso = dir.join("halo1.iso");
        std::fs::write(&iso, h1map::testing::campaign_disc()).unwrap();
        let s = H1Source::open(&iso).unwrap();
        assert!(s.map_names().contains(&"bloodgulch".to_string()));
        // An invented map reads; its made-up scenario has no placements
        // to show, which is a note, not a failure.
        let m = s.open_map("bloodgulch").unwrap();
        let v = view("bloodgulch", &m);
        assert_eq!(v.name, "bloodgulch");
        let other = dir.join("other.iso");
        std::fs::write(
            &other,
            h1map::testing::disc_image(0x4d53_0064, &[("x.map", h1map::testing::tiny_map())]),
        )
        .unwrap();
        assert!(H1Source::open(&other).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every multiplayer map of the owner's Halo 1 disc or maps folder
    /// (`$H1_ISO`, `$H1_MAP_DIR`, or `disc/halo1.iso`) shows a BSP and
    /// placements with nothing unread.
    #[test]
    #[ignore = "needs the owner's Halo 1 disc or maps"]
    fn disc_every_multiplayer_map_shows_its_bsp_and_placements() {
        let Some(path) = default_source() else {
            eprintln!("no Halo 1 source: set H1_ISO or H1_MAP_DIR");
            return;
        };
        let src = H1Source::open(&path).unwrap();
        let skip: Vec<&str> = h1map::disc::CAMPAIGN_LEVELS
            .iter()
            .chain(&h1map::disc::CAMPAIGN_EXTRAS)
            .copied()
            .collect();
        let mut seen = 0;
        for name in src.map_names() {
            if skip.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let v = view(&name, &src.open_map(&name).unwrap());
            assert!(v.notes.is_empty(), "{name}: {:?}", v.notes);
            assert!(!v.tris.is_empty(), "{name}: no triangles");
            assert!(
                v.of(H1Kind::StartingLocation).count() > 0,
                "{name}: no spawns"
            );
            eprintln!(
                "{name}: {} triangles, {} placements",
                v.tris.len(),
                v.placements.len()
            );
            seen += 1;
        }
        assert!(seen > 0, "no multiplayer maps in {}", path.display());
    }

    #[test]
    fn a_folder_lists_only_halo1_maps_and_a_source_is_checked_by_hash() {
        let dir = std::env::temp_dir().join(format!("h2-editor-h1dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tiny = h1map::testing::tiny_map();
        std::fs::write(dir.join("bloodgulch.map"), &tiny).unwrap();
        std::fs::write(dir.join("notes.map"), b"not a map").unwrap();
        let s = H1Source::open(&dir).unwrap();
        assert_eq!(s.map_names(), ["bloodgulch"]);
        let sha = content::Sha256::of(&tiny).to_string();
        assert!(find_source(&[&s], "bloodgulch", Some(&sha)).is_ok());
        let wrong = find_source(&[&s], "bloodgulch", Some("00")).unwrap_err();
        assert!(wrong.to_string().contains("another SHA-256"), "{wrong}");
        assert!(find_source(&[&s], "dangercanyon", None).is_err());
        // The same file, read back, is the same map.
        let m = H1Map::from_reader(Cursor::new(tiny)).unwrap();
        assert_eq!(
            m.tags().len(),
            s.open_map("bloodgulch").unwrap().tags().len()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
