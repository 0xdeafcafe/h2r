# Known Halo 2 superbounce spots from the community, against our scan

Evidence for [superbounce.md](../superbounce.md), "Spots in the geometry". Nothing here states what the code does; the design page does.

## 2026-11-21: the public record and what our scan finds

Read 2026-11-21 at `32a0f64e8` (the `sb4-scan` worktree) (built `h2`, offline, no game run). Web pages were read through a search tool and a page fetcher; Giant Bomb, StrategyWiki, the Fandom wikis and GameFAQs refused the fetcher (HTTP 402 or 403), so those are known only from search-result summaries and are marked so. No source derived from leaked Bungie builds was used. To reproduce the scan side:

```sh
h2 superbounce scan <map> --json     # per map: stand points, kind, score
h2 maps                              # the multiplayer maps on the disc
```

### What the sources give

The public record is thin: a handful of written guides and some videos with no text. Almost nothing carries coordinates, only landmarks.

- Halopedia, "Super bouncing": the only written per-map guide, 9 maps with steps. <https://www.halopedia.org/Super_Bouncing>
- Halo Fandom, "Super bouncing": same content as Halopedia, via summary only. <https://halo.fandom.com/wiki/Super_bouncing>
- Giant Bomb, "Super Bouncing": names the best-known maps (Lockout, Ascension, Headlong, Turf, Sanctuary, Warlock, Zanzibar, Burial Mounds); summary only. <https://www.giantbomb.com/super-bouncing/3015-401/>
- StrategyWiki, "Halo 2/Glitches": claims a super jump on every map and names Zanzibar bounces; summary only. <https://strategywiki.org/wiki/Halo_2/Glitches>
- Halo Stunting wiki, "Super Bounce": technique page, not fetched (HTTP 402). <https://halostunting.fandom.com/wiki/Super_Bounce>
- GameFAQs guide by antseezee: lists a Headlong bounce (top of the stairs, at the corner, crouching), a Colossus "Conveyor Belt Bounce" and a Midship "Bouncey Bounce", credited to Dutch Hayabusa; seen as search snippets only, the page refused the fetcher. <https://gamefaqs.gamespot.com/xbox/562116-halo-2/faqs/36745>
- Lumnix, "Halo 2's Super Bounce: How a Bug Became Legend": technique prose, no spots, names Lockout and Zanzibar as the spectacular ones. <https://lumnix.fr/en/article/super-bounce-halo-2-bug-legende-communaute>
- Videos found by title only (nothing read from them): "Halo 2 Tricks: Episode 40 *Headlong Super Bounce*" <https://www.youtube.com/watch?v=d0iq51bsRIw>; "HALO 2 - Midship Super Bounce" <https://www.youtube.com/watch?v=gG5SNqU_KJg>; Burial Mounds: "Outside the Map Super Bounce" <https://www.youtube.com/watch?v=zeSyFgw-VxA>, "Wall Bounce" <https://www.youtube.com/watch?v=vEgg5eEPzt0>, "Pizza Cutter Bounce" <https://www.youtube.com/watch?v=UnpcEsH3Dn4>, "Super Bounce and Stairway to Heaven" <https://www.youtube.com/watch?v=y2jYkBFoNRk>.

### The spots

Version: every written source is about the original Xbox. Halopedia says it was removed from the PC version (Vista) and works in MCC Classic mode "but not in the same spots as the original" (from a search summary of the Halopedia/Fandom text). A ResetEra poster says it is much harder in MCC. Bungie is said to have fixed some spots with updates (Lumnix). No MCC patch note was found.

| # | Map | Landmark | How | Use |
|---|---|---|---|---|
| 1 | Ascension | Path fork to the large tower: a small area sticking out from the wall | Crouch in, release, keep moving forward; turn, jump between the large and small rock behind; turn left, build speed, jump onto the line where the ramp meets the ground; land vertically, no buttons | Tower |
| 2 | Ascension | Other base: right side of the tower railing, left canopy | Crouch under the canopy, move forward, release, keep moving; jump, release forward in the air; face the second-closest strut (sniper platform to slanted walkway), jump onto its top | Strut/top |
| 3 | Burial Mounds | Corner of two rocks near the big metal structure | Two players: a Warthog at full speed rams the standing player head-on until launched; steer to a high platform. A vehicle pressure launch, not a seam bounce | High platform |
| 4 to 7 | Burial Mounds | Four videos (outside the map, wall, pizza cutter, stairway to heaven) | Not read; locations unknown | Out of map, platforms |
| 8 | Coagulation | Either base, back ramp, small hole on the left | Jump, crouch in the air to enter; crouch, walk forward, stand after a few seconds, jump out; release the stick just before landing | Vantage (treetops) |
| 9 | Lockout | BR tower: walkway left of the upper centre floor, small gap left of the door | Crouch in, release, walk out; face map centre, jump right onto the ledge, run, jump at the zigzag toward the lower corner ledge | Highest point; can kill |
| 10 | Lockout | Sniper tower: lifted hatch door between levels one and two | Crouch under its right side, stand, walk forward only (no strafing) to the sniper spawn; walk off the ledge, turn, jump onto the raised ledge right of the walkway; jump at the catwalk where the angle changes, a foot from the doorway | Sniper tower or lift beam |
| 11 | Sanctuary | Blue base, rock near the water | Crouch into the rock; crouched, walk up the ramp to the centre structure; at the turn jump onto the seam between the dirt and brick path | Centre structure |
| 12 | Turf | Medical tent ledge with two barriers; six dot designs on the tent | Jump, crouch, uncrouch just before landing on the dots | Invisible ceiling, out of map |
| 13 | Warlock | Blue base: under the root | Duck under it, walk left until free, forward without strafing until the gap between the air chute and the small wall; jump | Base-side |
| 14 to 17 | Warlock | One per base (red, blue, green, yellow), under the ramp facing the red side of the middle tower | Crouch, release, walk up, jump onto the central tower; turn to the ramp, jump, tap forward; landing zone differs by base | Central tower |
| 18 | Zanzibar | Defensive base, second level, window nearest the beach | Break the window, crouch under the one shard left, jump and hold forward when the screen shakes. NTSC/PAL60 only. Located 2026-11-21 by play (lab job 0043): the sill at (5.22, −26.285, 5.11) | Base roof |
| 19 | Zanzibar | Outside the inside-openable gate: rock against rock wall | Crouch there, run over the rocks, land on the stained cement | Ledge |
| 20 | Zanzibar | Central fan, right side, angled concrete | Crouch into it, back up, jump onto the corner of the lower slanted concrete | Fan top |
| 21 | Zanzibar | Smaller beach staircase railing, rock at the top | Crouch beneath the rock, walk out, run up to the divot in the rail, jump over it, land on the highest part | Cliff by the sniper spawn |
| 22 | Headlong | Top of the stairs, at the corner (GameFAQs snippet), and the Episode 40 video | Crouch at the corner; the rest unknown | Unknown |
| 23 | Midship | "Bouncey Bounce" (GameFAQs) and a video | Unknown | Unknown |
| 24 | Colossus | "Conveyor Belt Bounce" (GameFAQs) | Unknown | Unknown |

Also named without detail: Zanzibar "bounce one is reached from inside the base" and "another gets you on the wheel" (StrategyWiki); a variant called monster bounces (a thin seam over many grid lines, or a V of converging lines) and pressure launches (Halopedia).

### Against our scan

The scan runs per BSP 0 and prints its best 30 or so per map (the counts below are what it printed). None of the maps' landmarks is a coordinate, so every mapping below is a guess about a landmark that nobody has stood on, not a measurement. Distances are not given because none could be honestly computed; the only spot with a trusted position is the one we measured ourselves.

| Map | Known spots (distinct) | Scan output (bounce + crouch) | Found by our scan |
|---|---|---|---|
| Ascension | 2 | 38 (26 + 12) | Not matched. Our proven corner, crouch index 26 at (−10.9, 0.7, −1.2), is probably spot 1's "small area sticking out" at the tower-path fork, but the landmark was never checked in game |
| Lockout | 2 | 26 (16 + 10) | Not matched; the scan's lowest points sit near z −13.9 to −13.6, the tower tops are not told apart |
| Zanzibar | 4 | 41 (29 + 12) | Spot 18 located by play (lab job 0043, recording `zanzibar-window`; [superbounce-spots.md](superbounce-spots.md#2026-11-21-zanzibars-window-played)): the prime at (5.22, −26.285, 5.11) under the broken glass, which the scan cannot see (an object), and the landing on a single crease at y −26.70 that the slot rule rejected. Spots 19 to 21 not matched; many symmetric bounce points near x ±1.1, y −9.7 to −14, z 2.1 remain unverified candidates |
| Burial Mounds | 1 (+4 videos) | 22 (10 + 12) | Spot 3 is a vehicle push and is out of the scan's model; the videos are unlocated |
| Coagulation | 1 | 15 | Not matched; our earlier note found its crouch spots all more than 8 units from the seam strips |
| Headlong | 1 | 40 (29 + 11) | Unlocated |
| Midship | 1 | 54 (42 + 12) | Unlocated |
| Colossus | 1 | 43 (31 + 12) | Unlocated |
| Sanctuary, Turf, Warlock | 1, 1, 5 | not on the disc | Not scannable here (`h2 maps` lists 12 multiplayer maps; these three are not among them) |
| Beaver Creek, Waterworks, Cyclotron, Foundation | 0 found | 36, 65, 47, 84 | No community spot found |

So: of 24 documented spots, we can confidently say 0 are found, 0 missing, and 3 are not a seam bounce or not scannable. This is a research limit, not a scanner result. The next step that would settle it: stand at each landmark through the agent API (`session.methods` lists the position and teleport methods) and run `h2 superbounce scan <map> --near x,y,z,r` for each, which turns "unlocated" into "found by index N" or "missing".

### Technique facts, against superbounce.md

- **The recipe matches.** Every source says: be forced to stay crouched under a low ceiling or in a small recess, release crouch, walk out, jump onto a seam, land. Halopedia: "keep moving forward" after release; the player stays crouched until out of the area. Our "crouch, walk in, let go, walk out" is the same.
- **A failed bounce can be retried without re-priming, unless you crouch again** (Halopedia). Ours: the pair word stays at −3 until a crouch or sleep rebuilds it at 0. Consistent, and the community's "crouching again loses it" is our "a crouch takes it away at once".
- **Standing still.** Our model says a still body sleeps after 38 to 40 ticks and loses the effect. No source says this; the guides say "keep moving", which fits but is not evidence. No contradiction.
- **Crouch in the air.** Coagulation (jump, crouch mid-air to enter) and Turf (jump, crouch, uncrouch just before landing) use crouch changes mid-air. Our docs cover the crouch height term (a mid-air biped moves by its height change) and say it was zero on every recorded bounce, which were all standing. These two guides are a case where the term would matter, so the claim "neither was involved" holds for our traces, not for the community's spots.
- **Landing posture.** "Land vertically with no buttons held" (Ascension 1) and "release forward in mid-air" (Ascension 2) say the landing velocity matters; the Lockout 2 guide says walk forward only, no strafing. Our charge and landing sections already treat approach speed; this is the same fact, from the player's side.
- **Frame rate.** Halopedia and a wiki summary say NTSC and PAL60 give different bounces from PAL50 because collision checks run per frame, and one Zanzibar bounce works only on NTSC/PAL60. Our recomp runs the engine at the retail tick; this is a reason a PAL-only video may not reproduce.
- **Failure.** Landing somewhere unreachable, falling through the map and dying to the guardians, or dying on landing counted as a suicide. Not in our docs.
- **Theories.** Community: the engine pushes you out of the level because you are embedded, or because you start to fall out and physics forces you back up. Our trace (two facing faces, opposing impulses leave the upward part) agrees with the first.
- **Monster bounces and pressure launches** are a separate class, with a stronger launch. The scan has no model for either.
