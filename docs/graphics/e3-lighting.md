# E3 lighting: stencil shadows and dynamic lighting

A design for an opt-in lighting mode in the spirit of the Halo 2 E3 2003 demo:
hard-edged shadows from every direct light, cast by and onto everything, and
per-pixel lighting with normal maps. Phase 0 (research), phase 1
(harder, sharper retail shadows in the view: "Phase 1 as built") and phase 3
(the shadowlab comparison, decided for shadow maps) are done; the rest is
design. [pstencil.md](pstencil.md) adds the Digsite E3 tag set's lights.

The short version: **the evidence says there is nothing of the cut renderer to
switch back on in the retail game.** What retail kept is the vocabulary (a
light flag still called "stencil" in the official tools) and a shadow-buffer
system that took its place. So this is a new host-side renderer feature that
reads the retail scene, not a recovery. The README's word "recovering" should
be read that way.

It was written from the code and docs on master and from public sources,
without building or running the game. No leaked build, game file or archive
was downloaded or examined, and none may be for this work.

The setting is `[graphics] lighting = "retail" | "e3"`, the Video screen's
**Lighting** row ([settings-menu.md](../game/settings-menu.md)), default
Retail and live. `H2_LIGHTING=e3` (or `retail`) locks it, for development
and for headless `h2 run`.

## Evidence labels

- **P**: a developer's own statement, with the source. **P\*** means we read
  it through a secondary quotation (a wiki, a mirror or a search excerpt)
  because the original was unreachable, so the wording is unverified.
- **C**: a community finding or a secondary characterisation.
- **R**: read from the retail image or disc by this project, in the doc
  named.
- **S**: our inference or speculation.

## What the E3 2003 demo did

| Claim | Label | Source |
|---|---|---|
| "pstencil" began as Bernie Freidin's research project after Halo 1: stencil shadows added to the Halo engine. The Halo 2 renderer was being designed around it, with Adrian Perez and Chris Butcher. He also mentions new per-pixel effects and approximating real lighting in the pixel shader. | P | Xbox.com profile, 2003, mirrored at <https://halo.bungie.org/misc/xbox.bfreidin/> |
| It handled direct light sources; lightmaps still supplied ambient light. | P\* | Adrian Perez, via <https://www.halopedia.org/Pstencil>, citing Vice and Bungie.net's "One Final Effort" |
| The aim was one stencil-based model for all lighting, fully dynamic, inspired by SIGGRAPH papers. | P\* | Marty O'Donnell and Jaime Griesemer in Vice's oral history, <https://www.vice.com/en/article/the-complete-untold-history-of-halo-an-oral-history/> |
| It was tested with weapon-fire and grenade lighting on a night version of a Halo 1 map, in January 2003. | P\* | Perez, via Halopedia citing Bungie.net |
| Visible in the demo: sharp sun shadows, self-shadowing on characters and vehicles, shadow-casting street lamps, headlight and muzzle-flash lighting. | C | <https://www.halopedia.org/Halo_2_E3_demo> |
| Perez, on the January 2003 screenshot: "taken in January '03 with the graphics engine we went to E3 2003 with." The pstencil engine "just barely poked along at 30fps after herculean effort." | P | Bungie.net, "One Final Effort", 16 April 2010, read on the haloman30 Bungie.net archive mirror |
| Perez: "we were trying to combine lightmaps for ambient lighting with stencil for directional sources." | P | As above. This is the one first-hand statement of the shading split. |
| Freidin's pstencil added stencil shadows "using some new techniques that can only be done on the Xbox". He names no technique. | P | Freidin profile (above) |
| During E3 demo preparation, Jason Jones blames the Pelican for "generating these immense shadow volumes" and calls it "so expensive". | P\* | *Behind the Scenes: Making of Halo 2* (B-roll), via Halopedia's transcript |
| Joseph Staten, after E3: the aim had been "stencil shadows on everything, real-time reflections in the Master Chief's visor", and they found it impossible. | P\* | Same documentary, via Halopedia's transcript |
| The August 2002 announcement trailer showed stencil shadows and "extensive use of normal maps"; Halo 2 was to use normal maps everywhere, after Halo 1 used them mostly on the level. | C | TCRF "Prerelease:Halo 2" and Oozer3993's "Assembling a Cathedral out of a Hurricane", both read as search excerpts |
| Lehto: the stencil version "looked cool" but was too expensive and dropped after the announcement trailer. | P\* | Marcus Lehto on X, January 2020, via Halopedia |
| The demo's restoration lost "colored gels" support (projected coloured light), which affected Johnson's briefing scene. | C | Digsite's lead ("General") in Halo Waypoint's "Cutting Room Floor: Modding Evolved, Part 1", 2022 |
| Bloom is computed differently from retail, giving a soft glow over the whole mission. | C | Halopedia's E3 demo page |
| Forum readings of the E3 footage, 2003-04: every dynamic object casts shadows and self-shadows; a Ghost was seen casting several shadows at once. | C | Beyond3D, "Halo 2 Gameplay Video" (DemoCoder) and "halo 2...multiple shadows cast" (AlNom), read as search excerpts |
| "Doom 3-style" unified lighting. | C | Wiki and forum wording. No developer statement using it was found. A 2003 forum poster noted Bungie never claimed a unified model; the oral history's "completely dynamic lighting" (Griesemer, Vice) is the closest. |
| Normal maps and specular under those lights, light counts, falloff, the shader maths. | None | No developer statement on any of these. Freidin speaks only of "per-pixel effects" and approximating real lighting "in the Xbox pixel shader". The footage shows bump relief and strong specular (below, S). Retail has bump mapping regardless (below). |

**Whether the E3 demo itself ran pstencil is disputed.** Perez and O'Donnell
place it in the demo. Marcus Lehto and Nate Walpole say it was dropped after
the earlier announcement trailer (posts cited on Halopedia's Pstencil page).
Press on a December 2025 leak says a build dated 3 May 2003 includes the
pstencil system and that the complete demo is a build dated 11 May; it does
not say outright that the demo build runs pstencil
(<https://www.vice.com/en/article/halo-2s-legendary-e3-demo-has-leaked-after-22-years/>).
We have not examined that material and will not. For this design it does not
matter which account is right. The target is the look, described in the
table above.

The weight of public evidence is that the demo did run stencil volumes:

| Account | Says | Label |
|---|---|---|
| Perez (2010) | The January 2003 engine is the one taken to E3; it was stencil for direct light. | P |
| Jason Jones in the making-of B-roll | The Pelican's shadow volumes were hurting the demo during E3 preparation. | P\* |
| TCRF on the May 2003 builds | pstencil was supposedly cut in February 2003, but the May 3 builds and the E3 demo "seem to feature" it, with stencil shadow volumes rather than the later shadow buffer; the Xbox builds have it, the Windows build has no working lighting. The Master Chief ("cyborg") shader type there is specific to stencil, with a new bump map. | C, read as search excerpts |
| Vice and Clawsome Gamer (December 2025) | Put pstencil in the May 3 build and credit the demo with "dramatic stencil shadow lighting", without saying the May 11 build runs it. | C |
| Lehto, Walpole (X, 2020) | Dropped after the August 2002 trailer. | P\* |
| Modders on the Steam Workshop (2024) | One says pstencil was already cut by E3 and everything is shadow maps; another disagrees, citing the "razor sharp" shadows and the Pelican remark. | C, read as search excerpts |

The modders' claim predates the leak and is about the restoration, which runs
on the retail engine. Nothing public written after the leak says the May 11
build lacks stencil volumes.

**Digsite is not a leak.** It is the codename of a community modding group
that worked with 343 Industries (now Halo Studios) on cut content: "Digsite
Alpha" for pre-release Halo, "Digsite Delta" for Halo 2. It rebuilt the E3
level for the retail MCC engine from 343's internal archives and released it
free on the Steam Workshop on 9 November 2024, with a public GitHub
repository (`digsite/H2E3`) whose README points to replacement shaders for
the "not pstencil" lights. We used only its published prose: two Halo
Waypoint articles, the GitHub README and its description of the shaders, and
Halopedia's account of Ken Peters' 2024 talk. Separately, press and TCRF
report that the Digsite project's own repository leaked in December 2024 and
that more from it, including the E3 build, leaked in December 2025. Those
files are leaked material and are out of bounds.

## How it was meant to work (public sources)

What the public record supports, put together. Labels as above. Researched
2026-11-16.

### The model

- **Direct light dynamic, ambient baked.** Every direct source (sun, lamps,
  headlights, weapon fire, explosions) was a stencil-shadowed light; the
  lightmap supplied only ambient (Perez, **P**). The night Blood Gulch test
  was lit by weapon fire and grenades (Perez via Halopedia, **P\***).
- **Everything casts, including onto itself.** "Stencil shadows on
  everything" (Staten, **P\***); characters and vehicles self-shadow, and
  the sun self-shadows objects, which retail does only for dynamic lights
  (Halopedia, **C**). The Pelican cast volumes big enough to fill the screen
  (Jones, **P\***).
- **Per-pixel shading with normal maps on everything.** Normal maps were to
  be everywhere (**C**); Freidin's work was per-pixel
  maths in the NV2A pixel shader (**P**); the stencil-specific Master Chief
  shader came with a new bump map (TCRF, **C**). Visor reflections were a
  goal (Staten, **P\***).
- **Coloured projected lights.** Gels were used and were lost in the
  restoration (Digsite, **C**). Retail lights still have a gel (see "What is
  still unknown").
- **Not known from any public source:** light counts, falloff curves, the
  specular model, how many lights shadowed at once, how the ambient term was
  computed at night, or which Xbox-only technique Freidin meant. Nothing
  public describes how the sun's shadow and the lightmap were combined
  beyond "lightmaps for ambient".

### The techniques of the time

Bungie cited "SIGGRAPH papers" without naming them (Griesemer, Vice,
**P\***). The public techniques it could have built on, none of them
confirmed as Bungie's source (**S**):

- Shadow volumes: Crow, 1977; stencil-buffer volumes, Heidmann, 1991.
- Depth-fail ("Carmack's reverse"): Bilodeau and Songy, 1998-99 (patent
  US 6,384,822); Carmack's email to Kilgard, May 2000; made robust with caps,
  depth clamp and an infinite far plane by Everitt and Kilgard, NVIDIA, March
  2002, which also proposes two-sided stencil.
- Hardware shadow maps: Williams, 1978; Everitt, Rege and Cebenoyan,
  "Hardware Shadow Mapping", NVIDIA, 2001-02, for GeForce3-class depth
  textures with a hardware comparison. The NV2A is of that class and retail
  Halo 2 uses exactly this ("What replaced it").
- Per-pixel bump-mapped lighting on fixed-function combiners: Kilgard, GDC
  2000. The NV2A's register combiners are the same family.

Doom 3, the period's reference for this look, used depth-fail volumes with
per-pixel normal-mapped lights and no lightmaps. Halo 2's plan differs in one
way that matters to us: it kept lightmaps for ambient (Perez, **P**).

### The public footage

Bungie's "Halo 2: Full E3 2003 Trailer", the real-time demo, credited to Bungie
and released after the show, on the Internet Archive (`halo2_e3_realtimedemo_hires`, 9:04, added
June 2004). We watched the 480x324 file as frame grids every three seconds
and single frames at the times below. Times are into that file. At this
resolution edge sharpness is judged only on large shadows. Everything in this
table is our reading, so **S**.

| Time | What is seen |
|---|---|
| 1:07 | The Pelican lands; a large dark shape lies under it on the pad. |
| 1:45 | Medical bay: bright floor, marines shaded with dark undersides. |
| 2:00-2:09 | Interior: shadowed areas read as near black. Marines and the gun are silhouettes against a crisp-edged sunlit patch on the floor. |
| 3:19 | The turret's muzzle flash lights the Chief's back and the turret yellow-white. |
| 5:00-5:03 | Street lamps throw bright pools on the road below them. |
| 5:24 | Overpass shadows cross the road with sharp edges. |
| 5:44-5:49 | Under an overpass, a bright patch on the road moves ahead of the Warthog: headlights. The Warthog itself stays dark. |
| 6:27 | Chief and marine on the Warthog in shade are almost black, lit only at the edges. |
| 6:33, 8:36-8:42 | The Brute and Elites show strong bump relief and specular glints on armour. |
| 7:12 | A lamp pool under an overpass, edge crisp. |

Not judgeable at this resolution: whether shadows leak through thin geometry,
contact shadows under feet, and how many shadows one object casts. The 343
Industries community stream from November 2018 on the Internet Archive is
1080p and would settle some of these; we did not watch it.

### Target look

What phases 4 and 5 should be judged against. Each item has its source.

1. **Hard shadow edges, no visible filtering**, from the sun and from local
   lights, on large and small casters. Halopedia's "sharp rather than
   softened" (**C**); Steam commenter's "razor sharp" (**C**); footage 2:00,
   5:24, 7:12 (**S**).
2. **Characters and vehicles cast onto the world and onto themselves under
   the sun**, not only under dynamic lights. Staten (**P\***), Halopedia
   (**C**), Beyond3D (**C**).
3. **Large vehicles cast large shadows** (the Pelican). Jones (**P\***),
   footage 1:07 (**S**).
4. **Several shadows per object where several lights reach it.** Beyond3D on
   the Ghost (**C**).
5. **Lamps cast shadows and throw pools of light** that read as discrete
   shapes on the ground. Halopedia (**C**), footage 5:00, 7:12 (**S**).
6. **Vehicle headlights light the road in front** and move with the vehicle.
   Halopedia (**C**), footage 5:44-5:49 (**S**).
7. **Weapon fire and explosions light their surroundings**, including the
   firer. Perez via Halopedia (**P\***), Halopedia on the turret (**C**),
   footage 3:19 (**S**).
8. **Shadowed areas are very dark**: low ambient, high contrast, characters
   close to silhouettes in shade. Footage 2:00-2:09, 6:27 (**S**) only; no
   statement.
9. **Lightmaps give the ambient only;** direct light is dynamic. Perez
   (**P**). For us this stays the lightmap problem below; the item is met for
   objects and lights, not for the level's baked sun.
10. **Dynamic lights shade per pixel through normal maps**, so armour and
    walls show relief under a moving light. Freidin (**P**), normal maps
    everywhere (**C**), footage 6:33, 8:36 (**S**).
11. **Specular highlights from dynamic lights** on armour and visors. Staten
    on the visor (**P\***), footage 8:36-8:42 (**S**).
12. **Coloured, projected lights (gels)** where a light has one. Digsite
    (**C**).
13. **Out of scope but part of the look:** the demo's different bloom
    (Halopedia, **C**).

### What this means for the technique

Nothing public argues that the look needs stencil volumes. Every description
of the look is about properties a high-resolution, one-tap shadow map also
has: hard edges, everything casting, self-shadowing. The only public argument
for volumes is the Steam commenter's that 2003 hardware could not afford a
shadow map sharp enough, which is about the Xbox, not about us.

The public reasons Bungie left volumes are the ones our phase 3 measured.
Perez: "geometry/lighting fidelity compromises" (**P**), that is, meshes
simplified and closed so volumes work. Griesemer: the shadows did not work
at all in exteriors (**P\***, Halopedia's paraphrase of his X post). Jones: the Pelican's volumes filled
the screen (**P\***), which is fill cost. Butcher: the engine could not
support the environments a Halo game needs (**P\***). Retail and Halo 3 both
went to shadow maps. Shadow maps are the faithful choice for the look and the
unfaithful one only for the mechanism, which the non-goals already exclude.
What volumes would still buy, exact edges at grazing angles and no bias, is
what items 1 and 2 test, so the shadow-map path should be judged against them
at the resolutions we ship.

One community claim to discount: that retail still has true stencil lights
(Zanzibar's swinging lights, Foundation's fan room; ResetEra, 2018, **C**).
Phase 0 found no shadow-volume path in retail (**R**); those are lights
without the "no stencil shadow" flag, drawn with shadow buffers.

## Why it was cut

| Claim | Label | Source |
|---|---|---|
| The whole graphics engine was thrown away because a game could not ship on the Xbox with it; it managed a five-minute demo, not Halo-sized environments. | P\* | Chris Butcher, Eurogamer, 2010, <https://www.eurogamer.net/articles/bungie-halo-2-e3-demo-was-a-fake>, read through quotations |
| In the best case it struggled to hold 30 fps, and shipping it meant large cuts to geometry and lighting. | P\* | Perez, Bungie.net "One Final Effort", via Halopedia |
| It allowed one or two characters on screen and sometimes fell to about 3 fps. It did not work in exteriors, so newer hardware would not have saved it. | P\* | Griesemer, Vice oral history and a later post, via Halopedia |
| The Pelican's shadow volumes were a known cost during demo work. | P\* | The making-of documentary, via <https://www.halopedia.org/Behind_the_Scenes:_Making_of_Halo_2> |
| Fill rate was the specific bottleneck. | S | No developer names it for pstencil. It is the textbook cost of shadow volumes, and the official tools say retail dynamic lights are "predominantly fillrate bound" (below). |
| Memory was the main problem. | C | Forum recollection only. |

## What replaced it

Perez describes a return to Halo 1's approach: lightmaps plus shadow maps
(**P\***, as above). A Bungie GDC 2008 talk, "Lighting and Material of Halo 3"
(Hao Chen), is reported to describe Halo 2 as lightmap colour per pixel with
one dominant light direction per vertex for bump mapping (**P\***, read as a
search excerpt only).

What this project has read from the retail game agrees with that:

| Finding | Label | Doc |
|---|---|---|
| Lightmaps are palettised index textures, selected per structure render key, with the palette bound at stage 3. | R | [lighting.md](../formats/lighting.md) |
| Lightmap buckets carry per-vertex colour streams and a per-vertex signed lighting vector (stream type `30`), which the engine interpolates. Its downstream use is not reconstructed. | R | [lighting.md](../formats/lighting.md) |
| Lightmap groups hold nine-term spherical-harmonic probes per colour channel, contracted to per-vertex colour by `0x00023ca0`. | R | [lighting.md](../formats/lighting.md) |
| Render geometry always has a tangent-frame stream, so tangent-space normal maps are in retail. | R | [map-format.md](../formats/map-format.md), [structure-bsp.md](../formats/structure-bsp.md) |
| Shadows are depth textures: the first level binds a depth level as the depth target, then samples it as `LIN_D24S8` with the NV2A's hardware comparison (`D3DRS_SHADOWFUNC`). It also draws into 512x512 and 128x128 targets. | R | [d3d.md](d3d.md), [shaders.md](shaders.md) "Shadow maps" |
| `sky` tags carry lights with yaw and pitch, and a fallback object-lighting colour. | R | [lighting.md](../formats/lighting.md) |
| There are 1,596 `ligh` tags across the maps. The light array, record, update and per-light passes are now located, and the flag bits the render path tests are read off the code. | R | [effects.md](../formats/effects.md), "Phase 0 findings" below |

## What survives

| Finding | Label | Source |
|---|---|---|
| The official MCC Halo 2 tools document a light flag named "no stencil shadow", meaning the light casts no shadows. The same page describes shadow buffers with a resolution ("shadow quality bias"), antialiasing taps, a specular pass that doubles the cost, and a guideline of at most about four modest lights in view. | P (343 Industries documentation) | <https://learn.microsoft.com/en-us/halo-master-chief-collection/h2/guides/guidedynamiclights> |
| So the name "stencil" survived on a shadow-buffer implementation. | S | Follows from the row above. That page describes the MCC toolset; that the Xbox retail `ligh` flags match it bit for bit is not verified. |
| Modders report that the tools say "stencil" but produce shadow maps, and that true stencil lighting cannot be restored without engine source. | C | A Steam Workshop discussion, read as a search excerpt only |
| The community restoration of the E3 level in MCC says its lights are "not pstencil", and points to replacement pixel shaders with a sharp one-tap mode and a soft three-tap mode over the existing shadow-map path. | C | <https://github.com/digsite/H2E3>, <https://github.com/777Sev777/Halo-2-Custom-Shaders> |
| That level was rebuilt several times because pstencil-era assets and scripts lost function in the retail engine. | P\* | Ken Peters, 2024, via Halopedia's E3 demo page |
| Most of the E3 demo's light objects remain in retail with shadow casting off. | C | Halopedia's E3 demo page, citing developer commentary; not checked against tags |
| A pre-release build from May 2003 contains pstencil code. | C, unverified by us | Vice's report on the 2025 leak (above). The same is reported of <https://tcrf.net/Proto:Halo_2>, which could not be read. This says nothing about retail. |
| The retail game sets stencil render states from game code: `SetRenderState_StencilEnable` has 40 call sites in 30 functions, and `StencilFail` 15 in 6. | R | [d3d.md](d3d.md), inventory |
| Those sites are traced: one seven-mode masking helper, a quad reset, state resets and the generic setters. No tag pass sets stencil, and no code uses increment, decrement or z-fail ops. There is no shadow-volume path. | R | "Phase 0 findings" below |
| Geometry sections hold position, texture coordinate, tangent frame and lightmap coordinate streams, plus a few stream types we have not interpreted. Nothing identifies any of them as shadow-volume geometry. | R | [structure-bsp.md](../formats/structure-bsp.md) |
| Shader instances have fields no decoded engine path reads (`shad +0x0c`, `+0x2c`, `+0x34` and others). | R | [shader-instances.md](../formats/shader-instances.md), "Open items" |
| Whether any of those unread fields relates to the cut renderer. | S | Unknown. Unread is not the same as vestigial. |
| Halo-2-HD's README and write-up describe render targets and resolution only. They say nothing about stencil use or the lighting pipeline. | C | <https://github.com/grimdoomer/Halo-2-HD> |
| Halo 2 Vista restoring any of it. | None | No claim was found either way. |

**Not found, after looking:** unused shader templates or passes for shadow
volumes; tag fields for volume geometry; a dormant stencil-lighting path in
retail; any technical write-up of the original renderer.

### What that means

- **Re-enable:** only the retail dynamic-light features that exist and are
  switched off or turned down per light. If the community claim holds, some
  lights have shadow casting disabled by flag. Those shadows would be retail
  shadow buffers, not stencil volumes.
- **Newly implement:** everything that makes the E3 look. Shadow volumes or an
  equivalent, per-pixel lighting from those lights, and the scene knowledge
  both need.

Two static checks would firm this up at low cost, and are phase 0 below:
trace the stencil call sites to name each use, and find the light render path
so the `ligh` flags can be read off engine code. Both are done; see "Phase 0
findings".

## Phase 0 findings

Static reading of the retail XBE (`h2 disasm`, a linear disassembly of
`.text` searched for call sites, and the Ghidra function list) and of the
disc's tags (`h2 shader`, `h2 effect`), done 2026-11-16 without building or
running the game. Labels are this page's: **R** is read from engine code or
from the disc by this project, **S** is our inference. Names are ours unless
said otherwise. Read-only views of the structures below are in
`crates/engine/src/render.rs`, tested on invented records.

The short version: **the light data is found, and it is a plain salted datum
array the render code reads at fixed points.** The guest's dynamic lights are
additive per-light passes over a shadow buffer, on numbered layers that the
shader templates and the engine share, so draws can be told apart by layer.
Retail stencil does masking only. Nothing in it is a shadow volume.

### Stencil: every use, named

| Site | Use | Label |
|---|---|---|
| `0x00015370` `stencil_mode_set`, mode in BX, stored at `0x00467010` | The only engine code that configures a stencil test. Seven modes (`stencil::for_mode`): 0 off; 1 and 2 write 0 or `0xff` wherever a pixel is drawn (ALWAYS, REPLACE on pass); 3, 4 and 6 draw only where bit 0 is clear, set, or bits 0 and 1 are both clear (EQUAL/NOTEQUAL, KEEP); 5 sets bit 1 where bit 0 is clear. Ops are only KEEP and REPLACE: no increment, decrement or z-fail counting, which shadow volumes need. | R |
| 42 direct calls of `0x00015370` | Every caller passes 0, 1 or 2. Mode 1 is set before the base-depth (1) and lightmap (3) layers in the scene loop and the structure draw `0x0004d720`; mode 2 marks the sky (`0x0002c0b2`, before `0x0003ede0`) and structure draws with a section flag (`0x0004d6d8`, flag `0x4000`). No direct caller passes 3 to 6. | R; what reads the marks is not traced (S: modes 3 to 6 are for that, or unused) |
| `0x0003bf20` | Colour and depth writes off, stencil ALWAYS/REPLACE with reference 0, then a four-vertex quad from world points at `0x004c1b08`: resets the stencil under a quad. Called through a pointer (no direct caller). | R; purpose S |
| 24 other `SetRenderState_StencilEnable` sites, one per function (`0x0001e9cd`, `0x0002524b`, ..., `0x000314e3`, ..., `0x00052e6d`) | `StencilEnable(FALSE)`, mostly just after a `CullMode` set: state resets around a draw family. 23 pass a literal 0 or a zeroed register; `0x00026998` was not resolved. | R |
| `0x00017b60`, `0x00018ee0`, `0x0001a170`, `0x000222d0` | Generic render-state-by-index setters: every state setter is called from each of them, stencil included. They apply a tag pass's render-state list and the engine's own lists. | R |
| Tag passes | No pass of any `shad` on the disc sets a stencil state: across all 30 maps the resolved passes set only `ZFUNC`, `COLORWRITEENABLE`, `ALPHABLENDENABLE`, `ZWRITEENABLE`, blend, `ZENABLE`, `BLENDCOLOR`, alpha test, `CULLMODE`, `SAMPLEALPHA` and `FRONTFACE`. The decoder names stencil states, so their absence is real. | R |
| `ShadowFunc` | Set to `GREATER` (`0x204`) by `0x000496f0` at the start of the shadow-apply layer (`0x00031328`), back to `NEVER` (`0x200`) by `0x00049740`, which the layer end's table reaches (`0x00044649`). | R; which layer reaches `0x00049740` S |

Engine-driven render states (table `0x00467020` via `0x0001a170`) were not
enumerated. If any of them sets stencil it would be per template; nothing
above suggests it.

### The light record

| Fact | Evidence | Label |
|---|---|---|
| Lights live in a datum array: 350 records of `0x110` bytes, allocated from game state by `0x000bffa0`, header pointer at `0x004e030c`. | `0x000bffac`..`0x000bfffb` (array name at `0x0045323c`) | R |
| Created by `0x000c0350` from the effect part switch (`0x0017ab2e` → `0x0017ac70`), either attached to an object and marker or free in the world; and by `0x000c0520` from a placement record. Both call the update `0x000c0c80` once. | call sites | R |
| `+0x02` u16 flags: 1 live; 4 placed; `0x10` timed (definition `+0xb0` above zero); `0x20` copied from a placement bit. | `0x000c03b8`..`0x000c03f4`, `0x000c05cd`..`0x000c05ff` | R |
| `+0x04` `ligh` datum; `+0x14` creation tick (from the game-time global at `0x00510c54`). | constructors; age in `0x000c0e5d` | R |
| `+0x18` f32[3] and `+0x24` f32: a bounding sphere. The visible-light pass weights each light by radius over distance to the view. | `0x000319f3`..`0x00031a86` | R (centre/radius S from the formula) |
| `+0x4c`/`+0x50`/`+0x54`/`+0x56`: an object whose functions drive the light's intensity (`0x000bab40` into `+0xd0`). `+0x58` parent object, `+0x5c` marker; `+0x60`/`+0x6c`/`+0x78` frame relative to the marker. | `0x000c0d15`..`0x000c0dc4`, `0x000c048c`..`0x000c04e7` | R |
| `+0x84` position, `+0xac` forward, `+0xb8` up, `+0x90` = position + forward. Attached lights are re-posed from the marker in the render pass itself (`0x00031d4b`..`0x00031dac`), not in the tick. | constructors, `0x00031d6f` | R |
| `+0xcc` base scale; `+0xd0` intensity (fades `1 - shape(age / duration)` with definition `+0xb0` duration and `+0xb6` shape for timed lights). | `0x000c0e53`..`0x000c0ea3` | R |
| `+0xd4` and `+0xe0`: two RGB colours, each interpolated by intensity between a pair in the definition (`+0x58`/`+0x64` and `+0x40`/`+0x4c`, mode at `+0x34`), then clamped to `[0, 1]`. `+0xec` a scalar from definition `+0x70`/`+0x74`. `+0xf0`.. computed by `0x000c0b00` from definition `+0xc0`. | `0x000c0dc9`..`0x000c11d7` | R; what each colour is for S |
| `+0xc8` the parent's fade: for a biped or vehicle parent, both colours are scaled by `1 - object +0x2b0`, unless definition flag bit 7. | `0x000c0f8c`..`0x000c108f` | R (S: active camouflage) |
| `+0xf8` f32: must be above zero for the light to cast a shadow. | `0x00031e14` | R |

**Stability.** Records are salted and fixed-size; a handle stays valid until
the light is deleted. The frame of an attached light is rewritten during the
render pass, so an observer must read it there (at the per-light pass
`0x00031170` or after `0x00031dac`), not at tick end. `light::LightRecord::check`
states what an observer can rely on (live, finite, colours in range, target
equal to position plus forward) and refuses anything else.

### The light render path

`0x00031c80` (`render_dynamic_lights`, called by the scene-pass function
`0x0002bcd0` at `0x0002c03e`) does, per frame:

1. `0x00031910` builds the visible-light list from the frame's candidate list
   at `[0x00547f98]` (at most 128): per light the datum, a weight from the
   bounding sphere, and four bytes, two of them definition flag bits 5 and 20.
   `0x0004a7c0`/`0x0004ac30` then sort and trim it (arguments 3 and 0.01; S:
   keep the strongest few).
2. For each light with weight above a threshold: re-pose attached lights,
   record up to eight per frame at `0x004c6b00` (stride `0x408`), decide
   **shadow casting** (`0x00031e02`): definition flag bit 3 clear, the global
   at `0x004ba020` set, record `+0xf8` above zero, and a word returned by
   `0x000c17f0` zero (S: a cluster or view test). Gather the objects it touches (`0x000c1d90`, `0x001320f0`,
   `0x00031660`), then draw it with `0x00031170`.
3. `0x00031170` (`render_light_passes`) computes per-light flags at
   `0x004ba014` (bit 0 from a word in the caller's context, bit 1 when the colour
   at `0x00485950` is above a threshold, bit 2 shadow, bit 3 and bit 5 from
   further tests; bit 4 is cleared), then draws layers
   on the light's list (`0x004c1a60`): shadow generate (10) when bit 2,
   shadow apply (11), layer 14, light diffuse (12), and light specular (13)
   when bits 0 and 1. It ends by turning stencil off and clearing the scissor
   (`0x000314e3`, `0x000314eb`).

**Shadow depth levels.** Layer 10's begin (`0x00044408`..`0x0004449f`) picks
render target 10, cleared to colour `0xffffff`, when per-light flag bit 4 is
set and bit 5 clear, and otherwise target 9, cleared to depth 1.0. Since
`0x00031170` clears bit 4, the per-light path always takes target 9. `0x00048ee0` builds the light's shadow
projection (stores `0x28` dwords at `0x004c9798`) before the layer draws.
Shadow apply (11) samples it with `ShadowFunc` `GREATER`. The render target
table is at `0x004b4b58`, stride `0x98`; target 9 is S as the 512x512 depth
level and the other as the 128x128 target d3d.md reports.

### Definition flags read off the code

`ligh +0` is a u32. The bits the render path tests (`light::definition_flags`),
with counts over the 1,596 `ligh` records on the disc (shared tags counted per
map):

| Bit | Engine use | Set on | Label |
|---|---|---|---|
| 3 | Set: no shadow (`0x00031e02`). This is the flag the MCC tools call "no stencil shadow". | 1,275 (321 can cast) | R; the tools name P (343), the match S |
| 5 | With bit 18 clear, the frame is refreshed from the marker each frame (`0x00031d3a`); forces the shadow-apply layer (`0x000312d4`); copied to the visible list. | 98 | R; meaning S |
| 7 | Parent fade ignored (`0x000c0fe7`). | 378 | R |
| 18 | Suppresses the marker refresh with bit 5. | 27 | R |
| 20 | Copied to the visible list (`0x00031ad5`); consumer not traced. | 1 | R |
| 22 | An extra per-light call (`0x00031c20`) and a wider object gather. | 217 | R |

Most lights have shadows switched off by bit 3, which agrees with the
community claim about the E3 level's light objects (C above), but the disc
cannot say which lights those are and we did not try. The same bit is the
no-shadow bit in the Digsite E3 tag set's paired light tags
([pstencil.md](pstencil.md)). `h2 lights <map>` prints each light's shape and
this bit, with its placements, attachments and effect parts.

### Pass taxonomy: the layers

The layer index a draw carries (`0x00016b90`'s `layer`) is one numbering for
both sides: shader templates list passes per layer, and the engine draws
layer by layer, holding the current one at `0x00467130` (`layer::CURRENT_LAYER`).
`0x00044370` begins a layer (descriptor table at `0x0043fa1c`, `0x24` bytes
per layer: target, clear, list, multisample; `layer::Descriptor`),
`0x00044550` draws the current list's entries for it through `0x0003bc30`,
`0x000445d0` ends it. Roles below come from the passes the disc's 30 maps put
on each layer (their blend and depth state, and what they bind) and from the
engine's order; the numbers of passes are resolved `(lod, layer)` lists.

| Layer | Role (`layer::Role`) | Passes | List | Notes |
|---:|---|---:|---|---|
| 1 | Base depth: opaque base colour, depth write | 4,244 | scene | stencil mode 1 |
| 2 | Base overlay: blended detail | 339 | scene | gated by `0x004ba023` |
| 3 | Lightmap and per-vertex static lighting | 16,622 | scene | stencil mode 1; palette reset at end |
| 5 | Transfer (precomputed) lighting | 126 | scene | |
| 6 | Water edge blend | 2 | scene | |
| 7 | Self-illumination, additive | 3,417 | scene | |
| 8 | Environment map, additive | 977 | scene | gated by `0x004ba022` |
| 10 | Shadow generate | 10,006 | shadow casters | per light |
| 11 | Shadow apply | 9,989 | light | per light |
| 12 | Dynamic light diffuse, additive, projected or spherical | 10,137 | light | per light |
| 13 | Dynamic light specular, additive | 10,137 | light | per light |
| 14 | Opaque, depth write, on the light's list | 538 | light | role unknown |
| 15 | Transparents | 2,898 | scene | |
| 16 | Planar fog | 16,668 | scene | special case in `0x00044550` |
| 17 | Bloom | 250 | scene | gated by `0x004ba024` |
| 18, 19 | Active camouflage, two layers | 111, 30 | scene | |
| 21 | Hologram | 6 | scene | |
| 0, 4, 9, 20, 22, 23 | no passes on the disc | 0 | | 9 is on the shadow list; 20 is drawn by the scene loop |

So the order in a view (`0x0002bcd0`) is: base layers 1, 3, 5, 8, 20 on the
scene list, then the dynamic lights (10 to 14 per light, `0x00031c80`), then
sky, transparents, fog and the rest. The view can tag every draw with
`[0x00467130]` at the shader draw and composite on that.

### What is still unknown

- What reads the stencil marks of modes 1 and 2, and whether modes 3 to 6 are
  reached at all (indirect calls were not searched).
- Layers 14 and 20, and the per-light flag bits 3 to 5 at `0x004ba014`.
- Which definition fields the light passes' vertex constants come from.
  The fields themselves are named since: kind at `+0x04` (0 sphere, 2
  frustum), sphere radius `+0x18`, cone width, stretch, angle, falloff and
  cutoff `+0x20` to `+0x30`, colours `+0x40` to `+0x6f`, brightness `+0x70`
  ([effects.md](../formats/effects.md) "ligh"). `+0x14` was not confirmed as
  anything; it is zero on the study tags. These are
  in `0x00031170`'s callees (`0x0004a1e0` takes 1024.0 and the light, S: the
  scissor) and the engine vector inputs ([shader-instances.md](../formats/shader-instances.md)).
- The candidate list at `0x00547f98` and the cluster test (`0x000c17f0`).
- Engine-driven render states (`0x00467020`).

### Recommendation

Risk 1 is retired: the light data is located, its layout is fixed and
salted, and the moment it is settled is known. Risk 2 is mostly retired:
the layer number is the pass tag, and the dynamic-light layers are 10 to 13
(plus 14).

- **Next: phase 2, narrowed.** Start with two observers only: the current
  layer (`0x00467130`) attached to each draw at `0x0001bbf0`, and the light
  records read at `0x00031170`'s entry (handle in the caller's list). Prove
  them uncharged and digest-neutral with the off/on test. Then camera and
  poses. Estimate: 1 to 2 weeks for layer tagging and lights, 2 to 3 more for
  camera and object poses, so phase 2 shrinks to 3 to 5 weeks of the same
  size but with no research left in its first half.
- **Before phase 5:** trace the light passes' vertex constants (cone, range,
  falloff, gel) and layer 14. About a week, static.
- **Phase 3 (viewer prototype) can start now.** It does not depend on any of
  this, and the finding that retail has no shadow-volume path means the
  volumes-against-maps question is open on its merits.
- **Phase 1** stays the cheapest visible step: shadow buffers are layer 10
  into target 9 or 10, sampled with `GREATER` on layer 11, which the view can
  scale and force to one tap without guest changes. Built: "Phase 1 as
  built" below.

## Phase 1 as built

What was already on master: render scale (`d3d::ScaledBackend`, a 1x
reference that answers the guest and a scaled view that is shown;
high-resolution.md phases 1 and 2), per-pass submission, and in-place
depth sampling: a dirty `D24S8` depth level sampled through its texture
is turned into a raw-bits texture on the GPU (`Backend::depth_level_texture`,
`gfx::depth_bits`; [d3d.md](d3d.md) "Lazy depth levels"), with no read-back.
The shadow comparison already indexed texels by the bound texture's real
size (`textureDimensions`) and took the coordinate in the guest's size
(`tex_scale`), which is high-resolution.md's rule 2, so a larger map needed
no change there. What was missing, and is built:

- **Scaled shadow maps in the view** (`d3d::scaled`). Every `D24S8` depth
  surface not of the frame's size is a shadow level. With E3 lighting the
  view keeps a copy of it at `shadow_scale` times its size, made at first
  use, with a colour surface of that size to draw through. A draw that
  writes the level into a guest-size target (layer 10, shadow generate) is
  drawn again into the copy with colour writes off; a clear of it is
  repeated; a draw that samples it (layer 11, shadow apply) gets a texture
  of the copy from `depth_level_texture` instead of the reference's 1x one.
  What the view cannot repeat (guest bytes, a copy, a fill from the alias
  mirror, a draw it cannot make) marks the copy stale per aspect; a stale
  copy is filled from the reference (each texel repeated) at its next draw,
  or skipped for sampling (the 1x texture stands in), unless a clear makes
  it fresh first. Halo's shadow pass clears the map before it draws, so the
  mirror refill at each binding costs nothing in the view.
- **The factor**: tied to the render scale, at least 2
  (`d3d::scaled::shadow_scale`: 2, 2, 3, 4, 5, 6 for scales 1 to 6). At
  the render scale a shadow texel covers no more host pixels than it covers
  guest pixels at 1x, so a one-tap edge steps no coarser on screen than
  retail's filtered one; the minimum of 2 makes E3 sharper than retail at
  640x480 too. One setting and one rescale point; 512x512 at 6x is
  3072x3072, inside any GPU's limit. A separate factor can come later if
  the pictures ask for it.
- **One comparison tap** (`shaders::psh::PixelOptions::hard_shadow`, set by
  `Nv2aShaders` only when the view is told `set_hard_shadows`). A hard stage
  reads the nearest texel and compares once: no 2x2 percentage-closer
  filter. If a pass samples one map on several stages (taps the combiners
  average), each compares at the mean of their coordinates, so the average
  is one tap too. Whether retail's layer 11 does that is not known (S); the
  code covers both. Retail's translation is byte for byte what it was
  (`retail_shadow_lookups_are_unchanged` compares against a golden file
  written by the code before the option).
- **At render scale 1** the view used to sleep. With E3 it wakes: the frame
  at 1x, the shadow maps at 2x.
- **Live**: `ScaledBackend::follow_lighting` reads the row at every present
  and changes there, as the scale does. Copies at another factor are
  dropped at that boundary and made again at their next use.
- **The guest sees retail**: the reference keeps every level at 1x and
  answers every read (`read_depth`, `read_surface`, counts, the judge), so
  game memory, read-backs and the simulation are the 1x run's.

Tests (no game run): `crates/shaders/tests/shadow.rs` (retail and E3 golden
WGSL; one tap on the interpreter and the GPU; the mean of two stages over
one map), and `crates/d3d/tests/render_scale.rs`, E3 section, on a headless
Metal device: the 32x32 map's copy is `32k x 32k` at render scales 1 to 4;
the guest's written-back map, colour level and device memory, and the
reference's frame, equal a plain backend's byte for byte; retail through the
wrapper makes no copy and draws nothing extra; the E3 image is all black or
white and, against the caster's exact outline, wrong at 186 host pixels at
2x and 378 at 4x where retail is wrong at 658-688 and 2648-2692; the row
changes both ways at a frame boundary with no fallback frame, at 1x and 2x.

The run report's `render scale` line ends with `shadow maps at Kx: D draws,
T textures, R refills`. In a level with shadows, `T` above 0 is the scaled
map being sampled; `D` above 0 with `T` at 0 means the level went through
guest memory (written back) and the view sampled the 1x upload, still one
tap.

**In a level (2026-11-16).** The first level runs, Old Mombasa
(`03a_oldmombasa`, to present 4087) and The Heretic, at 2x, showed every
Old Mombasa frame byte for byte the same with and without E3. That is the
frames, not the path:

- Layers 10 and 11 are the game's only shadow-map path. Shadow generate
  draws into the 512x512 `LIN_D24S8` level with the 128x128
  `LIN_A8R8G8B8` target as its colour; shadow apply draws the same
  casters' receivers into the frame and samples the map (an old draw log
  has them in pairs of equal vertex counts). Old Mombasa: 5,217 draws into
  128x128, 5,217 draws with a `LinD24S8` texture, 5,123 repeated into the
  view's copy, 109 scaled textures sampled against the device's 111
  in-place depth textures (one per shadow pass), no refill. The Heretic:
  1,260, 1,260, 90 and 90. Every shadow pass the game made was drawn and
  sampled at 2x with one tap.
- The 160x120 and 320x240 targets are the colour levels of the pool over
  the shadow map (bloom and the like), not shadows. The map goes through
  the alias mirror between frames (438 stores, 436 loads), which marks the
  view's copy stale; the next shadow pass clears it whole first, so it is
  never refilled.
- About 110 shadow passes in 2,340 frames of the level: the dynamic lights
  of the opening cinematic. The run wrote one frame in 250, so its nine
  in-level frames most likely held none. The outdoor shadows of Old
  Mombasa (buildings, vehicles on the road) are in the lightmaps (layer 3,
  "What replaced it"), which phase 1 does not touch; sun shadows from
  objects are phase 4. The Heretic's ten differing frames differ in small
  spots around the cinematic's lights; whether those frames held a shadow
  pass was not recorded.

`e3_shadow_maps_survive_halos_render_target_pool` (render_scale.rs) runs
that order for three frames: shadow pass, a colour level over the map's
bytes drawn, swap. The map goes through the mirror every frame, and the
last frame's E3 image is the single-pass scene's exactly, at 1x, 2x and
4x; it differs from retail's at the shadow's edges and nowhere else
(within two map texels of the caster's outline); the guest's bytes and the
reference's frame are the plain backend's.

To catch the frames that do have shadows, the draw diagnostics list them
(`shadow-map frames K  a-b, c, ...`, frames that drew with a depth texture
bound) and `H2_BOOT_GFX_SHADOWS=N` writes the first N of them, the same
frames in a retail and an E3 run.

The first such run (to present 4000) listed 23 frames, `1745-1747,
3981-4000`, and all 21 pairs were byte for byte the same in retail and E3.
They were also entirely black, at 2x and at 1x, in both runs: the first
frames after the level loads, still faded out, and a cut to black in the
opening cinematic, under which the game still draws its lights' shadow
passes (21 of them sampled the 2x copy). So `H2_BOOT_GFX_SHADOWS` now skips
all-black frames and keeps looking.

Not done: a separate factor; float depth levels (`F24S8`, `F16`) and `D16`
maps stay 1x (their texture comes through guest memory); per-axis scale.

### Captures to take

Each pair the same frame with `H2_LIGHTING=retail` and `=e3`, at
`H2_RENDER_SCALE=2` (and once at 1 to see E3 at 640x480):

1. **The Heretic** (`00a_introduction`), the cinematic's opening shots:
   `H2_LEVEL=00a_introduction`, or the menu route of frame-budget.md
   (presents 4220 to 4520). Headless, `h2 run` with `H2_BOOT_GFX=<dir>` and
   `H2_BOOT_GFX_EVERY=30` writes `frame-NNNNN.png` (the view) and
   `frame-NNNNN-1x.png` (the reference, which must be identical between
   the two runs). Add `H2_BOOT_GFX_SHADOWS=40` (and `H2_BOOT_GFX_EVERY=never`
   for only those) to write the frames that sample the shadow map: most
   frames do not.
2. **Cairo Station** (`H2_LEVEL=01b_spacestation`), the opening corridors
   with the flashlight on, close to a Marine or a crate between the light
   and a wall: a dynamic light's shadow (S: which lights cast in which
   level was not checked, so pick a spot where retail shows a soft shadow).
3. The same spot in `h2 play` stepping the Lighting row: the edge should
   go hard at the next frame, with no flash and `d3d: lighting Retail -> E3`
   on stderr.

## Goals and non-goals

Goals:

- hard, per-pixel shadows from the sun and from dynamic lights, including
  self-shadowing on characters and vehicles;
- per-pixel diffuse and specular from dynamic lights using the materials'
  normal maps;
- off by default, and incapable of changing the simulation.

Non-goals:

- reproducing the E3 demo's level, scripts or assets. Only the retail disc's
  content is in scope;
- matching the original renderer's maths, which is not public;
- re-lighting the environment's baked lighting in early phases (see "The
  lightmap problem").

## Design

### Where it runs

The device (`d3d::Device`) and everything the guest can observe stay as they
are. [high-resolution.md](high-resolution.md) splits the backend in two: a
1x *reference* that answers every read the guest makes, and a *view* that
draws what is shown. E3 lighting lives only in the view. That one decision is
what keeps the simulation untouched: the guest's D3D calls, their results,
the write-backs and the pixel counts all come from a backend that does not
know this mode exists.

This design therefore depends on phase 1 of that one.

### What the view needs to know

The view sees `ResolvedDraw`s: streams, state, textures and vertex-program
constants. A lighting pass needs more than that, and it comes from
**observers**: read-only hooks at guest addresses, run on the runner thread
at the moment the guest reaches them.

| Need | Source | State |
|---|---|---|
| Camera and projection for the view being drawn | view records built by `0x002234e0` and used by `0x0002b790` ([engine-map.md](../engine/engine-map.md)) | Functions located; record layout not decoded |
| Which material and pass each draw belongs to | `0x0001bbf0` begins a shader draw and passes (shader, LOD, layer, pass) to `0x00016b90` ([shader-instances.md](../formats/shader-instances.md)) | Decoded |
| Which geometry each draw belongs to | `0x00041040` branches between structure clusters and render models ([engine-map.md](../engine/engine-map.md)) | Located, marked "likely" |
| The sun's direction and colour | `sky` lights ([lighting.md](../formats/lighting.md)) | Angles decoded; colour and the rest not |
| Dynamic lights: position, direction, cone, colour, range | the light datum array at `[0x004e030c]`, read at the per-light pass `0x00031170` ("Phase 0 findings"); the record's `ligh` datum gives the definition | Located: frame, colours, intensity, bounds, shadow decision. The definition's shape, cone, range, falloff, colours and brightness are decoded (`tags::effects::light`, [effects.md](../formats/effects.md) "ligh"); which of them feed the light passes' constants is not traced |
| Object poses and skinning matrices for shadow casters | the guest's object and render-cache data ([objects.md](../formats/objects.md), `0x0003d270`) | Not decoded for this use |
| Level and model geometry, including what the camera cannot see | tags: `tags::structure`, `tags::geometry`, `tags::model`; collision in [collision.md](../formats/collision.md) | Decoders exist |
| Which bitmap parameter of a material is its normal map | template and pass bindings ([shader-instances.md](../formats/shader-instances.md)) | Bindings decoded; meanings not named |

Observer rules:

- An observer reads guest memory and writes nothing: no guest memory, no CPU
  state, no device state.
- Its reads must not be charged. The runtime counts guest accesses and turns
  them into logical time (`H2_ACCESS_COUNT`, [perf-budget.md](../perf/perf-budget.md)),
  so a charged read would shift the schedule. Observers need a read path
  outside that accounting.
- It runs at a point where the data is settled: the guest's own call, not a
  host timer.
- What it produces is a host-side scene description attached to the draw
  stream as side data. The reference backend ignores it.
- An observer that cannot make sense of what it reads disables the mode for
  that frame and counts it. It never stops the guest.

### Shadows: the choice of technique

The view needs, for each direct light, a per-pixel answer to "is this point
lit?". Three ways to get it:

| Technique | For | Against |
|---|---|---|
| **Stencil shadow volumes** (depth-fail), as the original | The authentic look: exact, hard edges at any resolution. No bias tuning. | Needs closed, well-formed caster meshes, and render meshes are often not. Alpha-tested casters (grates, foliage) cast solid shadows. Fill cost grows with volume size and with resolution. Silhouette extraction per light per caster, on skinned meshes too. |
| **Host shadow maps** at high resolution with a one-tap comparison | Works with any mesh and with alpha test. Reuses the vertex programs. This is how the community restoration gets its "stencil-like" mode. | Bias and resolution artefacts. Not exact at grazing angles. Arguably not the feature the README names. |
| **Ray queries** against a host acceleration structure | Exact and simple once geometry is on the host. | wgpu's ray-query support is not something to depend on yet. Needs the same host geometry as volumes. |

Recommendation: prototype volumes and shadow maps side by side **in the
viewer** (`crates/viewer`), which already draws structure BSPs and render
models from tags with no game run. Decide on pictures and timings from that.
The scene-knowledge work is the same for all three, so the choice does not
block anything before it. That prototype is `shadowlab`; see "Phase 3:
shadow prototype".

For volumes, the caster geometry is the open design point:

- **Level.** The collision BSP is closed geometry and suits depth-fail
  volumes, but it is coarser than what is drawn, so a shadow can part from
  the surface that casts it. The render geometry matches what is seen and is
  not guaranteed closed. A level also mostly shadows itself through its
  lightmap already (below). Start with the level as a receiver only.
- **Objects.** Render models, posed with the guest's matrices. This is where
  the look comes from: characters and vehicles casting onto the world and
  themselves. Open edges need the usual treatment (per-triangle volumes or
  capping), at more fill.
- **Extrusion** on the GPU from a prebuilt edge mesh, so the CPU does not
  touch vertices per frame.

Fill cost killed this on the NV2A. A current GPU has far more fill to spare,
and the volumes can be rasterised at guest resolution even when the view is
scaled, then used as a mask. That is an estimate to be tested in the viewer,
not a result.

### Lighting: per-pixel, from the lights

Hard shadows alone are a mask. The second half is replacing how dynamic
lights shade surfaces:

1. The view builds a host-only buffer of world position (from its depth
   buffer and the observed camera) and per-pixel normal. The normal needs each
   opaque draw's normal map and tangent frame, so it needs the material
   tagging above.
2. For each dynamic light, the view accumulates diffuse and specular under the
   shadow mask.
3. The guest still issues its own dynamic-light passes. The view skips those
   draws, identified by their pass tags, and substitutes its result at the
   same point in the frame. The reference draws them as always.

Step 3 needs the pass taxonomy. Phase 0 recovered most of it: the layer index
is shared by templates and the engine, and the dynamic-light layers are 10 to
13 ("Pass taxonomy: the layers"). Layer 14 and a few empty layers are
unnamed. Compositing at the wrong point lights fog, transparents and the HUD,
so this is still the part most likely to take longer than planned.

### The lightmap problem

The E3 renderer drew direct light dynamically and kept lightmaps for ambient.
Retail lightmaps hold direct sunlight and its shadows together with bounce.
Adding a dynamic sun on top double-lights sunlit areas, and adding its shadow
mask darkens areas the lightmap already darkened.

There is no clean fix inside the retail data. Options, from cheap to costly:

- **Objects only.** The sun's hard shadows are cast by objects onto the
  level, and onto objects by objects and the level. The level's own shadows
  stay baked. This gives the most recognisable part of the look and avoids
  the double count, apart from where an object's shadow falls on an area
  already in baked shadow (mask by the lightmap's value there).
- **Estimate the direct term** from the per-vertex lighting vector and the
  probes, and subtract it. Speculative: the vector's meaning is not fully
  recovered.
- **Re-bake** the ambient term on the host from level geometry. A lightmapper
  is a project of its own.

This design commits to the first, and treats the others as research.

### The "re-enable" option

Separate from all of the above, and smaller: make retail's own shadow buffers
look harder.

- **Host-side.** In the view only, scale the shadow buffers
  ([high-resolution.md](high-resolution.md), phase 4) and force a one-tap
  comparison. No guest change. This is the cheapest visible step.
- **Guest-side.** Clear a "no shadow" flag on lights that have it. This edits
  tag data the guest reads, so the guest draws more. It is a mod, not a
  renderer option. It is outside the guarantee below until someone shows
  that light flags are read only by rendering code, and it would need its
  own flag, off by default, excluded from deterministic replays.

## Opt-in, and the simulation untouched

- **Off by default.** With the mode off there is no observer, no side data
  and no view-side pass. The code path is today's.
- **One-way data flow.** Guest to observers to view. Nothing returns.
- **The guest-visible values are the reference's.** With the mode on, every
  D3D result, write-back and pixel count is what it is with the mode off.
- **Failure is local.** A frame the view cannot light is shown with retail
  lighting.
- **No game data in the repo.** Observers and passes are tested on invented
  scenes. Viewer prototypes read the owner's disc at run time and write
  nothing into Git.

The acceptance test is the one in high-resolution.md: the same input timeline
with the mode off and on gives the same guest memory digest and the same D3D
return trace. A second test proves the observers are uncharged: the access
count and logical time per frame are identical off and on.

## Phases, effort and risks

Effort figures are guesses for one person who knows the codebase. They are
here to show relative size.

| Phase | Work | Effort | Needs |
|---|---|---|---|
| 0 | **Done 2026-11-16** ("Phase 0 findings"). Static research, no runtime change: trace the stencil call sites and name each use; find the light creation and render path from the `ligh` part case (`0x0017ab2e`); read the light flags off engine code; find who draws the shadow depth levels; name the layers in the scene-pass dispatch. Deliverables are docs, decoders and invented-data tests. | 2-4 weeks | Nothing |
| 1 | **Built 2026-11-16** ("Phase 1 as built"). Harder retail shadows in the view: scaled shadow buffers, one-tap comparison; the Lighting row. | 1-2 weeks | high-resolution.md phases 1-2, and in-place depth sampling (all were on master) |
| 2 | Observers and the scene description: camera, draw tagging, lights, object poses. Uncharged reads. Tests on invented guest memory. The light definitions are ready: `tags::effects::light` reads shape, cone, range, colour and the no-shadow flag, and `h2 lights <map>` lists a map's lights and where they come from (2026-11-16). | 3-5 weeks | Phase 0 |
| 3 | Viewer prototype: volumes against shadow maps on tag geometry, with timings. Decide. | 2-3 weeks | Nothing; can run beside phase 2 |
| 4 | Sun shadows from objects in the view, composited as a mask. | 6-10 weeks | Phases 2, 3 |
| 5 | Per-pixel dynamic lights with normal maps and specular, replacing the guest's light passes in the view. | 8-12 weeks or more | Phase 4, the pass taxonomy |
| 6 | The environment's direct light. Research; may never ship. | Unknown | |

Risks, largest first:

1. **The light data is not found or not stable.** Retired by phase 0: a
   salted datum array, read at a known point in the render pass. What is
   left is the cone, range and falloff inputs to the light passes.
2. **Pass taxonomy.** Mostly retired by phase 0: the layer number is the
   pass tag. Layer 14 and the stencil marks' readers are open.
3. **Material coverage.** There are 165 templates and 147 passes in retail
   ([shader-instances.md](../formats/shader-instances.md)). Finding the normal map for
   each needs the engine texture sources and inputs named, which are open
   items there. Expect a long tail, with a fallback to the vertex normal.
4. **The lightmap double count** limits how far the look can go without
   phase 6.
5. **Off-screen casters.** The guest culls to the frustum, so the draw stream
   lacks casters behind the camera. Shadows need tag geometry plus guest
   poses, which is more observer surface than the draw stream alone.
6. **Geometry quality** for volumes: open meshes, alpha-tested casters,
   level-of-detail switches changing silhouettes.
7. **Cost.** The view already costs a second backend. Volumes add fill, and
   the buffers add memory at 4K. A CPU-bound runtime at 5-6 fps on the menu
   has other priorities first; this sits behind 30 fps.
8. **Expectation.** The result will be a new renderer's idea of the E3 look
   on retail content. It should be described that way to users.

## Phase 3: shadow prototype

`crates/viewer/src/bin/shadowlab` renders one scene with each shadow
technique, offscreen with wgpu, and times them. It reads the owner's disc at
run time and writes only into a directory outside the repository; it refuses
an output path inside it. Nothing it writes from the disc is committed. The
numbers below are filled in by hand from a run.

```sh
cargo run -p viewer --release --bin shadowlab -- --list-models <map>
cargo run -p viewer --release --bin shadowlab -- --out "$OUT" \
    --model <mode tag> [--model <mode tag>@x,y,z[,yaw]]... [--level-casts] <map>
cargo run -p viewer --release --bin shadowlab -- --out "$OUT" --synthetic
```

### What it does

- **Scene.** The scenario's structure BSP (`--bsp N`) as the receiver:
  clusters plus placed instances, flat shaded. Casters are render models
  (`--model`, default level of detail or `--lod N`), stood on the level near
  its centre or placed at a given point. With `--level-casts` the BSP casts
  too, in every technique. `--synthetic` swaps in invented geometry (a cube,
  an open box and a plane with a hole over a ground plane).
- **Lights.** A sun (`--sun YAW,ELEV`) and a point light (`--point`, with a
  range for falloff). The default camera and point light are framed on the
  casters; the default sun is fixed (yaw 35, elevation 50) and not yet read
  from the `sky` tag.
- **Volumes.** Caster triangles are welded by position, so material and
  normal seams do not read as holes, and an edge table is built once. Per
  light, the volume is the lit faces' prisms with shared sides cancelled:
  sides on the silhouette and on lit faces' open edges, the lit faces as the
  near cap and their copies at infinity as the far cap. That volume is closed
  whatever the caster's topology, so depth-fail counting is always well
  formed; open geometry costs extra side quads instead of leaking. Rendered
  with an infinite far plane: ambient pass, two-sided stencil pass, additive
  light pass on stencil 0.
- **Shadow maps.** The sun gets one orthographic map fitted to the view
  frustum out to 2.5 times the target distance, with its depth range
  stretched over every caster, so casters outside the view still cast. A
  point light gets six cube faces. The map edge scales with the resolution
  (default 2048 at 1x for the sun, 1024 per cube face). Sampled with one
  nearest comparison tap (`map-hard`) or nine bilinear comparison taps
  (`map-pcf`), with slope-scaled bias and a normal offset of 1.5 texels.
- **Output.** For each light and scale (default 640x480 times 1, 2 and 4):
  each technique's image, a grid (volume, map-hard, map-pcf, and the
  volume-against-map-hard difference), `report.md` and `timings.csv`. Times
  are GPU timestamps at pass boundaries, median of `--frames` (30) frames,
  or wall time per technique where the adapter has no timestamp queries (the
  report says which).

### What the invented scenes show

These come from the tests (`cargo test -p viewer --bin shadowlab`), which use
invented geometry only.

- A closed cube's volume has a four-edge silhouette under a vertical sun and
  is closed under any light. Depth-pass counting fails with the eye inside a
  volume; depth-fail does not, but needs both caps.
- Open edges are counted per mesh (open, non-manifold, winding flips, closed
  pieces). An open box and a plane with a hole still give closed volumes.
- Where light enters an open mesh and meets a face turned away from it (the
  inside of the open box's far wall), the volume lets it through and the map
  shadows it. That is the open-mesh artefact to look for in the difference
  images: cyan in the grid.
- Elsewhere the volume and the one-tap map agree on almost every pixel.

### Results

One run, 2026-11-16: `--model` Master Chief, Elite, Ghost and Banshee on
Zanzibar's BSP 0, level not casting, default sun and point light, 640x480
times 1, 2 and 4. Apple M1 Max (Metal), GPU timestamps, median of 30 frames.
The images and `report.md` stay outside the repository.

Geometry (welded by position):

| Mesh | Triangles | Open edges | Non-manifold | Closed pieces | Closed triangles |
|---|---:|---:|---:|---:|---:|
| Level clusters | 12,462 | 146 | 1,114 | 0 of 28 | 0% |
| Level instance definitions | 3,727 | 35 | 624 | 55 of 145 | 64% |
| All casters, every level of detail | 8,690 | 308 | 170 | 16 of 93 | 37% |

Per caster the share of closed triangles runs from 0% (Master Chief's lowest
level of detail, Elite's two highest) to 100% (Master Chief level 1); the
Banshee has 204 open edges at every level.

Volumes:

| Light | Silhouette edges | Side quads | Of which open | Volume triangles | CPU build ms |
|---|---:|---:|---:|---:|---:|
| Sun | 2,487 | 2,511 | 238 | 14,596 | 0.25 |
| Point | 2,407 | 2,430 | 229 | 14,228 | 0.27 |

GPU ms, total per technique:

| Light | Scale | none | volume | map-hard | map-pcf |
|---|---:|---:|---:|---:|---:|
| Sun | 1x | 0.11 | 0.71 | 0.25 | 0.37 |
| Sun | 2x | 0.16 | 1.19 | 0.78 | 1.65 |
| Sun | 4x | 0.16 | 1.11 | 1.13 | 2.67 |
| Point | 1x | 0.10 | 1.12 | 0.76 | 0.60 |
| Point | 2x | 0.16 | 3.26 | 2.66 | 4.60 |
| Point | 4x | 0.13 | 3.33 | 2.93 | 6.26 |

Agreement, as a share of pixels facing the light:

| Light | Volume vs map-hard | Volume vs map-pcf | Notes |
|---|---:|---:|---|
| Sun | 0.28%, 0.15%, 0.08% at 1x, 2x, 4x | 0.26%, 0.14%, 0.08% | more pixels shadowed only by the volume than only by the map at every scale |
| Point | 0.09%, 0.05%, 0.03% | 0.09%, 0.05%, 0.03% | as above |

### Deciding

- **Cost.** The stencil pass against the map passes, and how each grows from
  1x to 4x: volume fill grows with pixels, map cost with the map edge.
- **Geometry.** Open edges and the share of closed triangles per caster; the
  side quads on open edges are the extra fill open meshes cost. Per level of
  detail, whether silhouettes change enough to pop.
- **Look.** Red and cyan in the grids: red is shadow only the volume casts,
  cyan only the map. Edge acne or peter-panning in `map-hard`, and how far
  `map-pcf` softens the hard edge the E3 look wants.
- **Not covered yet.** Alpha-tested casters (grates, foliage) are drawn
  solid by both techniques: the prototype does not read materials. Poses: models are
  drawn unposed, as the model viewer draws them. Cascades: one sun map only.

**Decided: shadow maps, one tap.** Volumes cost as much as a one-tap map or
more at every scale measured, Halo 2's casters are mostly open meshes (37% of
caster triangles closed, the level's clusters none), and the two agree on all
but a fraction of a percent of lit pixels. A study note that recommended
depth-fail volumes, and why it is not followed, is in
[pstencil.md](pstencil.md) ("The technique: why shadow maps"). Volumes for
closed object casters stay the fallback if phase 4's pictures show a case a
map cannot meet.

## Sources

Cited only. Nothing was copied, and no leaked material was downloaded or
read. Read 2026-11-15. Items marked P\* above were not read at their origin:
Eurogamer and the Steam pages refused the fetch, Bungie.net's article is
gone, the GDC slides were too large to fetch, and TCRF did not serve the
article.

- Bernie Freidin profile, Xbox.com, 2003, mirrored at
  <https://halo.bungie.org/misc/xbox.bfreidin/>
- "The Complete, Untold History of Halo", Vice:
  <https://www.vice.com/en/article/the-complete-untold-history-of-halo-an-oral-history/>
- Halopedia, "Pstencil", "Halo 2 E3 demo" and "Behind the Scenes: Making of
  Halo 2": <https://www.halopedia.org/Pstencil>,
  <https://www.halopedia.org/Halo_2_E3_demo>,
  <https://www.halopedia.org/Behind_the_Scenes:_Making_of_Halo_2>
- Eurogamer, "Bungie: Halo 2 E3 demo was a fake", 2010:
  <https://www.eurogamer.net/articles/bungie-halo-2-e3-demo-was-a-fake>
- Vice on the 2025 leak:
  <https://www.vice.com/en/article/halo-2s-legendary-e3-demo-has-leaked-after-22-years/>
- Microsoft Learn, "Halo 2 Guides: Dynamic Lights Best Practices":
  <https://learn.microsoft.com/en-us/halo-master-chief-collection/h2/guides/guidedynamiclights>
- Digsite's E3 level for MCC and the replacement shaders it points to:
  <https://github.com/digsite/H2E3>,
  <https://github.com/777Sev777/Halo-2-Custom-Shaders>
- Hao Chen, "Lighting and Material of Halo 3", GDC 2008 (title only; read as
  an excerpt)
- grimdoomer, Halo-2-HD: <https://github.com/grimdoomer/Halo-2-HD> and
  <https://icode4.coffee/?p=738>
- The Cutting Room Floor, "Proto:Halo 2": <https://tcrf.net/Proto:Halo_2>
  (not read)

Added 2026-11-16, all accessed that day. Same rules: cited, not copied, no
leaked file opened. TCRF served a page of instructions aimed at AI agents in
place of its articles, so TCRF is read through search excerpts only; Beyond3D
and the Steam Workshop refused the fetch and are search excerpts too. The
Bungie.net article was read in full on an archive mirror, so its quotes above
are P.

- Bungie.net, "One Final Effort", urk, 16 April 2010:
  <https://halo.bungie.net/News/content.aspx?cid=25650>, read at
  <https://bnetarchive.haloman30.com/News/contenta239.html?type=topnews&cid=25650>
- Bernie Freidin profile (above), re-read for exact wording
- Halopedia, "Pstencil" and "Behind the Scenes: Making of Halo 2" (above),
  re-read for the Jones, Staten, Lehto, Walpole and Griesemer citations
- Oozer3993, "An Oral History of the Halo 2 E3 2003 Demo" and "Assembling a
  Cathedral out of a Hurricane", Medium (search excerpts; the site refused
  the fetch):
  <https://medium.com/@Oozer3993/an-oral-history-of-the-halo-2-e3-2003-demo-700d978b92dc>,
  <https://medium.com/@Oozer3993/assembling-a-cathedral-out-of-a-hurricane-the-making-of-halo-2-24652d08d29a>
- GameWatcher, "Bungie 'literally scrapped' Halo 2's E3 engine", 4 December
  2010, quoting Butcher's Eurogamer interview (search excerpt):
  <https://www.gamewatcher.com/2010-12-04-bungie-literally-scrapped-halo-2-s-e3-engine-too-much-for-xbox>
- The Cutting Room Floor, "Proto:Halo 2/May 3, 2003 Builds and Tools",
  "Proto:Halo 2/E3 2003 Demo", "Prerelease:Halo 2" (search excerpts only):
  <https://tcrf.net/Proto:Halo_2/May_3,_2003_Builds_and_Tools>,
  <https://tcrf.net/Proto:Halo_2/E3_2003_Demo>, <https://tcrf.net/Prerelease:Halo_2>
- Vice, Denny Connolly, 29 December 2025 (above), re-read; Clawsome Gamer,
  Jake Bishop, 28 December 2025:
  <https://clawsomegamer.com/halo-2-e3-2003-demo-build-leaks-playable-earth-city/>
- Video Games Chronicle on the December 2024 leak and its link to the Digsite
  project (search excerpt):
  <https://www.videogameschronicle.com/news/over-25-years-of-classic-halo-content-has-leaked-online/>
- Halo Waypoint, Alex Wakeford, "Cutting Room Floor: Modding Evolved, Part 1"
  (2022) and "Digsite Demos: Halo 2" (2024):
  <https://www.halowaypoint.com/news/cutting-room-floor>,
  <https://www.halowaypoint.com/news/digsite-demos-halo-2>
- Digsite, `H2E3` README (prose only): <https://github.com/digsite/H2E3>;
  Steam Workshop item <https://steamcommunity.com/sharedfiles/filedetails/?id=3360515088>
  and its discussion threads (search excerpts)
- Steam Workshop, "Halo 2 Restored Lighting" comments (search excerpts):
  <https://steamcommunity.com/sharedfiles/filedetails/comments/3276750116>
- Beyond3D forum, 2003-04 threads "Halo 2 Gameplay Video", "New Halo 2
  screenshot", "halo 2...multiple shadows cast" (search excerpts):
  <https://forum.beyond3d.com/threads/halo-2-gameplay-video.4586/>,
  <https://forum.beyond3d.com/threads/new-halo-2-screenshot.6578/>,
  <https://forum.beyond3d.com/threads/halo-2-multiple-shadows-cast-im-probably-late.11504/>
- ResetEra, "DF Retro Extra: Halo 2 - Revisiting E3 2003's Impossible Xbox
  Demo!" (2018), on Digital Foundry's video of 6 June 2018, which we did not
  watch: <https://www.resetera.com/threads/df-retro-extra-halo-2-revisiting-e3-2003s-impossible-xbox-demo.47186/>
- Bungie, "Halo 2: Full E3 2003 Trailer", Internet Archive, watched as
  described: <https://archive.org/details/halo2_e3_realtimedemo_hires>. Not
  watched: 343 Industries' 2018 stream,
  <https://archive.org/details/halo-2-e3-2003-demo-stream-from-343-never-before-seen-alphasbetas-1080p-60-fps>
- F. C. Crow, "Shadow Algorithms for Computer Graphics", SIGGRAPH 1977
- T. Heidmann, "Real Shadows, Real Time", IRIS Universe 18, 1991
- L. Williams, "Casting Curved Shadows on Curved Surfaces", SIGGRAPH 1978
- C. Everitt and M. J. Kilgard, "Practical and Robust Stenciled Shadow
  Volumes for Hardware-Accelerated Rendering", NVIDIA, 2002:
  <https://arxiv.org/abs/cs/0301002>
- J. Carmack, email to M. Kilgard on depth-fail shadow volumes, May 2000, as
  described in the paper above and in Wikipedia's "Shadow volume"
  (<https://en.wikipedia.org/wiki/Shadow_volume>); W. Bilodeau and M. Songy,
  US patent 6,384,822
- C. Everitt, A. Rege and C. Cebenoyan, "Hardware Shadow Mapping", NVIDIA
  white paper, 2001-02 (bibliographic record only)
- M. J. Kilgard, "A Practical and Robust Bump-mapping Technique for Today's
  GPUs", GDC 2000:
  <https://developer.download.nvidia.com/assets/gamedev/docs/bumpmap.pdf>
  (not read beyond its abstract)
- H. Chen and X. Liu, "Lighting and Material of Halo 3", SIGGRAPH 2008 course
  notes, read as search excerpts; the excerpts mention Halo 2 only for its
  lightmap parameterisation:
  <https://www.realtimerendering.com/advances/s2008/SIGGRAPH-Lighting%20of%20Halo%203.pdf>
