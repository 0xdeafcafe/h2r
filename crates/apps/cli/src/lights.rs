//! `h2 lights`: the `ligh` tags a map defines, where each comes from, and
//! their shape, colour and shadow flag (docs/formats/effects.md, "ligh";
//! docs/graphics/e3-lighting.md, "Lights: which, and how many"). Writes nothing.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use map::{FourCC, Map, TagId};
use tags::effects::{
    self, Ctx, Rec,
    effect::Effect,
    light::{Light, LightKind},
};
use tags::object::{Context as ObjectContext, ObjectHeader, is_object_group};

const USAGE: &str = "usage: h2 lights <map> [--disc ISO]
       h2 lights --all [<map>|all] [--disc ISO]

Lists every ligh tag the map defines: shape, shadow flag, colour,
brightness, radius or cone, and where it comes from (scenario placements,
object attachments such as headlights and flashlights, effect parts such as
muzzle flashes and explosions). --all checks the field layout over every
light (or one map's) and prints the value distributions.";

type M = Map<BufReader<File>>;

const LIGH: FourCC = FourCC(*b"ligh");

/// The scenario's light placements and their palette (`scnr +0xe8` and
/// `+0xf0`; docs/formats/effects.md "ligh (light)").
const PLACEMENTS: usize = 0xe8;
const PALETTE: usize = 0xf0;
const PALETTE_STRIDE: usize = 0x28;
/// A light placement record: the palette index (i16) first. **V**: on every
/// disc scenario the block's elements end exactly where the palette begins.
const PLACEMENT_STRIDE: usize = 0x6c;
/// Bytes of `scnr` read to reach the blocks above.
const SCENARIO_HEAD: usize = PALETTE + 8;

/// Where a light is used from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// Placed in the scenario this many times.
    Placed(usize),
    /// An attachment of an object tag, at a marker.
    Attached {
        object: String,
        marker: Option<String>,
    },
    /// A part of an effect tag.
    Effect(String),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Placed(1) => write!(f, "placed once in the scenario"),
            Source::Placed(n) => write!(f, "placed {n} times in the scenario"),
            Source::Attached { object, marker } => match marker {
                Some(m) => write!(f, "attached to {object} at {m}"),
                None => write!(f, "attached to {object}"),
            },
            Source::Effect(e) => write!(f, "part of effect {e}"),
        }
    }
}

pub(super) fn run(args: Vec<String>) -> Result<()> {
    let mut args = args.into_iter();
    let (mut positional, mut disc, mut all) = (Vec::new(), None::<PathBuf>, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--all" => all = true,
            "--disc" => disc = Some(args.next().context("--disc needs an ISO path")?.into()),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n{USAGE}"),
            _ => positional.push(a),
        }
    }
    match (all, positional.as_slice()) {
        (true, []) => check(&map_names("all", &disc)?, disc),
        (true, [m]) => check(&map_names(m, &disc)?, disc),
        (false, [m]) => list(m, disc),
        _ => bail!("{USAGE}"),
    }
}

fn map_names(map: &str, disc: &Option<PathBuf>) -> Result<Vec<String>> {
    if map != "all" {
        return Ok(vec![map.to_owned()]);
    }
    let disc_path = disc.clone().map(Ok).unwrap_or_else(super::default_disc)?;
    let mut iso = super::open_iso(&disc_path)?;
    Ok(iso
        .entries()?
        .iter()
        .filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map"))
        .map(|e| {
            let file = e.path.rsplit('/').next().unwrap_or(&e.path);
            file.trim_end_matches(".map").to_owned()
        })
        .collect())
}

fn name(map: &M, id: TagId) -> String {
    map.tag(id)
        .ok()
        .and_then(|t| t.name.clone())
        .unwrap_or_else(|| format!("<{id}>"))
}

/// Placement counts per `ligh` tag: the palette's tag references, counted by
/// each placement's palette index (the placement's first i16).
fn placements(map: &mut M) -> Result<BTreeMap<u32, usize>> {
    let Some(scenario) = map.scenario().cloned() else {
        return Ok(BTreeMap::new());
    };
    let address = scenario
        .location
        .context("scenario has no payload")?
        .address;
    let mut ctx = Ctx::new(map);
    let head = ctx.mem.read(address, SCENARIO_HEAD)?;
    let r = Rec::exact(&head, SCENARIO_HEAD)?;
    let palette: Vec<Option<TagId>> = ctx
        .block(r.block(PALETTE), PALETTE_STRIDE)?
        .iter()
        .map(|e| {
            let p = Rec(e).tag_ref(0);
            (p.group == LIGH && !p.is_null()).then_some(p.id)
        })
        .collect();
    let mut out = BTreeMap::new();
    for e in ctx.block(r.block(PLACEMENTS), PLACEMENT_STRIDE)? {
        let index = Rec(&e).i16(0);
        if let Some(Some(id)) = usize::try_from(index).ok().and_then(|i| palette.get(i)) {
            *out.entry(id.0).or_default() += 1;
        }
    }
    Ok(out)
}

/// Whether the placement block ends exactly where the palette begins, the
/// check on [`PLACEMENT_STRIDE`]. `None` when either block is empty.
fn placement_stride_holds(map: &mut M) -> Result<Option<bool>> {
    let Some(scenario) = map.scenario().cloned() else {
        return Ok(None);
    };
    let address = scenario
        .location
        .context("scenario has no payload")?
        .address;
    let head = Ctx::new(map).mem.read(address, SCENARIO_HEAD)?;
    let r = Rec::exact(&head, SCENARIO_HEAD)?;
    let (p, q) = (r.block(PLACEMENTS), r.block(PALETTE));
    if p.count == 0 || q.count == 0 {
        return Ok(None);
    }
    let end = u64::from(p.address) + u64::from(p.count) * PLACEMENT_STRIDE as u64;
    Ok(Some(end == u64::from(q.address)))
}

/// Every source of every `ligh` tag in the map.
fn sources(map: &mut M) -> Result<BTreeMap<u32, Vec<Source>>> {
    let mut out: BTreeMap<u32, Vec<Source>> = BTreeMap::new();
    for (id, n) in placements(map)? {
        out.entry(id).or_default().push(Source::Placed(n));
    }
    let objects: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| is_object_group(t.group) && t.location.is_some())
        .map(|t| t.id)
        .collect();
    if !objects.is_empty() {
        let ctx = ObjectContext::read(map)?;
        for id in objects {
            let Ok(tag) = ctx.decode(id) else { continue };
            let Ok(h) = ObjectHeader::from_record(&tag.layers[0]) else {
                continue;
            };
            for a in h.attachments {
                if a.kind.group == LIGH && a.kind.id != TagId::NULL {
                    out.entry(a.kind.id.0).or_default().push(Source::Attached {
                        object: format!("{} {}", tag.group, name(map, id)),
                        marker: a.marker,
                    });
                }
            }
        }
    }
    let effects: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"effe") && t.location.is_some())
        .map(|t| t.id)
        .collect();
    for id in effects {
        let Ok(fx) = effects::read::<Effect, _>(map, id) else {
            continue;
        };
        let mut seen = Vec::new();
        for p in fx.events.iter().flat_map(|e| &e.parts) {
            if p.reference.group == LIGH
                && !p.reference.is_null()
                && !seen.contains(&p.reference.id)
            {
                seen.push(p.reference.id);
                out.entry(p.reference.id.0)
                    .or_default()
                    .push(Source::Effect(name(map, id)));
            }
        }
    }
    for v in out.values_mut() {
        v.sort();
        v.dedup();
    }
    Ok(out)
}

/// One line describing a light's shape and look.
pub fn describe(l: &Light) -> String {
    let shadow = if l.no_shadow() { "no shadow" } else { "shadow" };
    let [r, g, b] = l.colour();
    let [lo, hi] = l.brightness;
    let shape = match l.kind {
        LightKind::Sphere => format!("radius {}", l.radius),
        _ => format!(
            "cone {:.0}° x{} near {} falloff {} range {}",
            l.field_of_view.to_degrees(),
            l.height_stretch,
            l.near_width,
            l.falloff_distance,
            l.cutoff_distance
        ),
    };
    let timed = if l.duration > 0.0 {
        format!(", {} s", l.duration)
    } else {
        String::new()
    };
    format!(
        "{:<10} {:<9} colour {r:.2} {g:.2} {b:.2} brightness {lo}..{hi}, {shape}{timed}",
        l.kind.name(),
        shadow
    )
}

fn list(map_name: &str, disc: Option<PathBuf>) -> Result<()> {
    let mut map = super::maps::load_map(map_name, disc)?;
    let lights: Vec<TagId> = map
        .tags()
        .iter()
        .filter(|t| t.group == LIGH)
        .map(|t| t.id)
        .collect();
    let sources = sources(&mut map)?;
    let (mut casting, mut unused) = (0, 0);
    for &id in &lights {
        let l = effects::read::<Light, _>(&mut map, id)
            .with_context(|| format!("ligh {}", name(&map, id)))?;
        casting += usize::from(!l.no_shadow());
        println!("{}", name(&map, id));
        println!("  {}", describe(&l));
        match sources.get(&id.0) {
            Some(v) => {
                for s in v {
                    println!("  {s}");
                }
            }
            None => {
                unused += 1;
                println!("  no placement, attachment or effect part");
            }
        }
    }
    println!(
        "{} light tag(s), {casting} may cast a shadow, {unused} with no source found",
        lights.len()
    );
    Ok(())
}

/// Value distributions over every light, and the shape invariants the field
/// names rest on. Exits non-zero if an invariant fails.
fn check(names: &[String], disc: Option<PathBuf>) -> Result<()> {
    let mut kinds: BTreeMap<u16, usize> = BTreeMap::new();
    let mut fails = Vec::new();
    let (mut total, mut no_shadow, mut placed, mut attached, mut effect) = (0, 0, 0, 0, 0);
    let mut fov: BTreeMap<String, usize> = BTreeMap::new();
    let mut stride_ok = 0;
    let (mut cone_radius, mut sphere_zero, mut sphere_same) = (0, 0, 0);
    let mut sphere_fov: BTreeMap<String, usize> = BTreeMap::new();
    for m in names {
        let mut map = super::maps::load_map(m, disc.clone())?;
        let ids: Vec<TagId> = map
            .tags()
            .iter()
            .filter(|t| t.group == LIGH)
            .map(|t| t.id)
            .collect();
        match placement_stride_holds(&mut map)? {
            Some(true) => stride_ok += 1,
            Some(false) => fails.push(format!("{m}: light placements do not end at the palette")),
            None => {}
        }
        let src = sources(&mut map)?;
        for v in src.values() {
            for s in v {
                match s {
                    Source::Placed(n) => placed += n,
                    Source::Attached { .. } => attached += 1,
                    Source::Effect(_) => effect += 1,
                }
            }
        }
        for id in ids {
            let l = effects::read::<Light, _>(&mut map, id)?;
            total += 1;
            no_shadow += usize::from(l.no_shadow());
            let raw = u16::from_le_bytes([l.raw[4], l.raw[5]]);
            *kinds.entry(raw).or_default() += 1;
            let who = format!("{m}: {}", name(&map, id));
            for e in invariants(&l) {
                fails.push(format!("{who}: {e}"));
            }
            if l.kind != LightKind::Sphere {
                let deg = l.field_of_view.to_degrees().round();
                *fov.entry(format!("{} {deg}", l.kind.name())).or_default() += 1;
                cone_radius += usize::from(l.radius != 0.0);
            } else {
                sphere_fov
                    .entry(format!("{}", l.field_of_view.to_degrees().round()))
                    .and_modify(|n| *n += 1)
                    .or_insert(1);
                sphere_zero += usize::from(l.radius == 0.0);
                sphere_same += usize::from(l.radius == l.specular_radius);
            }
            if l.kind == LightKind::Orthogonal {
                println!("orthogonal: {who}: {}", describe(&l));
            }
        }
    }
    println!(
        "{} map(s), {total} ligh tag(s), {no_shadow} with no shadow",
        names.len()
    );
    println!("kinds (+4): {kinds:?}");
    println!("cones (kind, angle in degrees): {fov:?}; {cone_radius} also carry a radius");
    println!(
        "spheres: angle in degrees {sphere_fov:?}; radius zero on {sphere_zero}, equal to +0x1c on {sphere_same}"
    );
    println!("sources: {placed} placement(s), {attached} attachment(s), {effect} effect part(s)");
    println!(
        "placement stride {PLACEMENT_STRIDE:#x} tiles to the palette on {stride_ok} scenario(s)"
    );
    for f in fails.iter().take(40) {
        println!("FAIL {f}");
    }
    if !fails.is_empty() {
        bail!("{} light(s) break a layout invariant", fails.len());
    }
    Ok(())
}

/// What must hold if the field names are right: shapes use only their own
/// fields, angles and distances are sane, colours are finite and in range.
pub fn invariants(l: &Light) -> Vec<String> {
    let mut out = Vec::new();
    let finite = [
        l.radius,
        l.specular_radius,
        l.near_width,
        l.height_stretch,
        l.field_of_view,
        l.falloff_distance,
        l.cutoff_distance,
        l.brightness[0],
        l.brightness[1],
        l.duration,
    ];
    if !finite.iter().all(|v| v.is_finite() && *v >= 0.0) {
        out.push(format!("a scalar is negative or not finite: {finite:?}"));
    }
    let colours = l.diffuse_colour.iter().chain(&l.specular_colour).flatten();
    if !colours
        .clone()
        .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
    {
        out.push(format!(
            "a colour is outside 0..1: {:?} {:?}",
            l.diffuse_colour, l.specular_colour
        ));
    }
    let cone = 0.0 < l.field_of_view && l.field_of_view < std::f32::consts::PI;
    match l.kind {
        // Spheres carry the kit's 1-degree default angle; only the radius
        // matters.
        LightKind::Sphere => {}
        LightKind::Frustum | LightKind::Pyramid | LightKind::Orthogonal => {
            if !cone {
                out.push(format!("{} angle {} rad", l.kind.name(), l.field_of_view));
            }
            if l.cutoff_distance < l.falloff_distance {
                out.push(format!(
                    "cutoff {} before falloff {}",
                    l.cutoff_distance, l.falloff_distance
                ));
            }
        }
        LightKind::Other(v) => out.push(format!("unknown kind {v}")),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::effects::{EffectTag, SliceMemory, light::LIGHT_SIZE};

    /// An invented light root: kind, angle (radians), radius, falloff and
    /// cutoff, the four colours all `colour`.
    fn light(kind: u16, angle: f32, radius: f32, falloff: f32, cutoff: f32, colour: f32) -> Light {
        let mut b = vec![0u8; LIGHT_SIZE];
        b[4..6].copy_from_slice(&kind.to_le_bytes());
        for (o, v) in [
            (0x18, radius),
            (0x28, angle),
            (0x2c, falloff),
            (0x30, cutoff),
        ] {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        for o in (0x40..0x70).step_by(4) {
            b[o..o + 4].copy_from_slice(&colour.to_le_bytes());
        }
        // Null tag references, as the decoder expects them.
        for o in [0x7c, 0x94, 0xa0, 0xe0] {
            b[o..o + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        let mut mem = SliceMemory::new(0x8010_0000);
        let at = mem.push(&b);
        Light::decode_at(&mut Ctx::new(&mut mem), at).unwrap()
    }

    #[test]
    fn spheres_and_sane_cones_pass() {
        // A sphere keeps the kit's 1-degree default angle.
        assert!(invariants(&light(0, 1f32.to_radians(), 2.0, 0.0, 0.0, 0.5)).is_empty());
        assert!(invariants(&light(2, 1.0, 0.0, 2.0, 6.0, 1.0)).is_empty());
    }

    #[test]
    fn bad_cones_colours_and_kinds_fail() {
        // An angle in degrees where radians belong.
        assert_eq!(invariants(&light(2, 60.0, 0.0, 2.0, 6.0, 1.0)).len(), 1);
        // Cutoff before falloff.
        assert_eq!(invariants(&light(2, 1.0, 0.0, 6.0, 2.0, 1.0)).len(), 1);
        // A colour above 1.
        assert_eq!(invariants(&light(0, 0.0, 1.0, 0.0, 0.0, 1.5)).len(), 1);
        assert_eq!(
            invariants(&light(9, 0.0, 1.0, 0.0, 0.0, 0.5)),
            ["unknown kind 9"]
        );
    }

    #[test]
    fn describe_shows_degrees_and_the_shadow_flag() {
        let l = light(2, std::f32::consts::FRAC_PI_3, 0.0, 3.0, 6.0, 1.0);
        let d = describe(&l);
        assert!(d.starts_with("frustum    shadow "), "{d}");
        assert!(d.contains("cone 60°") && d.contains("range 6"), "{d}");
        assert_eq!(
            Source::Attached {
                object: "vehi warthog".into(),
                marker: Some("headlights".into())
            }
            .to_string(),
            "attached to vehi warthog at headlights"
        );
        assert_eq!(Source::Placed(1).to_string(), "placed once in the scenario");
    }
}
