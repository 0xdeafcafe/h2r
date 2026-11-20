# Halo 2's Stencil Lighting: the `pstencil` Engine — How It Worked, Why It Was Cut, and What Survived

*Research write-up, November 2026 (expanded reference edition). Compiled from public sources: VICE's
oral history of Bungie and its 2025 leak coverage, a contemporary xbox.com interview with the
system's author, Bungie.net's own "One Final Effort" and Weekly Update posts (via archived
citations), developer tweets, Halo Waypoint's official 2022–2024 Digsite articles, Halopedia,
The Cutting Room Floor, Wikipedia/Eurogamer, the Digsite team's public GitHub repositories and
Steam Workshop Q&A, and community video/mod references. All sources are listed at the end, grouped
by kind. Leaked archives are covered through news and wiki documentation only — nothing here links
to them.*

---

## 1. What pstencil was

During Halo 2's development, Bungie built a real-time lighting engine internally called **`pstencil`** —
"stencil lighting." Its ambition, contemporaneous with the technology behind Doom 3 (a comparison
Halopedia itself draws), was **dynamic stencil shadow volumes and full self-shadowing on objects**:
every character, vehicle, and light source casting real shadows in real time, with weapons fire,
grenades, and vehicle lights contributing dynamic light and shadow over a base of baked ambient.

Crucially, this was not a shadow feature bolted onto Halo: CE's lighting — it was a **ground-up rewrite of
the lighting model**. Direct illumination would come per-pixel from dynamic lights, and the Halo 2 renderer
itself was designed around the system: Freidin — "We're designing the Halo 2 rendering engine around this
system"; O'Donnell — "they thought they could do the whole engine this way, that they could do all the
lighting this way"; Griesemer — engineering's idea was "completely dynamic lighting." What was scrapped in
2003 was therefore not a shadow effect but the lighting architecture the game had been built on for two
years — and retail Halo 2's lighting is the fallback rebuilt in its place (see §6).

It is the lighting you see in two famous pieces of pre-release material:

- the **X02 announcement trailer** (August 2002), and
- the **E3 2003 "Earth City" gameplay demo** — the Master Chief's walk through sunlit New Mombasa streets,
  with long shadows off buildings, street lamps and Warthog headlights throwing real light, and sun shafts
  between the skyscrapers.

Neither look shipped. Halo 2 (November 9, 2004) released with baked radiosity lightmaps and dramatically
downgraded dynamic lighting — in Halopedia's words, "massively downgraded lighting in the final release of
Halo 2 compared to that shown in early pre-release trailers and press screenshots" — and the gap became one
of the most discussed downgrades of that console generation. Brian Jarrard (343 Industries, on the 2018
replay stream): *"In the world of video game sort of mythic legends, I think the Halo 2 E3 demo is right up
there. It's like the unicorn of the gaming industry."*

## 2. Where to see it — a reference library

The look survives in a surprising amount of publicly viewable material. Roughly in fidelity order:

**Rendered with the system (Bungie's own output):**

- **X02 announcement trailer, August 2002.** The trailer itself is on YouTube (official and mirrored
  uploads); Halopedia's `pstencil` page hosts a gallery of X02 screenshots expressly captioned "with
  pstencil lighting" — Chief on Cairo Station, Earth, the helmet close-up, the Mjolnir mark, BR55 shots
  (https://www.halopedia.org/Pstencil).
- **Night Blood Gulch test, January 16, 2003.** A single screenshot on Halopedia, captioned "running in
  the pstencil lighting engine, allowing for dynamic lighting from weapons fire and explosions"
  (`H2_BloodGulch_pstencil_Screenshot.jpg` on halo.wiki.gallery, via the pstencil page). This is Perez's
  weapons-fire showcase.
- **E3 2003 "Earth City" demo, May 2003.** The original stage demo footage is everywhere on YouTube. Two
  official replays matter: the **November 14, 2018 343 Industries stream** (Brian Jarrard, Sean Cooper,
  Frank O'Connor replaying the demo and showing never-before-seen alphas/betas; captured in 1080p60 by The
  Moa Archive, "Halo 2 E3 2003 Demo Stream from 343 + Never Before Seen Alphas/Betas"), and the **Halo
  World Championship 2024** community stage (Day 2), where the original demo was played live on stage
  before the Digsite remaster was unveiled (4K upscale on YouTube: `G4mbFRduYvA`).
- **Halo 2 developer commentary** (Limited Collector's Edition / MCC): Joe Staten points out the street
  lamps' real-time shadows around the 4:48 mark of the demo footage — Bungie's own narration of the
  feature.
- **The Making of Halo 2 documentary** (LCE DVD; an archive.org copy circulates): includes era alpha
  footage and the documentary record of Bungie fighting the demo's performance (the Pelican shadow-volume
  complaint).

**Official reconstructions (modern engine, original assets):**

- **The Digsite-restored E3 demo** — free Steam Workshop mod for MCC Halo 2 (Nov 9, 2024): the mission
  itself, playable, rebuilt from the original tags/scripts. The 2022 VKMT Showcase trailer is the first
  public look at the restoration.
- **Sev's public shader suite** for H2EK (`777Sev777/Halo-2-Custom-Shaders`): drop-in pixel shaders whose
  "Combined_0" mode is explicitly described as "stencil-like" shadowing, with E3-tagset-specific variants.
- **Halo 2: Restored Lighting mod** (Nexus Mods / Steam Workshop): re-enables per-pixel dynamic lighting
  across the retail campaign; video walkthroughs on YouTube (e.g. `FK5T_2NXjhI`).

**Community documentation and experiments:**

- **The Moa Archive** (YouTube): pre-alpha footage of Blood Gulch running the old pstencil-era lighting on
  original Xbox (e.g. `hjY1WnzvNvg`), plus the 2018 stream capture above and short alpha clips from the
  making-of (`bIU_Y6wAubU`).
- **ResetEra thread** *"What could have been: if Halo 2 shipped with stencil dynamic shadows"*
  (resetera.com/threads/923508) — long-running community comparison thread with side-by-side screenshots.
- **Reddit r/halo threads** on the cut (e.g. comments/`bakk4g`, comments/`v28e29`) and the 2024–25 leak
  discussion.
- **X/Twitter:** @CustomRider05's pre-alpha Blood Gulch clip post (`status/2099924536712216640`);
  @XFerginatorX's *Fixing Halo 2's Lighting* Unreal Engine 5 experiment (`status/2107744483312103453`) —
  the look rebuilt in a modern engine. Lehto's and Walpole's tweets on the cut (see §5).
- **zone archives** (Instagram `p/DeSK_JTlvuS`): side-by-side of the alpha lighting vs retail.
- **halo.bungie.org** — the long-running Bungie community site's news coverage, including the June 2022
  "Halo modders bring back stencil shadows to Halo 2" item and its mirror of the Freidin interview.

## 3. Who built it, and when

- **Bernie Freidin** (Bungie Software Design Engineer) was the primary author — the same engineer who wrote
  Halo: CE's rasterizer and its sun-glow and flashlight effects. In his own words, from an xbox.com
  interview as Halo 2 was gearing up: *"When we finished Halo, I worked on sort of a research project
  (called 'pstencil') where I took the Halo engine and added stencil shadows using some new techniques that
  can only be done on the Xbox, and it worked beautifully. We're designing the Halo 2 rendering engine
  around this system."* Per Halopedia, work began **shortly after Halo: Combat Evolved shipped in November
  2001**, with the intent of building Halo 2 around this lighting.
- **Adrian "Cuban" Perez** (engineer) and **Chris Butcher** (lead engineer) worked on it with him —
  "Adrian 'Cuban' Perez and Chris Butcher are working with me on this," per Freidin in the same interview.
- Work continued through 2002; the system was used to render the X02 announcement trailer, and development
  carried into early 2003 — including the **night variant of Blood Gulch** test (screenshot dated January
  16, 2003) where weapons fire and explosions dynamically lit and shadowed the canyon.

## 4. How it worked — the documented design

From Adrian Perez's own description (Bungie's "One Final Effort" post, cited via Halopedia):

1. **A hybrid with lightmaps — with the hierarchy inverted from Halo: CE.** Per Halopedia's summary of
   Perez: "pstencil's lighting was intended for use with directional light sources, while traditional
   lightmaps provided ambient lighting for a level. This would allow for lighting contributions from
   weapons fire and grenades to be much more dynamic and striking." `pstencil` supplied all **direct**
   lighting from dynamic light sources, computed per-pixel; baked lightmaps were reduced to the ambient
   term. In CE, the lightmap carried the direct sun lighting and dynamic lights were the embellishment;
   under pstencil the dynamic system carried the scene and the lightmap was the base coat. Retail Halo 2,
   which kept only the lightmap half, looks coherent precisely because that ambient base was designed to
   survive on its own.
2. **Dynamic shadow volumes.** Lights cast real shadows from arbitrary objects — the Doom 3 comparison is
   Halopedia's own. This is stencil-buffer shadow extrusion: the classic technique family where each
   caster's silhouette is extruded away from the light into a volume, the volume is rendered into the
   stencil buffer to build a per-pixel in-shadow mask, and shading passes then separate lit from shadowed
   pixels.
3. **Self-shadowing.** Objects shadow *themselves* — an arm casts shade across a torso, a visor across a
   chest — not just onto floors and walls.
4. **Dynamic light contribution from gameplay**: gunfire, grenade blasts, vehicle headlights, street lamps —
   the tested Blood Gulch night variant existed specifically to showcase this.

What the E3 2003 demo actually shows of it (per Halopedia's documented differences from the retail game):

- Sun shadows from the lightmap are **sharp** in the demo and were softened for retail.
- **Objects self-shadow from the sun** in the demo; in retail, objects only self-shadow from dynamic lights.
- **Street lamps and other light objects cast real-time shadows** in the demo. These light objects still
  exist in retail Halo 2 — they are simply *flagged off* from casting dynamic shadows, and the flag can be
  re-enabled per-object in the Halo 2 Editing Kit (H2EK). Joe Staten points this out in the Halo 2 developer
  commentary around the 4:48 mark of the demo footage.
- **Warthog headlights visibly illuminate the road**; firing the mounted turret lights the ground around it
  and visibly ejects glowing round particles.
- **Bloom was calculated differently** — "more of a 'soft glow' look across the entire mission" (Halopedia).
- The whole palette read hotter: plasma bolts and projectiles "more fluorescent," blood decals brighter and
  more fluorescent, Jackal shields a bright fluorescent blue, Chief's armor darker with a silver/purple
  visor reflection rather than the retail gold — a set of material responses that belong to the
  demo-era lighting path.
- The briefing scene used **colored gels** — a lighting-gel feature that retail Halo 2 dropped support
  for (Digsite's General_101, on remastering the demo: "losing support for colored gels in Halo 2 (used in
  Johnson's briefing scene), but we will see what we can do to work around those issues").

And how the demo *staged* that look: it was a standalone mission "created by Bungie out of demand for
material to show at E3 amidst the exceedingly hectic development process" (Halopedia), running on an early
engine branch (Havok physics was not yet integrated — vehicles and deaths behave like CE), shepherded by
over 3,400 lines of scripted HaloScript, with a scripted path the game nudges you along (it teleports you
to the Gauss Warthog at the right moment), invincible scripted marines, and SMG "dual-wielding" that is
really one weapon wearing two models. The demo's own script winks at it — Johnson, in-engine: "This is a
real-time feed; no smoke-and-mirrors pre-recorded bullshit!" In 2010, Bungie admitted to Eurogamer that the
demo was, in part, exactly that.

How that lighting was *authored* is now directly inspectable, because the official Digsite release ships the
demo's tagset (`digsite/H2E3` on GitHub, ~9,200 files). What it shows:

- Lights are individual `.light` tags that come in **paired variants**: `frustum_white.light` next to
  `frustum_white_no_shadow.light`, `light_tunnel` next to `light_tunnel_noshadow` — shadow-casting was a
  per-light choice, encoded right in the tag inventory (~120 `.light` tags plus `.light_volume` tags in the
  set).
- The cutscenes were lit by a dedicated rig under
  `tags/scenarios/solo/earthcity/earthcity_e3/cinematics/superlights/` — dozens of named lights including
  `x03_sun`/`x03_sunlight`, machine-gun-hole lights (`light_mghole_l/r`, with `.light_volume` companions),
  `light_elite_sword_activate_light`… and one simply called **`light_perez`** — presumably named for Adrian
  Perez.
- Gameplay dynamic lighting lived in `tags/effects/lights/`: `muzzle_flash`, `plasma_rifle_muzzle_flash`,
  `grenade_frag_explosion`, `pelican_afterburn`, `phantom_fire`, `mambasa_explode1` — every gunshot and
  blast you see in the demo footage is one of these discrete light tags.

Notably, community documentation of the demo build (The Cutting Room Floor) records that a build from
**May 3, 2003 — one week before the show — had noticeably heavier stencil shadows than the E3 build
itself**: the shadow system was being *dialed back* in the final week before the public showing, which tells
you the performance fight was already being lost a month before the demo was even on stage.

*(Pass-structure specifics beyond the above — e.g. z-fail vs z-pass stencil counting, how the sun's shafts
were composited — were never documented publicly. Everything in this section is the documented behavior;
technique-family inference is limited to the sentence in point 2.)*

## 5. Why it was cut

In the developers' own words — the Griesemer and O'Donnell quotes verbatim from VICE's three-part
oral history of Bungie (Steve Haske, May 2017), the Perez claims via Halopedia's archived citation of
Bungie's "One Final Effort" post:

- **Jaime Griesemer:** *"Engineering had read some cool SIGGRAPH papers and had this idea for completely
  dynamic lighting, like they were going to be the ones to make it work. Eventually we realized we could
  achieve that with just one or two characters on-screen, but it'd still occasionally reduce the frame rate
  to three per second."*
- **Marty O'Donnell:** *"They had taken a gamble on what I think was called a stencil lighting model. And
  they thought they could do the whole engine this way, that they could do all the lighting this way, and it
  was going to be revolutionary. Then we had the famous Halo 2 demo at E3 2003. The lighting model was just
  beautiful. But when we came back we realized: we can't ship this. It wouldn't run. There was no way we
  could do the whole game this way. It was a huge, horrible realization that the entire plan that had been
  worked on for two years was basically going to be thrown out."*
- **Adrian Perez:** in a best-case scenario the game would *struggle to reach a consistent 30 FPS*, and
  shipping it across the whole game would have required "massive sacrifices in the geometry and lighting
  fidelity across the board."
- **Griesemer, on "just wait for the Xbox 360"** (on X, `@32nds`, December 2025, via Halopedia): a delay
  wouldn't have saved it, because **"the shadows did not work at all in exterior environments."** Outdoor
  scenes — exactly what Halo is made of — were the worst case: huge caster silhouettes, enormous shadow
  volumes, unbounded fill-rate cost.
- **The verdict landed immediately after E3.** Paul Bertone, in the same oral history: *"After the E3 demo it
  was decided that Halo 2 was in too much trouble as it was, and the entire studio was folded into just the
  Halo team."*

The exteriors point is the technical heart of it. Stencil shadow volume cost scales with silhouette complexity
× number of lights × screen coverage — it's a fill-rate tax the 2001 Xbox simply could not pay in open,
sunlit battlefields full of characters. Id Software made the same trade in the opposite direction: Doom 3's
stencil shadows shipped inside tight, dark, scripted corridors with tightly controlled light counts and
encounters. Halo 2's design — big exteriors, sandbox combat, a dozen AI and vehicles in view — was the exact
inverse of what the technique could afford. Bungie chose the game over the renderer, kept the lightmap half
of the hybrid, and cut the rest.

There is honest disagreement about the tail end of the timeline, and it now has three positions:

- **Marcus Lehto** says the stencil-shadow path was abandoned *after the X02 announce trailer* (Twitter
  `@game_fabricator`, Jan 5, 2020, via Halopedia): *"Good eye. That was early in development when we were
  using the now infamous stencil shadow version of our engine. Looked cool, but turned out to be way to
  expensive for rendering. We ditched that feature after the H2 announce trailer was done."* — with
  **Nate Walpole** (`@bentllama`, same day) adding: *"We ditched a lot after the H2 announce trailer,
  including sanity. The scars, they teach."*
- **Adrian Perez and Marty O'Donnell** say the E3 2003 demo still used it — and the LCE making-of
  documentary records Bungie fighting Pelican *shadow volumes* wrecking the frame rate *while building the
  demo*.
- **The Digsite crew, who worked from the actual builds with sanctioned access, say it was over by E3.**
  In the official release's Steam Workshop Q&A (Nov 9, 2024), Digsite's Sean T: *"the pstencil engine is not
  reimplemented and was already cut by the E3 demo. there is custom shaders for to replicate some of the
  lighting used for the demo."* SEV (shader author on the restoration): *"Pstencil, and shadow volumes are
  not re-implemented, however the lighting system was modified and improved."*

The evidence threads reconcile roughly like this: shadow volumes were still being fought during demo
development (the Pelican complaint; the May 3 build's heavier shadows dialed back by May 11), and by the
demo build that shipped to the stage, the full pstencil path was out — the demo's celebrated look was
carried by the surviving ordinary dynamic lights, the sharp lightmap, and dense hand-placed light tags
(§4). Community skeptics note the demo's shadows are too razor-sharp for era shadow-maps and dispute the
"already cut" reading; the released builds can settle it, but nobody has published that analysis.

A footnote from Bungie's own site, via Halopedia's citation of a Weekly Update ("Massive Attack!"): the E3
demo binary itself *"still works and is playable even, but is waaaaaaaay too buggy as it stands to ever be
released."* And in November 2018, 343's Jarrard, Sean Cooper and Frank O'Connor replayed it on stream —
O'Connor set out to break it and couldn't; it was "actually quite robust" off-script. The build survived at
Bungie/343 all along; only the engine it ran on didn't.

## 6. What shipped instead (November 9, 2004)

What shipped is not CE's lighting carried forward — it is the **fallback architecture rebuilt after the
pstencil plan was thrown out**, re-centered on the one half of the hybrid that survived:

- Baked **radiosity lightmaps** for all static lighting, with deliberately *softened* sun shadows (the demo's
  sharp ones being a casualty of the cut pipeline).
- Object self-shadowing only from dynamic (non-sun) lights.
- The E3 demo's dynamic street-lamp and light-object shadows **disabled by a tag flag** in every shipped
  level — the engine could still do it, the content just stopped asking.
- No volumetric/stencil dynamic shadow system — but the machinery was never fully excised. The retail
  editing kit's shader library still ships stencil-named vertex shaders — `stencil_env_bumped_tangentspace_
  transparent`, `stencil_texture_secondary_detail_full_xform` — alongside `lightaccum_frustum_*` and
  `lightaccum_sphere_diffuse_*` (light-accumulation passes) and `lightmap_perpixel` /
  `lightmap_dynamic_*` (`tags/rasterizer/vertex_shaders_dx9/`, digsite/h2 repository). The naming is fossil
  evidence: the shipped game carried the cut pipeline's shader vocabulary even as no content asked for it.

This is why E3-demo-vs-retail comparison shots became infamous: the geometry and much of the art survived
into Outskirts and Metropolis, but the light was fundamentally different.

## 7. The systems it touched

The cut was not contained in a renderer file — the lighting model reached across half the engine, which is
why its removal scarred so many systems:

- **The renderer itself.** Freidin: "We're designing the Halo 2 rendering engine around this system." The
  retail renderer is the successor of a design that assumed per-pixel dynamic lighting as the primary path;
  its shader library still carries the vocabulary (`stencil_*`, `lightaccum_frustum/sphere_*`,
  `lightmap_perpixel`).
- **The lightmapper and toolchain.** Digsite's restoration work is, in large part, a catalogue of what the
  cut broke: the demo map is "a demo map for an engine that no longer exists" that had to be kept from
  "blow[ing] up the current lightmapper" (Kenneth Peters, Waypoint); its HaloScript "wouldn't compile in
  retail, and once you get past that they do not behave exactly like the showcase" (General_101, foreman of
  the restoration); Halopedia names "the pstencil lighting engine" among the incompatibilities that forced
  the mission to be "built and rebuilt several times."
- **The tag system.** The demo-era tag format predates retail's variant system — every NPC was its own tag
  set (`marine.biped`, `marine_odst.biped`…) rather than variants of one. Lights carry per-light shadow
  flags; the lit Ghost exists as a separate `ghost_lit.vehicle` the demo build swaps in. Digsite had to
  modernize all of it by hand.
- **Physics.** The E3 demo predates Havok integration entirely (vehicles use CE-era speed calculations;
  enemies "fall down" instead of ragdolling). The dynamic-lighting engine and the physics engine that
  shipped in retail Halo 2 never coexisted in one build.
- **Cinematics.** Cutscene lighting was a hand-built rig (`superlights/` with ~30 named lights), using
  features retail dropped — the colored gels of Johnson's briefing among them.
- **Game design.** Griesemer's "one or two characters on-screen" constraint was a design constraint, not
  just a rendering one — the sandbox Halo 2 shipped with (squad battles, vehicle mayhem) is the direct
  beneficiary of the choice to cut.
- **Post-processing.** The demo's different bloom ("soft glow" across the whole mission) and fluorescent
  palette belong to the same era pipeline.
- **MCC, twenty years on.** The port that finally carried the restored demo still culls dynamic lights
  aggressively ("the maximum amount of lights that can ever display at one time" — Digsite's Neo Te Aika),
  which is why the restoration reads darker than the 2003 film.

## 8. The recovery, 2022–2024: Digsite

Between 2022 and 2024, **343 Industries and the Digsite community modding crew** recovered and rebuilt the
E3 demo mission — internally named **`earthcity`** (`earthcity_e3`) — for release inside Halo: The Master
Chief Collection on PC.

How the dig began is documented in Halo Waypoint's three-part *Cutting Room Floor* series (Kenneth Peters
with Alex Wakeford, July 2022): a methodical recovery of 343's internal archives (the "\share\Halo1"
folder, "devoid of light, bereft of hope, swarming with corruption that even the Flood recoil from in
fear") starting in 2021, run by Peters with Publishing's Sean Cooper ("kornman") and help from Marcus
Lehto, then handed to an NDA'd crew of community modders — General_101, Conscars, Num0005, Scruffy,
Sean T, Ludus, Zeddikins. Halo: CE prerelease content became **Digsite Alpha**; Halo 2 development content
became **Digsite Delta**. Of the Halo 2 archive specifically: "The /share/halo2 archive was in good shape,
albeit missing many work-in-progress files from the earliest development days."

- An early restoration was previewed at the **VKMT community modding showcase, 2022**.
- The finished demo was unveiled at the **Halo World Championship 2024** (where the original demo was first
  replayed live on an actual Xbox devkit), and released as a free Steam Workshop mod on **November 9, 2024**
  — Halo 2's 20th anniversary. A companion recovered mission, **Alphamoon**, followed on November 15.
- Senior Franchise Writer **Kenneth Peters**, on working with the demo's 20-year-old source: *"We have seen
  things… things a programmer wouldn't believe…"* — his caption for "well over 3400 lines of barely
  documented haloscript originally written under an insane time crunch almost two decades ago," which the
  modern compiler no longer fully accepts.
- The mission had to be **built and rebuilt several times** because the early-development assets and scripts
  had changed or lost compatibility with final Halo 2 — Halopedia specifically cites "the pstencil lighting
  engine" among the incompatibilities.
- General_101's account (Waypoint) of the archaeology: the team started from "previous analysis of leaked
  builds and the like" plus a few recovered source files; kornman and Peters dove into the Halo 2 source
  depot for the demo's animation sources; character tags were reworked from the pre-variant format; the
  colored gels of Johnson's briefing had no retail equivalent to map to. "This may be a shocker for some,
  but several parts of the demo gameplay were smoke and mirrors!"
- Where possible the team reused original tags, audio, and assets (even hand-rebuilding lip-sync frame by
  frame, since Bungie's lip-sync software license couldn't be reused), and added nav markers because the
  demo's scripted path was never self-evident to a player.
- The public Digsite repository (`digsite/H2E3` on GitHub) documents the release. Its README's own words on
  the lighting: if you want "the improved dynamic lights… grab the shader used for the **'not pstencil'
  lights on E3**" — the tagset's lights are, by the crew's own naming, the *not-pstencil* ones, and Sev's
  community shader is what stands in for the engine feature MCC's Halo 2 doesn't have. The same README tells
  you to swap out the **`ghost_lit`** Ghost vehicle tag — the lit variant "spams debug.txt writes in
  tag_test during the final cutscene entirely killing performance," a fossil of dev-era debug behavior
  still attached to the lit tag.
- The restoration has known limits the crew discussed openly in the Workshop threads: gunshot/explosion
  lights "do all exist, but are aggressively culled due to the maximum amount of lights that can ever
  display at one time" (Digsite's Neo Te Aika) — which is why MCC footage reads darker than the 2003 film
  in side-by-side comparisons.

Sev's shader suite (public on GitHub, built for H2EK) offers two shadow-filtering modes whose descriptions
read as a small tribute: `1tap/Combined_0` — "Very Sharp, Aliasing-free, **Stencil-like** shadowing, more
dramatic shadows and stylized lighting" — and `3tap/Combined_1`, softer. The E3 variants are "specifically
for the E3 2003 Tagset... the files are flipped for E3." Building on this, the community's **Halo 2:
Restored Lighting** mod (Nexus Mods / Steam Workshop) re-enables per-pixel dynamic lighting across the
retail campaign, explicitly framed as restoring "what was supposed to be the game's original per-pixel
lighting... before the Pstencil engine was scrapped."

Halo Waypoint's release article promised "more behind the scenes information about the E3 2003 demo and
other recovered content in a future Digsite blog." The official Digsite blog corpus so far comprises the
2022 three-part *Cutting Room Floor* series and the 2023 two-part *Digsite Deliveries* series; as of
November 2026, the promised E3-demo-specific deep dive has not surfaced in searches.

## 9. The leaks: the Digsite archive goes public, 2024–2025

What Digsite worked from was Halo Studios'/343's internal archive. Parts of it are now public — not by
Microsoft's choice:

- **December 25, 2024:** the Digsite team's repository (~90 GB) leaked via 4chan — early Halo: CE
  development builds, debug builds of Halo 2 and MCC, prerelease content across the trilogy and Reach, plus
  tools and internal documents. Wikipedia (citing VGC) records that a former Digsite member denied the team
  was responsible, but also that the team had dissolved shortly beforehand, after Microsoft refused their
  request to be paid and asked them to continue as volunteers. The Cutting Room Floor, which documents
  material from this leak, notes that every file's timestamp was overwritten to 1/1/2000 00:00 GMT, so
  internal build timelines are hard to reconstruct. Community footage from this material includes a
  pre-alpha Blood Gulch running pstencil lighting on original Xbox hardware.
- **December 26, 2025:** the one that matters to this document. Two Halo 2 prototypes, verified by Hidden
  Palace: a **May 3, 2003 pre-E3 build containing the pstencil stencil-shadow rendering system itself, plus
  the era's actual editing tools (`blam.exe` and `sapien.exe`)** — and the **May 11, 2003 build with the
  complete E3 demo as a runnable .xbe**.

The upshot for this topic: pstencil is no longer known only through trailers, demos, and interviews. The
stencil-shadow system's own build and the tools Bungie used to author its lighting are in circulation,
running on original hardware and emulators (xemu) in community hands. The May 3 build is the *same* build
whose heavier shadows TCRF compared against the E3 build — so the raw material for a definitive technical
account of pstencil (its passes, its tags, its tuning) now exists outside Microsoft. No such authoritative
analysis has been published yet; what exists so far is community exploration. And to be explicit about
provenance: these were unauthorized leaks, not Digsite releases — this document links only to news coverage
and wiki documentation of them, never to the archives.

One closing observation on feasibility: the reason pstencil died was 2003 Xbox fill rate — a constraint
modern hardware does not share. That is why Sev's shaders and the Restored Lighting mod can approximate the
look within the retail engine today, and why the system in the leaked builds could, in principle, be run
whole on current PCs. The cut was a hardware decision, and the hardware moved on.

## 10. Timeline at a glance

| Date | Event |
| --- | --- |
| Nov 2001 | Halo: CE ships; Bernie Freidin begins stencil-shadow work on the Blam! engine |
| 2002 | pstencil developed through the year |
| Aug 2002 | X02 announcement trailer rendered with the stencil-shadow engine |
| Jan 2003 | Night Blood Gulch dynamic-lighting test (screenshot dated Jan 16) |
| May 3 2003 | Internal build with heavier stencil shadows |
| May 11 2003 | E3 demo build finalized; shadows dialed back from the May 3 build |
| May 2003 | E3 "Earth City" demo shown — the look that defined the legend |
| 2003–04 | Dynamic shadow system cut; retail lighting finalized around lightmaps |
| Nov 9 2004 | Halo 2 ships without pstencil |
| 2010 | Bungie admits to Eurogamer the demo was partly "smoke and mirrors" |
| Nov 14 2018 | 343 (Jarrard, Cooper, O'Connor) replays the demo on stream; O'Connor fails to break it |
| Jan 5 2020 | Lehto and Walpole tweet their accounts of the cut |
| Jul 2022 | Halo Waypoint's three-part *Cutting Room Floor* series introduces Digsite |
| 2022 | Digsite restoration previewed (VKMT showcase) |
| Jul 2023 | *Digsite Deliveries* series |
| Oct 2024 | Halo World Championship: demo replayed on an Xbox devkit; restoration unveiled |
| Nov 9 2024 | Restored E3 demo released for MCC on Steam Workshop; Alphamoon follows Nov 15 |
| Dec 25 2024 | ~90 GB Digsite repository leaks via 4chan (early builds, debug builds, tools, docs) |
| Dec 26 2025 | May 3 2003 build (pstencil system + blam/sapien tools) and May 11 E3 demo .xbe leak; verified by Hidden Palace; Griesemer fields the "why not wait for 360" question on X |

## 11. Open questions

- **The pass structure.** No *published* source documents the exact rendering passes (stencil ops, how the
  sun shafts were composited, how pstencil lights were tagged). Two clean-room leads now constrain the
  design without touching leaked code: the retail kit's surviving `stencil_*` / `lightaccum_*` vertex
  shaders (§6), and the official E3 tagset's light inventory with per-light shadow variants (§4). The
  December 2025 leak put the May 3 build's `sapien` editor in circulation too — someone just has to do and
  publish the analysis.
- **What "p" stood for.** Unstated anywhere public. Don't trust anyone who tells you.
- **The shafts specifically.** The sun shafts in the demo footage are visually iconic, but recovered
  documentation centers on the shadow-volume system; whether the shafts were a distinct volumetric pass or a
  consequence of the same lighting path is not established in anything cited here.
- **Full-resolution comparisons** of the May 3 and May 11 2003 builds — both now in circulation after the
  December 2025 leak — would quantify exactly what was dialed back in the final week before E3.

## 12. Further public findings

Accounts, techniques and readings that sections 4 to 6 lack. Labels: **P** a
developer's own statement; **P\*** a statement read through a secondary
quotation (a wiki, a mirror or a search excerpt), so the wording is
unverified; **C** a community finding; **S** inference.

**Statements and claims**

- Freidin's pstencil added stencil shadows "using some new techniques that can
  only be done on the Xbox". He names no technique (P). He also mentions new
  per-pixel effects and approximating real lighting in the pixel shader.
- Perez, on the January 2003 screenshot: "taken in January '03 with the
  graphics engine we went to E3 2003 with"; pstencil "just barely poked along
  at 30fps after herculean effort"; "we were trying to combine lightmaps for
  ambient lighting with stencil for directional sources" (P, Bungie.net "One
  Final Effort", read on an archive mirror). It was tested with weapon-fire
  and grenade lighting on a night version of a Halo 1 map in January 2003
  (P\*).
- During E3 demo preparation Jason Jones blames the Pelican for "generating
  these immense shadow volumes" and calls it "so expensive" (P\*, the
  making-of documentary, via Halopedia's transcript). Joseph Staten, after
  E3: the aim had been "stencil shadows on everything, real-time reflections
  in the Master Chief's visor", and they found it impossible (P\*).
- The August 2002 announcement trailer showed stencil shadows and "extensive
  use of normal maps"; Halo 2 was to use normal maps everywhere (C, TCRF
  "Prerelease:Halo 2" and a Medium essay, search excerpts).
- Forum readings of the 2003-04 footage: every dynamic object casts and
  self-shadows; a Ghost was seen casting several shadows at once (C,
  Beyond3D, search excerpts).
- "Doom 3-style" unified lighting is wiki and forum wording (C). No developer
  statement using it was found; Griesemer's "completely dynamic lighting" is
  the closest.
- TCRF on the May 2003 builds: pstencil was supposedly cut in February 2003,
  but the May 3 builds and the E3 demo "seem to feature" it, with stencil
  shadow volumes rather than the later shadow buffer; the Xbox builds have it
  and the Windows build has no working lighting. The Master Chief shader type
  there is specific to stencil, with a new bump map (C, search excerpts).
- Not known from any public source: light counts, falloff curves, the specular
  model, how many lights shadowed at once, how the ambient term was computed
  at night, which Xbox-only technique Freidin meant, and how the sun's shadow
  and the lightmap were combined beyond "lightmaps for ambient".
- Why it was cut, beyond section 5: fill rate was the specific bottleneck (S;
  no developer names it for pstencil, but it is the textbook cost of shadow
  volumes and the official tools call retail dynamic lights "predominantly
  fillrate bound"). Memory as the main problem is forum recollection only (C).
- Digsite is a community modding group that worked with 343 Industries on cut
  content ("Digsite Alpha" for pre-release Halo, "Digsite Delta" for Halo 2);
  its release of the E3 level for MCC is official. Separately, press and TCRF
  report that the Digsite project's own repository leaked in December 2024 and
  that more, including the E3 build, leaked in December 2025. That material is
  out of bounds for this project, and none was downloaded or read.

**What replaced it, per public sources.** Perez describes a return to Halo
1's approach: lightmaps plus shadow maps (P\*). A Bungie GDC 2008 talk,
"Lighting and Material of Halo 3" (Hao Chen), is reported to describe Halo 2
as lightmap colour per pixel with one dominant light direction per vertex for
bump mapping (P\*, search excerpt only).

**What survives, per the official tools.** The MCC Halo 2 tools document a
light flag named "no stencil shadow", meaning the light casts no shadows. The
same page describes shadow buffers with a resolution ("shadow quality bias"),
antialiasing taps, a specular pass that doubles the cost, and a guideline of
at most about four modest lights in view (P, 343 Industries documentation).
So the name "stencil" survived on a shadow-buffer implementation (S; that the
Xbox retail `ligh` flags match the MCC page bit for bit is not verified).
Modders report that the tools say "stencil" but produce shadow maps, and
that true stencil lighting cannot be restored without engine source (C). The
community restoration says its lights are "not pstencil" and points to
replacement pixel shaders with a sharp one-tap mode and a soft three-tap mode
over the existing shadow-map path (C). Most of the demo's light objects remain
in retail with shadow casting off (C, not checked against tags). One
community claim to discount: that retail still has true stencil lights
(Zanzibar's swinging lights, Foundation's fan room; ResetEra, 2018, C). Our
reading of the retail engine found no shadow-volume path
([e3-lighting.md](e3-lighting.md), "Stencil: every use, named"); those are
lights without the "no stencil shadow" flag, drawn with shadow buffers.
Halo-2-HD's README and write-up describe render targets and resolution only.
No claim about Halo 2 Vista restoring any of it was found either way. Not
found, after looking: unused shader templates or passes for shadow volumes,
tag fields for volume geometry, and any technical write-up of the original
renderer.

**The techniques of the time.** Bungie cited "SIGGRAPH papers" without
naming them. The public techniques it could have built on, none confirmed as
its source (S):

- Shadow volumes: Crow, 1977; stencil-buffer volumes, Heidmann, 1991.
- Depth-fail ("Carmack's reverse"): Bilodeau and Songy, 1998-99 (patent US
  6,384,822); Carmack's email to Kilgard, May 2000; made robust with caps,
  depth clamp and an infinite far plane by Everitt and Kilgard, NVIDIA, March
  2002, which also proposes two-sided stencil.
- Hardware shadow maps: Williams, 1978; Everitt, Rege and Cebenoyan,
  "Hardware Shadow Mapping", NVIDIA, 2001-02, for GeForce3-class depth
  textures with a hardware comparison. The NV2A is of that class and retail
  Halo 2 uses exactly this.
- Per-pixel bump-mapped lighting on fixed-function combiners: Kilgard, GDC
  2000. The NV2A's register combiners are the same family.

Doom 3, the period's reference for this look, used depth-fail volumes with
per-pixel normal-mapped lights and no lightmaps. Halo 2's plan differed in
keeping lightmaps for ambient.

**The public footage.** Bungie's "Halo 2: Full E3 2003 Trailer", the
real-time demo, on the Internet Archive (`halo2_e3_realtimedemo_hires`, 9:04).
The 480x324 file was watched as frame grids every three seconds and single
frames at the times below. At this resolution edge sharpness is judged only
on large shadows. Everything here is our reading (S).

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

Not judgeable at this resolution: whether shadows leak through thin
geometry, contact shadows under feet, and how many shadows one object
casts. 343 Industries' November 2018 community stream on the Internet Archive
is 1080p and would settle some of these; it was not watched.

## Sources

**Primary — Bungie and Microsoft people, on the record:**

- VICE — *The Complete, Untold History of Halo* (Steve Haske, May 2017; three-part oral history — the
  Griesemer, O'Donnell, and Bertone quotes verbatim):
  https://www.vice.com/en/article/the-complete-untold-history-of-halo-an-oral-history/
- xbox.com — *Master Chief Bernie Freidin, Bungie Software Design Engineer* (interview by Violet Leigh,
  mirrored by halo.bungie.org; Freidin's own description of the pstencil research project):
  https://halo.bungie.org/misc/xbox.bfreidin/
- Bungie.net — *One Final Effort* (Adrian Perez; directional-light/lightmap-ambient design, the 30 FPS and
  Blood Gulch claims — cited and archived via Halopedia's pstencil page).
- Bungie.net — *Weekly What's Update: Massive Attack!* ("the E3 demo… still works and is playable even, but
  is waaaaaaaay too buggy as it stands to ever be released" — cited via Halopedia's E3 demo page).
- Eurogamer — *Halo 2 E3 demo was "smoke and mirrors"* (2010; Bungie's admission, cited via Wikipedia's
  *Earth City demo* article): https://www.eurogamer.net/halo-2-e3-demo-was-smoke-and-mirrors
- Marcus Lehto, Twitter `@game_fabricator`, Jan 5 2020 ("the now infamous stencil shadow version of our
  engine… We ditched that feature after the H2 announce trailer was done") — via Halopedia.
- Nate Walpole, Twitter `@bentllama`, Jan 5 2020 ("We ditched a lot after the H2 announce trailer, including
  sanity. The scars, they teach.") — via Halopedia.
- Jaime Griesemer, X `@32nds`, December 2025 (Xbox 360 delay wouldn't have helped; "the shadows did not work
  at all in exterior environments") — via Halopedia.
- *Halo 2* developer commentary (LCE/MCC; Joe Staten on street-lamp shadows at ~4:48 of the demo).

**Official recovery — Halo Waypoint and the Digsite release:**

- Halo Waypoint — *Cutting Room Floor* (Part 1 of the July 2022 trilogy; Kenneth Peters' account, the
  Earth City/General_101 section, the 3,400-line script, colored gels): 
  https://www.halowaypoint.com/news/cutting-room-floor
- Halo Waypoint — *Cutting Room Corps* (Part 2; the crew Q&A, General_101 on the E3 scripts not compiling
  in retail): https://www.halowaypoint.com/news/cutting-room-corps
- Halo Waypoint — *Digsite Deliveries* (July 2023): https://www.halowaypoint.com/news/digsite-deliveries
- Halo Waypoint — *Digsite Demos: Halo 2* (Nov 2024 release announcement):
  https://www.halowaypoint.com/news/digsite-demos-halo-2
- Halo World Championship 2024 — Community Stage Day 2 (original demo replayed; remaster unveiled; Jarrard
  quotes from Halopedia's coverage; 4K upscale on YouTube `G4mbFRduYvA`).
- The Moa Archive / UncookedMoa — *Halo 2 E3 2003 Demo Stream from 343 + Never Before Seen Alphas/Betas*
  (1080p60 capture of the Nov 14, 2018 stream — Halopedia's primary source for Jarrard's and the stream's
  quotes).
- Steam Workshop — (H2) E3 2003 Demo, release + discussions (Digsite crew Q&A: Sean T and SEV on pstencil
  not being reimplemented and "already cut by the E3 demo"; Neo Te Aika on light culling):
  https://steamcommunity.com/workshop/filedetails/discussion/3360515088/4633735012740577154/
- digsite/H2E3 — the released E3 demo repository (tagset, `ghost_lit`, Sev shader pointer, team credits):
  https://github.com/digsite/H2E3
- digsite/h2 — Halo 2 Digsite repository for the MCC editing kits (retail-kit tag set incl. the
  `stencil_*` and `lightaccum_*` vertex shaders): https://github.com/digsite/h2

**Wikis, press, and leak coverage:**

- Halopedia — *pstencil* (the One Final Effort citations, Lehto/Walpole/Griesemer posts, X02 and Blood
  Gulch galleries): https://www.halopedia.org/Pstencil
- Halopedia — *Halo 2 E3 Demo* (Jarrard's "unicorn" quote, the 2018 stream, the differences list — bloom,
  sharp sun shadows, street-lamp flags, Havok, fluorescent palette — the Digsite remaster section):
  https://www.halopedia.org/Halo_2_E3_demo
- The Cutting Room Floor — *Proto:Halo 2/E3 2003 Demo* (build dates, May 3 vs E3 comparison, demo boot
  configuration): https://tcrf.net/Proto:Halo_2/E3_2003_Demo
- The Cutting Room Floor — *Category:Halo Digsite Repository leaks* (what the Dec 2024/2025 leaks
  contained; the zeroed filedates): https://tcrf.net/Category:Halo_Digsite_Repository_leaks
- Wikipedia — *Earth City demo* (leak provenance; the 2010 "fake" admission citing Eurogamer; Digsite team
  dissolution context citing VGC): https://en.wikipedia.org/wiki/Earth_City_demo
- VICE — *Halo 2's Legendary E3 Demo Has Leaked After 22 Years* (Denny Connolly, Dec 29 2025):
  https://www.vice.com/en/article/halo-2s-legendary-e3-demo-has-leaked-after-22-years/
- Clawsome Gamer — *Halo 2 E3 2003 Demo Build Leaks* (Jake Bishop, Dec 28 2025; the May 3 build's pstencil
  system and blam/sapien tools, as cited by VICE):
  https://www.clawsomegamer.com/halo-2-e3-2003-demo-build-leaks-playable-earth-city/
- halo.bungie.org news, June 2 2022 — *Halo modders bring back stencil shadows to Halo 2* (the restoration
  effort's first public nod): https://halo.bungie.org/news.html?item=42711

**Community reconstructions and style references:**

- Sev — Halo 2 custom shaders for H2EK (shadow-filter modes incl. "stencil-like" 1tap; E3 tagset variants):
  https://github.com/777Sev777/Halo-2-Custom-Shaders
- Halo 2: Restored Lighting mod (community per-pixel lighting restoration built on Sev's shaders; video
  `FK5T_2NXjhI`): https://www.nexusmods.com/halothemasterchiefcollection/mods/2603 and
  https://steamcommunity.com/sharedfiles/filedetails/?id=3743458113
- The Moa Archive — pre-alpha/pstencil-era footage (YouTube `hjY1WnzvNvg`, `bIU_Y6wAubU`).
- *The Making of Halo 2* documentary (Limited Collector's Edition DVD; archive.org copy circulates at
  `archive.org/details/Halo_2_Making_Of_Video`).
- ResetEra — *What could have been: if Halo 2 shipped with stencil dynamic shadows* (community comparison
  thread): https://www.resetera.com/threads/923508/
- Reddit r/halo cut-lighting discussions (e.g. `reddit.com/r/halo/comments/bakk4g/`, `…/v28e29/`).
- X/Twitter — @CustomRider05 pre-alpha Blood Gulch clip (`x.com/CustomRider05/status/2099924536712216640`);
  @XFerginatorX *Fixing Halo 2's Lighting* UE5 experiment
  (`x.com/XFerginatorX/status/2107744483312103453`).
- zone archives — alpha-vs-retail lighting comparison (Instagram `instagram.com/p/DeSK_JTlvuS`).

Further sources for section 12 (cited, not copied; no leaked file opened;
TCRF, Beyond3D, the Medium essays and the Steam Workshop are read through
search excerpts only, and Bungie.net's article on an archive mirror):

- Microsoft Learn, "Halo 2 Guides: Dynamic Lights Best Practices":
  https://learn.microsoft.com/en-us/halo-master-chief-collection/h2/guides/guidedynamiclights
- Bungie.net, "One Final Effort", 16 April 2010, read at
  https://bnetarchive.haloman30.com/News/contenta239.html?type=topnews&cid=25650
- Oozer3993 on Medium, "An Oral History of the Halo 2 E3 2003 Demo" and
  "Assembling a Cathedral out of a Hurricane".
- GameWatcher, "Bungie 'literally scrapped' Halo 2's E3 engine", 4 December
  2010, quoting Butcher's Eurogamer interview.
- The Cutting Room Floor, "Proto:Halo 2/May 3, 2003 Builds and Tools",
  "Proto:Halo 2/E3 2003 Demo", "Prerelease:Halo 2".
- Video Games Chronicle on the December 2024 leak and its link to the Digsite
  project.
- Beyond3D forum, 2003-04 threads "Halo 2 Gameplay Video", "New Halo 2
  screenshot" and "halo 2...multiple shadows cast".
- Steam Workshop, "Halo 2 Restored Lighting" comments.
- Bungie, "Halo 2: Full E3 2003 Trailer", Internet Archive
  (`halo2_e3_realtimedemo_hires`).
- F. C. Crow, "Shadow Algorithms for Computer Graphics", SIGGRAPH 1977;
  T. Heidmann, "Real Shadows, Real Time", IRIS Universe 18, 1991;
  L. Williams, "Casting Curved Shadows on Curved Surfaces", SIGGRAPH 1978.
- C. Everitt and M. J. Kilgard, "Practical and Robust Stenciled Shadow
  Volumes for Hardware-Accelerated Rendering", NVIDIA, 2002:
  https://arxiv.org/abs/cs/0301002; W. Bilodeau and M. Songy, US patent
  6,384,822; C. Everitt, A. Rege and C. Cebenoyan, "Hardware Shadow Mapping",
  NVIDIA, 2001-02; M. J. Kilgard, "A Practical and Robust Bump-mapping
  Technique for Today's GPUs", GDC 2000.
- H. Chen and X. Liu, "Lighting and Material of Halo 3", SIGGRAPH 2008 course
  notes, read as search excerpts.
- grimdoomer, Halo-2-HD: https://github.com/grimdoomer/Halo-2-HD
