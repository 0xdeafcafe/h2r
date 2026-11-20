# pstencil: the Digsite study tags and the shadow technique

Status: Built (research). A reading of published tags and a decision; it states nothing about our code beyond what it cites.

A companion to [e3-lighting.md](e3-lighting.md). That page has the design
and the retail light record and render path ("Phase 0 findings");
the history of the cut renderer is in [pstencil-history.md](pstencil-history.md)
and the shadowlab prototype results in [notes/e3-lighting.md](notes/e3-lighting.md). This page adds what a
study of the Digsite crew's published tag set says about lights, corrects
some readings of it, and records why the shadow technique stays shadow maps
when the study notes argued for stencil volumes.

It folds in two research notes written by another session on 2026-11-16 (a
history of pstencil and a design for "pstencil-class lighting"). They were
not committed. What was new in them is here; what pstencil-history.md already had
is not repeated.

## Provenance

The sources are the same as pstencil-history.md's, plus three published
repositories:

- `digsite/H2E3`: the restored E3 2003 level for MCC, released by the
  Digsite crew with Halo Studios' approval (see pstencil-history.md,
  section 12). It ships the level's tags. They are under the MCC EULA,
  not open source.
- `digsite/h2`: the crew's Halo 2 repository for the MCC editing kit, the
  retail tag set as the kit has it. Same terms.
- `777Sev777/Halo-2-Custom-Shaders`: Sev's public replacement shadow shaders
  for the MCC Halo 2 kit.

Rules for these, which the notes followed and this page keeps:

- Nothing comes from the December 2024 and December 2025 leaks. The history
  note discussed them through news coverage only and drew no technical
  fact from them; that discussion is not carried over beyond the paragraph
  pstencil-history.md section 12 has.
- Study copies only. Tag files and shaders are fetched into a temporary
  directory outside the repository, read, and not committed or shipped. This
  page states layouts as encoding semantics ("flags dword, bit 3 = no
  shadow") and quotes no tag bytes or shader code.
- The fetch commands are at the end.

Checked: every technical claim in the design note traces to those three
repositories or to public documentation. The history note's quotes are from
Vice's oral history, the Bernie Freidin profile, Halopedia and the Steam
Workshop Q&A, all cited in pstencil-history.md.

## What the E3 tag set holds

Read 2026-11-16 from the `digsite/H2E3` tree listing and eight study copies.
Labels as in e3-lighting.md; **D** below means read from a Digsite study copy
by this project.

- About 120 `.light` tags and 7 `.light_volume` tags (**C**, the notes' count
  of the tree).
- **Paired variants.** `frustum_white` sits beside `frustum_white_no_shadow`,
  `light_tunnel` beside `light_tunnel_noshadow`. Shadow casting was chosen
  per light, in the tag (**D**: the pair differs in the no-shadow bit alone).
- **Effect lights** under `effects/lights/`: muzzle flashes per weapon, frag
  and other explosions, Pelican afterburner, Phantom fire. Every gunshot and
  blast in the demo footage is one of these (**C**).
- **Vehicle and character lights**: Warthog headlights and tail lights, Ghost
  cockpit and running lights, Longsword exhaust, first- and third-person
  flashlights (**C**).
- **Scenario fixtures**: building and wall down-lights, tent lamps, sparking
  electrics (**C**).
- **A cinematic rig** under `earthcity_e3/cinematics/superlights/`: about 30
  named lights for the cutscenes, including a cutscene sun (`x03_sun`) and
  one named `light_perez` (**C**).
- **`ghost_lit`.** The builder script renames a lit Ghost variant over the
  ordinary Ghost for the map build; the README warns it floods the debug log
  in the last cutscene (**C**).
- The README calls these the "not pstencil" lights and points to Sev's
  shaders for the improved look (**C**). The crew said on the Workshop that
  pstencil was not reimplemented and was already gone by the E3 build, and
  that gunshot and explosion lights all exist but are culled hard by the
  engine's limit on lights in view, which is why MCC's version reads darker
  than the 2003 footage (**C**, Workshop Q&A, 9 November 2024, search
  excerpts).

### The light tag, checked

The study tags are editing-kit tag files: a 0x40-byte file header with the
group `ligh`, a 16-byte field-set header, then the light definition, then the
paths of the tags it references. The definition's fields are in the same
order as the cache's 0xe4-byte root; only tag references (16 bytes in a file,
8 in a cache) and blocks (12 against 8) are wider. So file offset 0x50 is
`ligh +0`, and the layout in [effects.md](../formats/effects.md) ("ligh")
applies to both. One value changes form: the cone angle is degrees in the
file and radians in the cache. Checked on the eight study tags and on all
1,596 retail records (`h2 lights --all`).

The study tags agree with retail's: the frustum pair and `frustum_blue` are
cones of 60 degrees with ranges of 10 and 20 units; `sphere_blue`, the muzzle
flash and the frag explosion are spheres of radius 8, 1.8 and 2; the
explosion is timed (just under a second). Retail Old Mombasa still places
lights named for the E3 level (`scenarios\objects\lights\solo\earthcity\`,
street lamps among them), and six of its eight have the no-shadow bit clear.

The design note made three readings that do not hold:

| The note said | What the bytes say |
|---|---|
| Size classes: 352 bytes for sphere and effect lights, 409 for frustum, 413 for the sun, "the tail is type-specific". | The definition is the same size in every file. The tail is the referenced tags' paths, so file size follows path length. `x03_sun` is 352 bytes and a frustum. |
| `x03_sun`'s `0x00100004` at file 0x50 is "a sun/directional marker". | That word is the flags (bits 2 and 20). Its shape field says frustum, like the other cones; it is a wide, long-range cone. |
| Intensity floats near file 0x78 (white 60/12, sun 50/80). | Those are the cone's field of view in degrees and its falloff distance. Brightness is a pair at `+0x70`. |

The no-shadow reading (file 0x50, bit `0x08`) is right, and agrees with the
engine (bit 3 at `0x00031e02`, e3-lighting.md "Definition flags read off the
code").

## The retail kit's shader names

`digsite/h2`'s `tags/rasterizer/vertex_shaders_dx9/` has vertex shaders named
`stencil_*` (`stencil_env_bumped_tangentspace_transparent`,
`stencil_texture_secondary_detail_full_xform`), `lightaccum_frustum_*`,
`lightaccum_sphere_*`, `lightmap_perpixel` and `lightmap_dynamic_*` (**C**,
names only; the notes call them fossils of the cut pipeline).

What retail does with that vocabulary is already read off the Xbox code: the
dynamic lights are additive per-light passes on layers 12 and 13 over a shadow
buffer, one pass family per light shape, and no pass sets a stencil state
(e3-lighting.md, "Stencil: every use, named" and "Pass taxonomy"). So
`lightaccum_{frustum,sphere}` names the passes retail ships, not a dormant
path. A `stencil_` name alone does not show a stencil-volume path: retail's
own light flag keeps the word "stencil" on a shadow-buffer light. These are
the MCC kit's DirectX 9 rebuilds; the Xbox disc carries NV2A programs and
was not searched for these names.

## Sev's shadow-apply contract

Sev's shaders replace the MCC kit's two shadow-apply pixel passes (its
one-sample and three-sample slots). Read as documentation of that pass
(**C**; the reading is recorded in
[notes/e3-lighting.md](notes/e3-lighting.md), "Sev's shadow-apply
shaders"): it compares a projected shadow depth map with a small constant
bias, multiplies by a fog term from a second texture, fades by a constant
and is gated by vertex alpha. Neither mode is a single tap. Both filter a
wide footprint of the map; the sharp mode ("stencil-like" in the README)
then steepens the filtered result around half coverage, and the soft mode
leaves it. The E3 variants are the same two files in the other slots.

This is the same shape as retail's layer 11 (shadow apply, `ShadowFunc
GREATER` on the shadow depth level). The restoration's hard edge is
therefore a filtered edge made steep, not a nearest tap; e3-lighting.md's
Sharp form ("The edge") is that idea with four taps. The Lighting row on
the D3D path uses one nearest tap instead
([e3-lighting-setting.md](e3-lighting-setting.md)).

## The technique: why shadow maps

The design note recommended true depth-fail stencil volumes for cast
shadows, per shadow-casting light, as "historically accurate" with fill rate
"free on modern GPUs", keeping shadow maps only for compositing. This
project's decision stays **shadow maps**, for four reasons:

1. **Measured cost.** shadowlab on Zanzibar (Apple M1 Max, GPU timestamps,
   notes/e3-lighting.md) put volumes at or above
   one-tap maps everywhere: the sun at 0.71 ms against 0.25 ms at 1x, level
   at 4x (1.11 against 1.13 ms); the point light at 1.12 against 0.76 ms at
   1x and 3.33 against 2.93 ms at 4x. Fill is cheaper than on the NV2A, not
   free, and volume fill grows with resolution while the map grows only with
   its edge.
2. **Halo 2's meshes are open.** On the same run only 37% of the casters'
   triangles (Master Chief, Elite, Ghost, Banshee, every level of detail) lie
   in closed pieces, the level's clusters none at all; 238 of the sun
   volume's 2,511 side quads come from open edges. The prototype's volumes
   stay closed by construction, but where light enters an open mesh and
   meets a face turned away from it the volume lets light through that the
   map stops: the leak the prototype tests in notes/e3-lighting.md describe. Alpha-tested casters
   would cast solid with volumes and need a map anyway.
3. **The look does not need volumes.** Volume and one-tap map disagree on
   0.03% to 0.28% of lit pixels, falling with resolution. Every public
   description of the E3 look (hard edges, everything casts, self-shadowing)
   is met by a dense map with a steep edge (e3-lighting.md, "What this means
   for the technique").
4. **The authors' own record.** Digsite's lights are "not pstencil" and the
   crew's restoration gets its look from shadow maps (Sev's sharp mode).
   Bungie's stated reasons for leaving volumes were exteriors, fill and
   caster complexity (pstencil-history.md, section 5), the cases point 2
   and the Pelican measure. Retail and Halo 3 both went to shadow maps.

What volumes would still buy is exact edges at grazing angles and no bias.
e3-lighting.md answers that with a map fitted to each caster and the Sharp
edge, adds the reasons the native renderer brings (blended frames, the
stencil buffer, the baked sun), and keeps no volume fallback.

The rest of the design note's plan is not followed where retail already
has the answer: per-pixel dynamic lights are the materials' own light
passes, not a new shading path (e3-lighting.md, "What retail already
has"). One point from it is kept: MCC's dimness came from culling lights
early, so the renderer chooses its own lights and does not cap them as
the Xbox did (e3-lighting.md, "Lights: which, and how many").

## Not known

- The pstencil pass order and constants. Nothing public describes them.
- Which of the E3 tag set's lights, if any, were classed differently for
  pstencil. The README's "not pstencil" is the only word on it.
- Flag bit 20 (set on `x03_sun` and on one retail light): the engine copies
  it to the visible-light list and the consumer is not traced
  (e3-lighting.md).
- How the demo's sun shafts were made. Any shaft pass would be ours, and
  should be described as such.

## Fetching the study copies

Outside the repository only. These write into `$TMPDIR`:

```sh
D="$TMPDIR/digsite-lights"; mkdir -p "$D"
B=https://raw.githubusercontent.com/digsite/H2E3/main/tags
for f in scenarios/objects/lights/generic/frustum_white \
         scenarios/objects/lights/generic/frustum_white_no_shadow \
         scenarios/objects/lights/generic/frustum_blue \
         scenarios/objects/lights/generic/sphere_blue \
         scenarios/solo/earthcity/earthcity_e3/cinematics/superlights/x03_sun \
         scenarios/solo/earthcity/earthcity_e3/cinematics/superlights/light_perez \
         effects/lights/muzzle_flash effects/lights/grenade_frag_explosion; do
  curl -sfL "$B/$f.light" -o "$D/$(basename "$f").light"
done
```

The vertex shaders are under
`https://raw.githubusercontent.com/digsite/h2/main/tags/rasterizer/vertex_shaders_dx9/`
and Sev's shadow-apply shaders under
`https://github.com/777Sev777/Halo-2-Custom-Shaders`; fetch them the same way
if needed. Delete the directory when done.

## Sources

Added 2026-11-16, all cited, nothing copied, no leaked file opened. The rest
are in pstencil-history.md.

- Digsite, `H2E3` (tags, `H2E3_builder.py`, README): <https://github.com/digsite/H2E3>
- Digsite, `h2` (MCC editing kit tag set): <https://github.com/digsite/h2>
- Sev, Halo 2 custom shaders: <https://github.com/777Sev777/Halo-2-Custom-Shaders>
- Steam Workshop, (H2) E3 2003 Demo discussion, 9 November 2024 (search
  excerpts): <https://steamcommunity.com/workshop/filedetails/discussion/3360515088/4633735012740577154/>
- halo.bungie.org news, 2 June 2022, "Halo modders bring back stencil shadows
  to Halo 2": <https://halo.bungie.org/news.html?item=42711>
- Halo 2: Restored Lighting, a community per-pixel lighting mod on Sev's
  shaders: <https://www.nexusmods.com/halothemasterchiefcollection/mods/2603>

The history, from public sources, is in [pstencil-history.md](pstencil-history.md); its §4 list of how the demo looked is the tuning reference.
