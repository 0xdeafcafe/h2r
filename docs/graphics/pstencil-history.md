# Halo 2's stencil lighting: the pstencil engine, how it worked and why it was cut

Status: Reference (research). The cut renderer as public sources describe it; it states nothing about our code. Our reading of the retail engine is in [e3-lighting.md](e3-lighting.md) and [pstencil.md](pstencil.md).

Compiled November 2026 from public sources: VICE's Bungie oral history, a contemporary xbox.com interview with the system's author, Bungie.net posts (via archived citations), developer tweets, Halo Waypoint's Digsite articles, Halopedia, The Cutting Room Floor, Wikipedia and Eurogamer, the Digsite team's public repositories and Steam Workshop Q&A. Leaked archives are covered through news and wiki documentation only; nothing here links to them. The library of footage, the recovery story, the leak coverage and the full source list are in [notes/2026-11-pstencil-research.md](notes/2026-11-pstencil-research.md).

| Question | Answer | Section |
|---|---|---|
| What was it | A per-pixel stencil-shadow lighting engine the Halo 2 renderer was to be built around | 1 |
| Who | Bernie Freidin, with Adrian Perez and Chris Butcher | 3 |
| How | Dynamic lights per pixel, shadow volumes, self-shadowing, lightmaps for ambient only | 4 |
| Why cut | Frame rate: exteriors made the volumes unaffordable on the 2001 Xbox | 5 |
| What shipped | Baked radiosity lightmaps, shadow buffers, dynamic-light shadows flagged off | 6 |
| What is open | The pass structure, the sun shafts, the build-to-build tuning | 11 |

## 1. What pstencil was

During development Bungie built a real-time lighting engine internally called **`pstencil`**. It aimed at dynamic stencil shadow volumes and full self-shadowing on objects, with weapons fire, grenades and vehicle lights adding dynamic light over a base of baked ambient. It is the lighting in the X02 announcement trailer (August 2002) and the E3 2003 "Earth City" demo.

It was a rewrite of the lighting model, not a shadow feature added to Halo: CE. Freidin: "We're designing the Halo 2 rendering engine around this system." O'Donnell: they "thought they could do the whole engine this way". Griesemer: engineering's idea was "completely dynamic lighting". Retail Halo 2 (9 November 2004) shipped with baked lightmaps and much weaker dynamic lighting, and the gap is one of the most discussed downgrades of that console generation.

## 2. Where to see it

The look survives in public footage and official and community reconstructions. The list, with links, is in [the research record](notes/2026-11-pstencil-research.md#2026-11-where-to-see-the-look).

| Kind | Examples |
|---|---|
| Rendered with the system | X02 trailer; a night Blood Gulch screenshot (16 January 2003); the E3 2003 demo and its 2018 and 2024 replays |
| Official reconstruction | The Digsite-restored E3 demo (Steam Workshop, 9 November 2024) |
| Community | Sev's shadow shaders for the H2 editing kit; the "Halo 2: Restored Lighting" mod |

## 3. Who built it, and when

| Who | Role |
|---|---|
| Bernie Freidin | Primary author; wrote Halo: CE's rasterizer. Began the research project "shortly after" CE shipped in November 2001 |
| Adrian "Cuban" Perez, Chris Butcher | Worked on it with him |

Work ran through 2002 (the X02 trailer) into early 2003 (the night Blood Gulch test, 16 January 2003). Freidin, on the technique: stencil shadows "using some new techniques that can only be done on the Xbox".

## 4. How it worked: the documented design

From Perez's description (Bungie's "One Final Effort" post, cited via Halopedia):

| Part | What it did |
|---|---|
| Hybrid, hierarchy inverted from CE | `pstencil` supplied all direct lighting from dynamic lights, per pixel. Baked lightmaps were the ambient term only. In CE the lightmap carried the sun and dynamic lights were embellishment |
| Dynamic shadow volumes | Lights cast real shadows from arbitrary objects: caster silhouettes extruded into volumes, rendered to the stencil buffer as a per-pixel in-shadow mask |
| Self-shadowing | Objects shadow themselves (an arm across a torso), not only floors and walls |
| Gameplay lights | Gunfire, grenade blasts, vehicle headlights, street lamps. The night Blood Gulch variant existed to show this |

What the E3 2003 demo shows of it, against retail (Halopedia's list of differences):

| Demo | Retail |
|---|---|
| Sun shadows from the lightmap are sharp | Softened |
| Objects self-shadow from the sun | Objects self-shadow from dynamic lights only |
| Street lamps and light objects cast real-time shadows | The same light objects exist with shadow casting flagged off per object; the flag can be re-enabled in the editing kit |
| Warthog headlights light the road; the turret lights the ground and ejects glowing particles | Weaker |
| Bloom is a "soft glow" across the mission | Calculated differently |
| Brighter, more fluorescent plasma, blood decals and Jackal shields; darker Chief armour with a silver and purple visor reflection | Retail palette |
| The briefing scene uses coloured gels | Retail dropped gel support |

The demo was a standalone mission built for E3 on an early engine branch (no Havok yet, 3,400 lines of scripted HaloScript, a scripted path, invincible marines). Bungie admitted to Eurogamer in 2010 that it was in part "smoke and mirrors".

The official Digsite release ships the demo's tagset (`digsite/H2E3`, about 9,200 files). It shows how the lighting was authored:

- About 120 `.light` tags plus `.light_volume` tags, in **paired variants** (`frustum_white` beside `frustum_white_no_shadow`, `light_tunnel` beside `light_tunnel_noshadow`). Shadow casting was a per-light choice in the tag.
- Cutscenes lit by a dedicated rig under `scenarios/solo/earthcity/earthcity_e3/cinematics/superlights/`: dozens of named lights including `x03_sun`, machine-gun-hole lights and `light_perez`.
- Gameplay lights under `tags/effects/lights/` (`muzzle_flash`, `plasma_rifle_muzzle_flash`, `grenade_frag_explosion`, `pelican_afterburn`, `phantom_fire`, `mambasa_explode1`): every gunshot and blast in the footage is one discrete light tag.

A build from 3 May 2003, a week before the show, is recorded by The Cutting Room Floor as having heavier stencil shadows than the E3 build, so the shadows were dialled back in the final week. Pass-structure specifics (z-fail against z-pass, how the sun shafts were composited) were never documented publicly.

## 5. Why it was cut

| Who | Statement |
|---|---|
| Jaime Griesemer (VICE, May 2017) | Completely dynamic lighting "with just one or two characters on-screen" would still "occasionally reduce the frame rate to three per second" |
| Marty O'Donnell (VICE) | The E3 lighting "was just beautiful" but "we can't ship this. It wouldn't run." Two years of plan "thrown out" |
| Adrian Perez | Best case was a struggle to reach a consistent 30 FPS; shipping it everywhere needed "massive sacrifices in the geometry and lighting fidelity across the board" |
| Griesemer (X, December 2025) | Waiting for the Xbox 360 would not have saved it: "the shadows did not work at all in exterior environments" |
| Paul Bertone (VICE) | After E3 Halo 2 "was in too much trouble as it was" and the studio folded into the Halo team |

The exteriors point is the technical heart. Stencil volume cost scales with silhouette complexity, light count and screen coverage, a fill-rate tax the 2001 Xbox could not pay in open, sunlit battlefields. Doom 3 shipped the same technique in tight, dark corridors with controlled light counts. Bungie kept the lightmap half of the hybrid and cut the rest.

The end of the timeline has three positions:

| Position | Who | Claim |
|---|---|---|
| Cut after the announce trailer | Marcus Lehto, Nate Walpole (Twitter, 5 January 2020) | "We ditched that feature after the H2 announce trailer was done" |
| Still in the E3 demo | Perez, O'Donnell, the LCE making-of documentary | Pelican shadow volumes wrecked the frame rate while building the demo |
| Gone by the E3 demo | The Digsite crew (Steam Workshop Q&A, 9 November 2024) | "the pstencil engine is not reimplemented and was already cut by the E3 demo" |

The evidence reconciles roughly like this. Volumes were still being fought during demo development (the Pelican complaint, the 3 May build's heavier shadows dialled back by 11 May). By the demo build on stage the full path was out, and the look was carried by ordinary dynamic lights, the sharp lightmap and dense hand-placed light tags. Skeptics note the demo's shadows are too sharp for era shadow maps; the released builds could settle it and nobody has published that analysis.

## 6. What shipped instead

Retail Halo 2 is the fallback architecture rebuilt around the surviving half of the hybrid:

- Baked radiosity lightmaps for all static lighting, with deliberately softened sun shadows.
- Object self-shadowing only from dynamic (non-sun) lights.
- The demo's street-lamp and light-object shadows disabled by a tag flag in every shipped level.
- No volumetric stencil shadow system, but the shader vocabulary survives. The retail editing kit's `tags/rasterizer/vertex_shaders_dx9/` (`digsite/h2`) holds `stencil_env_bumped_tangentspace_transparent`, `stencil_texture_secondary_detail_full_xform`, `lightaccum_frustum_*`, `lightaccum_sphere_diffuse_*`, `lightmap_perpixel` and `lightmap_dynamic_*`.

## 7. The systems it touched

The lighting model reached across half the engine, which is why the cut scarred it. Detail is in [the research record](notes/2026-11-pstencil-research.md#2026-11-the-systems-the-cut-touched).

| System | Effect |
|---|---|
| Renderer | The retail renderer descends from a design that assumed per-pixel dynamic lighting; its shader library keeps the vocabulary |
| Lightmapper and tools | The demo map is "a demo map for an engine that no longer exists"; its HaloScript does not compile in retail |
| Tags | The demo's tag format predates retail's variants; lights carry per-light shadow flags; a separate `ghost_lit.vehicle` exists |
| Physics | The demo predates Havok; dynamic lighting and retail physics never shared a build |
| Cinematics | A hand-built light rig using features retail dropped (coloured gels) |
| Game design | The "one or two characters on-screen" limit is why the retail sandbox could exist |
| MCC | The port still culls dynamic lights hard, so the restored demo reads darker than the 2003 footage |

## 8. The recovery, 2022 to 2024: Digsite

343 Industries and the Digsite modding crew recovered and rebuilt the E3 demo mission (`earthcity_e3`) for MCC on PC. It was previewed at the VKMT showcase in 2022, unveiled at the Halo World Championship 2024 and released on Steam Workshop on 9 November 2024. The crew's tagset calls its lights the "not pstencil" ones, and Sev's shader suite stands in for the engine feature MCC lacks (`1tap/Combined_0` "Stencil-like", `3tap/Combined_1` softer). The full account is in [the research record](notes/2026-11-pstencil-research.md#2026-11-the-recovery-2022-to-2024).

## 9. The leaks

Parts of Halo Studios' internal archive are public, not by Microsoft's choice. Two events matter here:

| Date | Event |
|---|---|
| 25 December 2024 | The Digsite repository (about 90 GB) leaks via 4chan; every file's timestamp was overwritten to 1/1/2000 |
| 26 December 2025 | Two Halo 2 prototypes leak, verified by Hidden Palace: a 3 May 2003 build containing the pstencil system and the era's `blam.exe` and `sapien.exe`, and the 11 May 2003 build with the E3 demo as a runnable `.xbe` |

No authoritative technical analysis of the May 3 build has been published. This project reads no leaked file. The reason pstencil died was 2003 Xbox fill rate, which modern hardware does not share.

## 10. Timeline

| Date | Event |
|---|---|
| Nov 2001 | Halo: CE ships; Freidin begins the stencil-shadow work |
| Aug 2002 | X02 trailer rendered with the engine |
| Jan 2003 | Night Blood Gulch test (screenshot dated 16 January) |
| 3 May 2003 | Internal build with heavier stencil shadows |
| 11 May 2003 | E3 demo build finalised; shadows dialled back |
| May 2003 | E3 "Earth City" demo shown |
| 2003-04 | Dynamic shadow system cut; retail lighting finalised around lightmaps |
| 9 Nov 2004 | Halo 2 ships without pstencil |
| 2010 | Bungie admits to Eurogamer the demo was partly "smoke and mirrors" |
| 14 Nov 2018 | 343 replays the demo on stream |
| 5 Jan 2020 | Lehto and Walpole tweet their accounts of the cut |
| Jul 2022 | Halo Waypoint's *Cutting Room Floor* series introduces Digsite |
| 9 Nov 2024 | Restored E3 demo released for MCC on Steam Workshop |
| 25 Dec 2024 | Digsite repository leaks |
| 26 Dec 2025 | 3 May 2003 build and 11 May E3 demo `.xbe` leak |

## 11. Open questions

| Question | State |
|---|---|
| The pass structure | No published source documents the stencil ops, the sun-shaft composite or how pstencil lights were tagged. Two clean-room leads constrain the design: the retail kit's `stencil_*` and `lightaccum_*` vertex shaders (section 6) and the E3 tagset's light inventory with per-light shadow variants (section 4) |
| What "p" stood for | Unstated in any public source |
| The sun shafts | Whether a distinct volumetric pass or a consequence of the same lighting path is not established |
| 3 May against 11 May builds | A full-resolution comparison would quantify what was dialled back |

## 12. Further public findings

Labels: **P** a developer's own statement; **P\*** a statement read through a secondary quotation (wiki, mirror or search excerpt), so the wording is unverified; **C** a community finding; **S** inference.

**Statements and claims**

- Freidin names no technique for the stencil shadows (P). He also mentions new per-pixel effects and approximating real lighting in the pixel shader.
- Perez, on the January 2003 screenshot: "taken in January '03 with the graphics engine we went to E3 2003 with"; pstencil "just barely poked along at 30fps after herculean effort"; "we were trying to combine lightmaps for ambient lighting with stencil for directional sources" (P, Bungie.net "One Final Effort", read on an archive mirror). It was tested with weapon-fire and grenade lighting on a night version of a Halo 1 map (P\*).
- During E3 demo preparation Jason Jones blames the Pelican for "generating these immense shadow volumes" and calls it "so expensive" (P\*, the making-of documentary, via Halopedia's transcript). Joseph Staten, after E3: the aim had been "stencil shadows on everything, real-time reflections in the Master Chief's visor", and they found it impossible (P\*).
- The August 2002 trailer showed stencil shadows and "extensive use of normal maps"; Halo 2 was to use normal maps everywhere (C, TCRF "Prerelease:Halo 2" and a Medium essay, search excerpts).
- Forum readings of the 2003-04 footage: every dynamic object casts and self-shadows; a Ghost was seen casting several shadows at once (C, Beyond3D, search excerpts).
- "Doom 3-style" unified lighting is wiki and forum wording (C). No developer statement using it was found; Griesemer's "completely dynamic lighting" is the closest.
- TCRF on the May 2003 builds: pstencil was supposedly cut in February 2003, but the May 3 builds and the E3 demo "seem to feature" it, with stencil shadow volumes rather than the later shadow buffer; the Xbox builds have it and the Windows build has no working lighting. The Master Chief shader type there is specific to stencil, with a new bump map (C, search excerpts).
- Not known from any public source: light counts, falloff curves, the specular model, how many lights shadowed at once, how the ambient term was computed at night, which Xbox-only technique Freidin meant, and how the sun's shadow and the lightmap were combined beyond "lightmaps for ambient".
- Fill rate was the specific bottleneck (S; no developer names it for pstencil, but it is the textbook cost of shadow volumes and the official tools call retail dynamic lights "predominantly fillrate bound"). Memory as the main problem is forum recollection only (C).
- Digsite worked with 343 Industries on cut content ("Digsite Alpha" for pre-release Halo, "Digsite Delta" for Halo 2); its release of the E3 level for MCC is official. Press and TCRF report that the Digsite repository leaked in December 2024 and more, including the E3 build, in December 2025. That material is out of bounds for this project, and none was downloaded or read.

**What replaced it, per public sources.** Perez describes a return to Halo 1's approach: lightmaps plus shadow maps (P\*). A Bungie GDC 2008 talk, "Lighting and Material of Halo 3" (Hao Chen), is reported to describe Halo 2 as lightmap colour per pixel with one dominant light direction per vertex for bump mapping (P\*, search excerpt only).

**What survives, per the official tools.** The MCC Halo 2 tools document a light flag named "no stencil shadow", meaning the light casts no shadows. The same page describes shadow buffers with a resolution ("shadow quality bias"), antialiasing taps, a specular pass that doubles the cost, and a guideline of at most about four modest lights in view (P, 343 Industries documentation). So the name "stencil" survived on a shadow-buffer implementation (S; that the Xbox retail `ligh` flags match the MCC page bit for bit is not verified).

Modders report that the tools say "stencil" but produce shadow maps, and that true stencil lighting cannot be restored without engine source (C). The community restoration says its lights are "not pstencil" and points to replacement pixel shaders with a sharp one-tap mode and a soft three-tap mode over the existing shadow-map path (C). Most of the demo's light objects remain in retail with shadow casting off (C, not checked against tags).

One community claim to discount: that retail still has true stencil lights (Zanzibar's swinging lights, Foundation's fan room; ResetEra, 2018, C). Our reading of the retail engine found no shadow-volume path ([e3-lighting.md](e3-lighting.md), "Stencil: every use, named"); those are lights without the "no stencil shadow" flag, drawn with shadow buffers. Halo-2-HD's README and write-up describe render targets and resolution only, and no claim about Halo 2 Vista restoring any of it was found. Not found, after looking: unused shader templates or passes for shadow volumes, tag fields for volume geometry, and any technical write-up of the original renderer.

**The techniques of the time.** Bungie cited "SIGGRAPH papers" without naming them. The public techniques it could have built on, none confirmed as its source (S):

| Technique | Public source |
|---|---|
| Shadow volumes | Crow, 1977; stencil-buffer volumes, Heidmann, 1991 |
| Depth-fail ("Carmack's reverse") | Bilodeau and Songy, 1998-99 (patent US 6,384,822); Carmack's email to Kilgard, May 2000; made practical with caps, depth clamp and an infinite far plane by Everitt and Kilgard, NVIDIA, March 2002, which also proposes two-sided stencil |
| Hardware shadow maps | Williams, 1978; Everitt, Rege and Cebenoyan, "Hardware Shadow Mapping", NVIDIA, 2001-02, for GeForce3-class depth textures with a hardware comparison. The NV2A is of that class and retail Halo 2 uses exactly this |
| Per-pixel bump-mapped lighting on fixed-function combiners | Kilgard, GDC 2000. The NV2A's register combiners are the same family |

Doom 3, the period's reference for this look, used depth-fail volumes with per-pixel normal-mapped lights and no lightmaps. Halo 2's plan differed in keeping lightmaps for ambient.

**The public footage.** Bungie's "Halo 2: Full E3 2003 Trailer", the real-time demo, on the Internet Archive (`halo2_e3_realtimedemo_hires`, 9:04). The 480x324 file was watched as frame grids every three seconds and single frames at the times below. At this resolution edge sharpness is judged only on large shadows. Everything here is our reading (S).

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

Not judgeable at this resolution: whether shadows leak through thin geometry, contact shadows under feet, and how many shadows one object casts. 343 Industries' November 2018 community stream on the Internet Archive is 1080p and would settle some of these; it was not watched.

The sources for all sections are in [the research record](notes/2026-11-pstencil-research.md#2026-11-sources).
