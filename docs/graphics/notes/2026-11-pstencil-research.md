# pstencil research record: library, recovery, leaks and sources

Evidence for [pstencil-history.md](../pstencil-history.md). Compiled November 2026 at commit f7f5604cd from public sources only (listed at the end), by reading the cited pages, articles and posts. Reproduce by opening the cited URL; wiki and forum wording is read through search excerpts where the entry says so. Leaked archives are covered through news and wiki documentation only; nothing here links to them.

## 2026-11: where to see the look

The look survives in a surprising amount of publicly viewable material. Roughly in fidelity order:

**Rendered with the system (Bungie's own output):**

- **X02 announcement trailer, August 2002.** The trailer itself is on YouTube (official and mirrored
  uploads); Halopedia's `pstencil` page hosts a gallery of X02 screenshots expressly captioned "with
  pstencil lighting" - Chief on Cairo Station, Earth, the helmet close-up, the Mjolnir mark, BR55 shots
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
  lamps' real-time shadows around the 4:48 mark of the demo footage - Bungie's own narration of the
  feature.
- **The Making of Halo 2 documentary** (LCE DVD; an archive.org copy circulates): includes era alpha
  footage and the documentary record of Bungie fighting the demo's performance (the Pelican shadow-volume
  complaint).

**Official reconstructions (modern engine, original assets):**

- **The Digsite-restored E3 demo** - free Steam Workshop mod for MCC Halo 2 (Nov 9, 2024): the mission
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
  (resetera.com/threads/923508) - long-running community comparison thread with side-by-side screenshots.
- **Reddit r/halo threads** on the cut (e.g. comments/`bakk4g`, comments/`v28e29`) and the 2024–25 leak
  discussion.
- **X/Twitter:** @CustomRider05's pre-alpha Blood Gulch clip post (`status/2099924536712216640`);
  @XFerginatorX's *Fixing Halo 2's Lighting* Unreal Engine 5 experiment (`status/2107744483312103453`) , 
  the look rebuilt in a modern engine. Lehto's and Walpole's tweets on the cut (see §5).
- **zone archives** (Instagram `p/DeSK_JTlvuS`): side-by-side of the alpha lighting vs retail.
- **halo.bungie.org** - the long-running Bungie community site's news coverage, including the June 2022
  "Halo modders bring back stencil shadows to Halo 2" item and its mirror of the Freidin interview.

## 2026-11: the systems the cut touched

The cut was not contained in a renderer file - the lighting model reached across half the engine, which is
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
- **The tag system.** The demo-era tag format predates retail's variant system - every NPC was its own tag
  set (`marine.biped`, `marine_odst.biped`…) rather than variants of one. Lights carry per-light shadow
  flags; the lit Ghost exists as a separate `ghost_lit.vehicle` the demo build swaps in. Digsite had to
  modernize all of it by hand.
- **Physics.** The E3 demo predates Havok integration entirely (vehicles use CE-era speed calculations;
  enemies "fall down" instead of ragdolling). The dynamic-lighting engine and the physics engine that
  shipped in retail Halo 2 never coexisted in one build.
- **Cinematics.** Cutscene lighting was a hand-built rig (`superlights/` with ~30 named lights), using
  features retail dropped - the coloured gels of Johnson's briefing among them.
- **Game design.** Griesemer's "one or two characters on-screen" constraint was a design constraint, not
  just a rendering one - the sandbox Halo 2 shipped with (squad battles, vehicle mayhem) is the direct
  beneficiary of the choice to cut.
- **Post-processing.** The demo's different bloom ("soft glow" across the whole mission) and fluorescent
  palette belong to the same era pipeline.
- **MCC, twenty years on.** The port that finally carried the restored demo still culls dynamic lights
  aggressively ("the maximum amount of lights that can ever display at one time" - Digsite's Neo Te Aika),
  which is why the restoration reads darker than the 2003 film.

## 2026-11: the recovery, 2022 to 2024

Between 2022 and 2024, **343 Industries and the Digsite community modding crew** recovered and rebuilt the
E3 demo mission - internally named **`earthcity`** (`earthcity_e3`) - for release inside Halo: The Master
Chief Collection on PC.

How the dig began is documented in Halo Waypoint's three-part *Cutting Room Floor* series (Kenneth Peters
with Alex Wakeford, July 2022): a methodical recovery of 343's internal archives (the "\share\Halo1"
folder, "devoid of light, bereft of hope, swarming with corruption that even the Flood recoil from in
fear") starting in 2021, run by Peters with Publishing's Sean Cooper ("kornman") and help from Marcus
Lehto, then handed to an NDA'd crew of community modders - General_101, Conscars, Num0005, Scruffy,
Sean T, Ludus, Zeddikins. Halo: CE prerelease content became **Digsite Alpha**; Halo 2 development content
became **Digsite Delta**. Of the Halo 2 archive specifically: "The /share/halo2 archive was in good shape,
albeit missing many work-in-progress files from the earliest development days."

- An early restoration was previewed at the **VKMT community modding showcase, 2022**.
- The finished demo was unveiled at the **Halo World Championship 2024** (where the original demo was first
  replayed live on an actual Xbox devkit), and released as a free Steam Workshop mod on **November 9, 2024**
  - Halo 2's 20th anniversary. A companion recovered mission, **Alphamoon**, followed on November 15.
- Senior Franchise Writer **Kenneth Peters**, on working with the demo's 20-year-old source: *"We have seen
  things… things a programmer wouldn't believe…"* - his caption for "well over 3400 lines of barely
  documented haloscript originally written under an insane time crunch almost two decades ago," which the
  modern compiler no longer fully accepts.
- The mission had to be **built and rebuilt several times** because the early-development assets and scripts
  had changed or lost compatibility with final Halo 2 - Halopedia specifically cites "the pstencil lighting
  engine" among the incompatibilities.
- General_101's account (Waypoint) of the archaeology: the team started from "previous analysis of leaked
  builds and the like" plus a few recovered source files; kornman and Peters dove into the Halo 2 source
  depot for the demo's animation sources; character tags were reworked from the pre-variant format; the
  coloured gels of Johnson's briefing had no retail equivalent to map to. "This may be a shocker for some,
  but several parts of the demo gameplay were smoke and mirrors!"
- Where possible the team reused original tags, audio, and assets (even hand-rebuilding lip-sync frame by
  frame, since Bungie's lip-sync software license couldn't be reused), and added nav markers because the
  demo's scripted path was never self-evident to a player.
- The public Digsite repository (`digsite/H2E3` on GitHub) documents the release. Its README's own words on
  the lighting: if you want "the improved dynamic lights… grab the shader used for the **'not pstencil'
  lights on E3**" - the tagset's lights are, by the crew's own naming, the *not-pstencil* ones, and Sev's
  community shader is what stands in for the engine feature MCC's Halo 2 doesn't have. The same README tells
  you to swap out the **`ghost_lit`** Ghost vehicle tag - the lit variant "spams debug.txt writes in
  tag_test during the final cutscene entirely killing performance," a fossil of dev-era debug behaviour
  still attached to the lit tag.
- The restoration has known limits the crew discussed openly in the Workshop threads: gunshot/explosion
  lights "do all exist, but are aggressively culled due to the maximum amount of lights that can ever
  display at one time" (Digsite's Neo Te Aika) - which is why MCC footage reads darker than the 2003 film
  in side-by-side comparisons.

Sev's shader suite (public on GitHub, built for H2EK) offers two shadow-filtering modes whose descriptions
read as a small tribute: `1tap/Combined_0` - "Very Sharp, Aliasing-free, **Stencil-like** shadowing, more
dramatic shadows and stylized lighting" - and `3tap/Combined_1`, softer. The E3 variants are "specifically
for the E3 2003 Tagset... the files are flipped for E3." Building on this, the community's **Halo 2:
Restored Lighting** mod (Nexus Mods / Steam Workshop) re-enables per-pixel dynamic lighting across the
retail campaign, explicitly framed as restoring "what was supposed to be the game's original per-pixel
lighting... before the Pstencil engine was scrapped."

Halo Waypoint's release article promised "more behind the scenes information about the E3 2003 demo and
other recovered content in a future Digsite blog." The official Digsite blog corpus so far comprises the
2022 three-part *Cutting Room Floor* series and the 2023 two-part *Digsite Deliveries* series; as of
November 2026, the promised E3-demo-specific deep dive has not surfaced in searches.

## 2026-11: the leaks, 2024 to 2025

What Digsite worked from was Halo Studios'/343's internal archive. Parts of it are now public - not by
Microsoft's choice:

- **December 25, 2024:** the Digsite team's repository (~90 GB) leaked via 4chan - early Halo: CE
  development builds, debug builds of Halo 2 and MCC, prerelease content across the trilogy and Reach, plus
  tools and internal documents. Wikipedia (citing VGC) records that a former Digsite member denied the team
  was responsible, but also that the team had dissolved shortly beforehand, after Microsoft refused their
  request to be paid and asked them to continue as volunteers. The Cutting Room Floor, which documents
  material from this leak, notes that every file's timestamp was overwritten to 1/1/2000 00:00 GMT, so
  internal build timelines are hard to reconstruct. Community footage from this material includes a
  pre-alpha Blood Gulch running pstencil lighting on original Xbox hardware.
- **December 26, 2025:** the one that matters to this document. Two Halo 2 prototypes, verified by Hidden
  Palace: a **May 3, 2003 pre-E3 build containing the pstencil stencil-shadow rendering system itself, plus
  the era's actual editing tools (`blam.exe` and `sapien.exe`)** - and the **May 11, 2003 build with the
  complete E3 demo as a runnable .xbe**.

The upshot for this topic: pstencil is no longer known only through trailers, demos, and interviews. The
stencil-shadow system's own build and the tools Bungie used to author its lighting are in circulation,
running on original hardware and emulators (xemu) in community hands. The May 3 build is the *same* build
whose heavier shadows TCRF compared against the E3 build - so the raw material for a definitive technical
account of pstencil (its passes, its tags, its tuning) now exists outside Microsoft.

No such authoritative
analysis has been published yet; what exists so far is community exploration. And to be explicit about
provenance: these were unauthorized leaks, not Digsite releases - this document links only to news coverage
and wiki documentation of them, never to the archives.

One closing observation on feasibility: the reason pstencil died was 2003 Xbox fill rate - a constraint
modern hardware does not share. That is why Sev's shaders and the Restored Lighting mod can approximate the
look within the retail engine today, and why the system in the leaked builds could, in principle, be run
whole on current PCs. The cut was a hardware decision, and the hardware moved on.

## 2026-11: sources

**Primary - Bungie and Microsoft people, on the record:**

- VICE - *The Complete, Untold History of Halo* (Steve Haske, May 2017; three-part oral history - the
  Griesemer, O'Donnell, and Bertone quotes verbatim):
  https://www.vice.com/en/article/the-complete-untold-history-of-halo-an-oral-history/
- xbox.com - *Master Chief Bernie Freidin, Bungie Software Design Engineer* (interview by Violet Leigh,
  mirrored by halo.bungie.org; Freidin's own description of the pstencil research project):
  https://halo.bungie.org/misc/xbox.bfreidin/
- Bungie.net - *One Final Effort* (Adrian Perez; directional-light/lightmap-ambient design, the 30 FPS and
  Blood Gulch claims - cited and archived via Halopedia's pstencil page).
- Bungie.net - *Weekly What's Update: Massive Attack!* ("the E3 demo… still works and is playable even, but
  is waaaaaaaay too buggy as it stands to ever be released" - cited via Halopedia's E3 demo page).
- Eurogamer - *Halo 2 E3 demo was "smoke and mirrors"* (2010; Bungie's admission, cited via Wikipedia's
  *Earth City demo* article): https://www.eurogamer.net/halo-2-e3-demo-was-smoke-and-mirrors
- Marcus Lehto, Twitter `@game_fabricator`, Jan 5 2020 ("the now infamous stencil shadow version of our
  engine… We ditched that feature after the H2 announce trailer was done") - via Halopedia.
- Nate Walpole, Twitter `@bentllama`, Jan 5 2020 ("We ditched a lot after the H2 announce trailer, including
  sanity. The scars, they teach.") - via Halopedia.
- Jaime Griesemer, X `@32nds`, December 2025 (Xbox 360 delay wouldn't have helped; "the shadows did not work
  at all in exterior environments") - via Halopedia.
- *Halo 2* developer commentary (LCE/MCC; Joe Staten on street-lamp shadows at ~4:48 of the demo).

**Official recovery - Halo Waypoint and the Digsite release:**

- Halo Waypoint - *Cutting Room Floor* (Part 1 of the July 2022 trilogy; Kenneth Peters' account, the
  Earth City/General_101 section, the 3,400-line script, coloured gels): 
  https://www.halowaypoint.com/news/cutting-room-floor
- Halo Waypoint - *Cutting Room Corps* (Part 2; the crew Q&A, General_101 on the E3 scripts not compiling
  in retail): https://www.halowaypoint.com/news/cutting-room-corps
- Halo Waypoint - *Digsite Deliveries* (July 2023): https://www.halowaypoint.com/news/digsite-deliveries
- Halo Waypoint - *Digsite Demos: Halo 2* (Nov 2024 release announcement):
  https://www.halowaypoint.com/news/digsite-demos-halo-2
- Halo World Championship 2024 - Community Stage Day 2 (original demo replayed; remaster unveiled; Jarrard
  quotes from Halopedia's coverage; 4K upscale on YouTube `G4mbFRduYvA`).
- The Moa Archive / UncookedMoa - *Halo 2 E3 2003 Demo Stream from 343 + Never Before Seen Alphas/Betas*
  (1080p60 capture of the Nov 14, 2018 stream - Halopedia's primary source for Jarrard's and the stream's
  quotes).
- Steam Workshop - (H2) E3 2003 Demo, release + discussions (Digsite crew Q&A: Sean T and SEV on pstencil
  not being reimplemented and "already cut by the E3 demo"; Neo Te Aika on light culling):
  https://steamcommunity.com/workshop/filedetails/discussion/3360515088/4633735012740577154/
- digsite/H2E3 - the released E3 demo repository (tagset, `ghost_lit`, Sev shader pointer, team credits):
  https://github.com/digsite/H2E3
- digsite/h2 - Halo 2 Digsite repository for the MCC editing kits (retail-kit tag set incl. the
  `stencil_*` and `lightaccum_*` vertex shaders): https://github.com/digsite/h2

**Wikis, press, and leak coverage:**

- Halopedia - *pstencil* (the One Final Effort citations, Lehto/Walpole/Griesemer posts, X02 and Blood
  Gulch galleries): https://www.halopedia.org/Pstencil
- Halopedia - *Halo 2 E3 Demo* (Jarrard's "unicorn" quote, the 2018 stream, the differences list - bloom,
  sharp sun shadows, street-lamp flags, Havok, fluorescent palette - the Digsite remaster section):
  https://www.halopedia.org/Halo_2_E3_demo
- The Cutting Room Floor - *Proto:Halo 2/E3 2003 Demo* (build dates, May 3 vs E3 comparison, demo boot
  configuration): https://tcrf.net/Proto:Halo_2/E3_2003_Demo
- The Cutting Room Floor - *Category:Halo Digsite Repository leaks* (what the Dec 2024/2025 leaks
  contained; the zeroed filedates): https://tcrf.net/Category:Halo_Digsite_Repository_leaks
- Wikipedia - *Earth City demo* (leak provenance; the 2010 "fake" admission citing Eurogamer; Digsite team
  dissolution context citing VGC): https://en.wikipedia.org/wiki/Earth_City_demo
- VICE - *Halo 2's Legendary E3 Demo Has Leaked After 22 Years* (Denny Connolly, Dec 29 2025):
  https://www.vice.com/en/article/halo-2s-legendary-e3-demo-has-leaked-after-22-years/
- Clawsome Gamer - *Halo 2 E3 2003 Demo Build Leaks* (Jake Bishop, Dec 28 2025; the May 3 build's pstencil
  system and blam/sapien tools, as cited by VICE):
  https://www.clawsomegamer.com/halo-2-e3-2003-demo-build-leaks-playable-earth-city/
- halo.bungie.org news, June 2 2022 - *Halo modders bring back stencil shadows to Halo 2* (the restoration
  effort's first public nod): https://halo.bungie.org/news.html?item=42711

**Community reconstructions and style references:**

- Sev - Halo 2 custom shaders for H2EK (shadow-filter modes incl. "stencil-like" 1tap; E3 tagset variants):
  https://github.com/777Sev777/Halo-2-Custom-Shaders
- Halo 2: Restored Lighting mod (community per-pixel lighting restoration built on Sev's shaders; video
  `FK5T_2NXjhI`): https://www.nexusmods.com/halothemasterchiefcollection/mods/2603 and
  https://steamcommunity.com/sharedfiles/filedetails/?id=3743458113
- The Moa Archive - pre-alpha/pstencil-era footage (YouTube `hjY1WnzvNvg`, `bIU_Y6wAubU`).
- *The Making of Halo 2* documentary (Limited Collector's Edition DVD; archive.org copy circulates at
  `archive.org/details/Halo_2_Making_Of_Video`).
- ResetEra - *What could have been: if Halo 2 shipped with stencil dynamic shadows* (community comparison
  thread): https://www.resetera.com/threads/923508/
- Reddit r/halo cut-lighting discussions (e.g. `reddit.com/r/halo/comments/bakk4g/`, `…/v28e29/`).
- X/Twitter - @CustomRider05 pre-alpha Blood Gulch clip (`x.com/CustomRider05/status/2099924536712216640`);
  @XFerginatorX *Fixing Halo 2's Lighting* UE5 experiment
  (`x.com/XFerginatorX/status/2107744483312103453`).
- zone archives - alpha-vs-retail lighting comparison (Instagram `instagram.com/p/DeSK_JTlvuS`).

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
- C. Everitt and M. J. Kilgard, "Practical and ... Stenciled Shadow
  Volumes for Hardware-Accelerated Rendering", NVIDIA, 2002:
  https://arxiv.org/abs/cs/0301002; W. Bilodeau and M. Songy, US patent
  6,384,822; C. Everitt, A. Rege and C. Cebenoyan, "Hardware Shadow Mapping",
  NVIDIA, 2001-02; M. J. Kilgard, "A Practical ... Bump-mapping
  Technique for Today's GPUs", GDC 2000.
- H. Chen and X. Liu, "Lighting and Material of Halo 3", SIGGRAPH 2008 course
  notes, read as search excerpts.
- grimdoomer, Halo-2-HD: https://github.com/grimdoomer/Halo-2-HD
