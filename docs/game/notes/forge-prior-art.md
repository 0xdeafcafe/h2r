# Forge prior art: Halo 3, Halo: Reach and Halo Infinite

Evidence for [forge-editing.md](../forge-editing.md), [forge-design.md](../forge-design.md) and [forge-firefight-ui.md](../forge-firefight-ui.md).

## 2026-11-21: what the three retail Forges do

Read at commit `7e82ceaac`. Method: the public pages listed under
[Sources](#sources), fetched on this date and read for the facts below.
To reproduce, open each URL beside the fact that cites it. Every fact
carries the key of the page that states it. **Unconfirmed** marks a
point no fetched page states; it is common knowledge of the retail
games at best, and a design page that leans on one says so.

Not consulted: leaked or pre-release builds, projects built from them,
community decompilations, and map-variant source code from any project.
A forum thread compiled from a pre-release Reach build appeared in
search and was not opened. The archived Bungie.net Forge how-tos and
the Halo Waypoint support site could not be fetched; facts known only
from a search snippet of the support site are marked **snippet**.

### Halo 3 (2007)

**The monitor and its buttons** (retail manual [h3-manual] p. 5 and
p. 26, [h3-controls]).

| Button | Edit Mode |
|---|---|
| D-pad Up | Player Mode or Edit Mode |
| Left stick, right stick | move, look; with RT held they move and rotate the held item |
| RB, LB | move up, move down |
| LT | thrust |
| RT (hold) | rotate the selected item; with the move stick it pushes or pulls the item |
| A | pick up or drop |
| B (hold) | drop the selected item |
| X | inventory, or the options of the item aimed at |
| Y | delete |
| Start | the Forge controls and options, with Save Map |

- X on a placed item edits it; X again shows the settings for every
  object of that type ([h3-forge]).
- The HUD's budget read-out and the reticle's change over an object:
  **unconfirmed**.

**The object menu.** Halopedia files Halo 3's objects under Weapons,
Equipment, Vehicles, Teleporters, Spawners, Goals and Scenery
([h3-objects]). That these are the in-game tab names and their order:
**unconfirmed**. Each object has a cost and a maximum per map
([h3-objects], [h3-weapons]). Not every object is on every map
([h3-manual] p. 26).

**Budget.** Each map has a budget; deleting a default object returns
its credits ([h3-manual] p. 26). It is a counter, not a currency
([h3-forge]). Every map also has a 640 item maximum, which deleting
default items does not free ([h3-forge]). An item's price can change
with its properties ([h3-objects]).

**Item properties.** Per object: Respawn Rate and Place at Start. Per
object type: Run-Time Minimum and Run-Time Maximum, which hold the
count on the map above or below a number ([h3-forge]). Teleporters are
a Sender Node, a Receiver Node and a Two-Way Node, joined by Channel
([h3-teleporters]). Spare Clips, Team, Symmetry and the shape of a
spawn area: **unconfirmed** in a fetched page ([forge.md](../forge.md)
1.1 has them from the map-variant file's fields).

**Spawns and goals.** Respawn Point, Starting Point (round start only)
and Respawn Area are objects with no cost ([h3-spawners]). Goal objects
are per game type: flag spawn and return points, bomb spawn and plant
points, ball spawns, hill markers, territory markers, go-to points
([h3-goals]). Many of them appear in the item menu only after that game
type is picked with "Change game type" in Forge's pause menu
([h3-spawners], [h3-goals]).

**Forge as a lobby.** The game has five lobbies: Campaign, Matchmaking,
Custom Games, Forge and Theater. Each has Switch Lobby and a Network
choice of Xbox LIVE, System Link or Local ([h3-manual] p. 18). Up to
eight players edit together over System Link or Xbox LIVE, and up to
four in split screen ([h3-manual] p. 26). The party leader's Forge
options include one that keeps everyone but the leader out of Edit Mode
([h3-forge]); the option's label: **unconfirmed**. Save Map is on
Start; the saved variant plays in the Custom Games lobby and can go on
the File Share ([h3-manual] p. 24, p. 26). A Forge session is saved as
a film ([h3-manual] p. 25).

### Halo: Reach (2010)

**Buttons** ([reach-controls], retail manual [reach-manual] p. 23). As
Halo 3, with these differences: B is Tools, X is Object Options, and
clicking the right stick zooms the view. RT held is "Manipulate
object": with the move stick it pushes the object away or pulls it in,
and with the sticks it rotates it.

**Precision** ([reach-forge], [reach-vidoc]).

- Rotation snaps in steps; Halopedia lists 1, 5, 10, 15, 30, 45 and 90
  degrees. The in-game option list: **unconfirmed**.
- Players can nudge a piece and edit its coordinates in world space.
  The screen's fields and its steps: **unconfirmed**.
- A fine editing mode exists (the ViDoc's words). Its button:
  **unconfirmed**.

**Physics** ([reach-forge], [reach-vidoc]). Normal: the object falls
and collides, as in Halo 3. Fixed: it stays where it is released, in
the air too. Phased: as Fixed, and it may pass through other objects
and the map.

**Labels** ([reach-labels]). A label can be put on most objects. It
names the object's part in a game type: a flag's return point, a hill,
a territory, an infection spawn, a race checkpoint. A label with
"Gametype-Specific" set limits the object to that game type. Spawn
Sequence orders hills, checkpoints and phases. A game type's labels are
offered only when that game type, not Basic Editing, is the one chosen
in the Forge lobby. Some structures and vehicles take a team colour
([reach-forge]).

**The object menu** ([reach-objects], [reach-structure],
[reach-gadgets]). Halopedia's categories: Weapons (human and Covenant),
Armor Abilities, Vehicles, Gadgets, Spawning, Objectives, Scenery and
Structure. Structure has sub-palettes (Building Blocks, Bridges and
Platforms, Buildings, Decorative, Doors, Windows and Walls, Inclines,
Natural). Gadgets holds teleporters, man cannons, shield doors, health
stations, explosives, power-ups, screen effects, toys and coloured
lights. A maximum is per category, not per item ([reach-forge]). The
exact menu strings: **unconfirmed**.

**Forge World** ([reach-world], [reach-manual] p. 23). A blank canvas
of several joined regions with a budget of 10,000 and the full
Structure palette: the map is made of palette pieces, not edited
level geometry.

**Spawning objects** ([reach-spawning]). Initial Spawn, Respawn Point,
three kinds of Respawn Zone, Initial Loadout Camera, and Safe and Kill
boundaries, each with a soft form. What each zone and boundary does:
**unconfirmed**.

**Lobby** ([reach-manual] p. 17, p. 23, p. 25). Six lobbies:
Matchmaking, Firefight, Custom Game, Campaign, Theater and Forge.
Network is chosen in any lobby: Xbox LIVE, System Link or Local. Eight
players over a network, four in split screen.

### Halo Infinite (2022 onward)

| Feature | What retail has | Source |
|---|---|---|
| Transforms | move, rotate and scale widgets with single-axis handles; Scale X, Y and Z in Object Properties (**snippet**). Typed position and rotation: **unconfirmed** | [inf-cu29], [inf-forge-hp], [inf-s4-notes] |
| Snapping | a Movement Snap setting (0.001 is one of its values), a Rotation Snap setting, and magnets that join objects at marked points | [inf-s3-hp], [inf-forge] |
| Axes and pivots | a prefab has a Centre of Mass pivot (**snippet**). A local or world axis toggle and an editable pivot: **unconfirmed** | [inf-s5-notes] |
| Groups and prefabs | several objects become a prefab with a parent object; a prefab can hold script brains, is published on its own and browsed in the Community tab; ungrouping breaks the tie to the saved prefab; prefabs weld to vehicles | [inf-cu29], [inf-faq], [inf-fund], [inf-s5] |
| Undo and redo | supported; in solo sessions only (**snippet**) | [inf-fund], [inf-overview] |
| Lighting | no lightmap bake; each placed light casts static shadows, dynamic shadows or both; light probes are generated (capped at 14,000 a map; minutes on the largest maps); reflection volumes; area lights, light groups, fog per light; time of day, sun, sky and fog settings | [inf-forge-hp], [inf-forge], [inf-s5], [inf-fund] |
| Node graph | a visual flowchart of events, conditions and actions held in a Script Brain object; 128 nodes and 512 connections a brain; a node budget for the map; AI spawner and game event nodes | [inf-forge], [inf-cu29], [inf-s4], [inf-s3-hp], [inf-s5] |
| Object browser | objects listed by category; a Folders tab whose eye icon hides objects (**snippet**) | [inf-cu29], [inf-aug-notes] |
| Budget | meters for simulation, object limits by kind (dynamic, static, vehicles, effects), node graph and run time; 7,000 objects; an object is static or dynamic; collision is Normal, Physics, Phased or No Collision | [inf-s4], [inf-forge-hp], [inf-s5] |
| Test mode and bots | a Test Mode; a nav mesh built for bots, rebuilt only after geometry is added; up to 32 AI units | [inf-fund], [inf-s5] |
| Files | every save is a version; a bookmark follows the latest published version | [inf-faq] |
| Co-creation | eight players; join in progress was off at the beta | [inf-faq] |

### Verdicts

What our design takes from each, and why, is in
[forge-editing.md](../forge-editing.md), "What it takes after".

## Sources

[h3-manual]: https://www.halopedia.org/images/d/da/H3_Manual.pdf
[h3-forge]: https://www.halopedia.org/H3:Forge
[h3-controls]: https://www.halopedia.org/H3:Control_schemes
[h3-objects]: https://www.halopedia.org/Forge_objects_in_Halo_3
[h3-weapons]: https://www.halopedia.org/Forge_objects_in_Halo_3/Weapons
[h3-spawners]: https://www.halopedia.org/Forge_objects_in_Halo_3/Spawners
[h3-goals]: https://www.halopedia.org/Forge_objects_in_Halo_3/Goals
[h3-teleporters]: https://www.halopedia.org/Forge_Objects_in_Halo_3/Teleporters
[reach-manual]: https://www.halopedia.org/images/2/2f/HR_Manual.pdf
[reach-controls]: https://www.halopedia.org/Reach:Control_schemes
[reach-forge]: https://www.halopedia.org/Forge/Halo:_Reach
[reach-labels]: https://www.halopedia.org/Forge_Labels
[reach-vidoc]: https://www.halopedia.org/Halo:_Reach_ViDoc:_Forge_World
[reach-world]: https://www.halopedia.org/Forge_World
[reach-objects]: https://www.halopedia.org/Forge_objects_in_Halo:_Reach
[reach-spawning]: https://www.halopedia.org/Forge_objects_in_Halo:_Reach/Spawning
[reach-gadgets]: https://www.halopedia.org/Forge_objects_in_Halo:_Reach/Gadgets
[reach-structure]: https://www.halopedia.org/Forge_objects_in_Halo:_Reach/Structure
[inf-forge]: https://www.halowaypoint.com/halo-infinite/forge
[inf-fund]: https://www.halowaypoint.com/news/halo-infinite-forge-fundamentals
[inf-faq]: https://www.halowaypoint.com/news/halo-infinite-forge-beta-faq
[inf-cu29]: https://www.halowaypoint.com/news/forge-overview-cu29
[inf-s4]: https://www.halowaypoint.com/news/forge-update-overview-season4
[inf-s5]: https://www.halowaypoint.com/news/forge-overview-season-5
[inf-forge-hp]: https://www.halopedia.org/Forge/Halo_Infinite
[inf-s3-hp]: https://www.halopedia.org/Season_03:_Echoes_Within
[inf-overview]: https://support.halowaypoint.com/hc/en-us/articles/10581874119828-Halo-Infinite-Forge-Overview
[inf-s4-notes]: https://support.halowaypoint.com/hc/en-us/articles/16471139448724-Halo-Infinite-Season-4-Infection-Patch-Notes
[inf-s5-notes]: https://support.halowaypoint.com/hc/en-us/articles/19892918820244-Halo-Infinite-Season-5-Reckoning-Patch-Notes
[inf-aug-notes]: https://support.halowaypoint.com/hc/en-us/articles/17986755787028-Halo-Infinite-Update-August-9-2023

- Halo 3 and Halo: Reach retail manuals, as hosted by Halopedia
  ([h3-manual], [reach-manual]).
- Halopedia: [h3-forge], [h3-controls], [h3-objects], [h3-weapons],
  [h3-spawners], [h3-goals], [h3-teleporters], [reach-controls],
  [reach-forge], [reach-labels], [reach-vidoc], [reach-world],
  [reach-objects], [reach-spawning], [reach-gadgets], [reach-structure],
  [inf-forge-hp], [inf-s3-hp].
- Halo Waypoint: [inf-forge], [inf-fund], [inf-faq], [inf-cu29],
  [inf-s4], [inf-s5].
- Halo Waypoint support, search snippets only: [inf-overview],
  [inf-s4-notes], [inf-s5-notes], [inf-aug-notes].
