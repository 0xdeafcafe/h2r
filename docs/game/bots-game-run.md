# Bots: game runs and testing

What needs a game run, next tracing and `h2 bots test` (sections 11 to 13). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 11. What needs a game run

Each item is for the user to play, or for a coordinator run.

1. All configured phantoms join the hosted System Link or Xbox Live
   pregame lobby. The controller sign-in screen lists human controllers.
2. A second human signs in on a free port while the phantoms are in
   the lobby. Check the human views, HUD and audio during play.
3. Rostered Live phantoms appear under their own bot accounts.
4. Bot movement on real maps: the nav mesh and stuck recovery on every
   shipped MP map, jumps and lifts included.
5. Aim and combat feel per difficulty against a human; tuning of the §3.4
   table.
6. Emote triggers and gates in real play.
7. CTF, Oddball and King of the Hill with bots (§11.2).
8. A phantom peer seen correctly by a real System Link client: lobby,
   in-game, scoreboard, carnage report. The host's CPU with 15 phantoms.
9. The Clankers page in Game Setup: layout on the real screen, and its
   values persisting.
10. Clankermatch end to end: §8.5's checklist (the glyph on every name
    surface once it is built).
11. Whether an empty search makes the game host in each playlist type, or
    `MatchProposed` with `host = me` is needed (§8.4).
12. Every glitch (§3.8): none has been seen working in a game. Its
    checklist is in §3.8.

### 11.1 The brain in play (checklist)

For the user, or a coordinator run with `h2 bots test` and its log. Two
or three Clankers on Lockout, Midship and one big map (Coagulation or
Waterworks), on Normal and Legendary. Each line is a yes or no.

Stuck (§3.2):

1. Over a few minutes no bot stands pressed against a wall, a railing or
   a corner for more than about 3 s.
2. A bot that walks into a step or low ledge sidesteps, then jumps, then
   crouch-jumps, and gets over it or turns away. It does not jump in open
   floor while walking freely.
3. A bot that cannot get somewhere goes somewhere else within about 3 s
   and does not come back to the same spot for about 10 s.
4. No bot walks back and forth between two points for more than about
   4 s.
5. Two bots meeting in a corridor pass each other; a bot does not push
   into a teammate standing still.

Ammunition (§3.6):

6. A bot that empties a magazine starts its reload after a short input
   delay (the reload animation plays).
7. A bot with a part-empty magazine reloads after a fight, not during it.
8. A bot whose gun runs dry with a loaded second weapon switches to it
   (the switch animation plays).
9. A bot with both guns dry walks to the nearest weapon on the map and
   takes it (another kind: it swaps; the same kind: its ammunition goes
   up). Check with Famine on, or a variant with low starting ammunition.
10. A bot with both guns dry and a human within a couple of metres runs
    in and melees instead.
11. A bot does not stop on the flag, the ball or the bomb trying to pick
    it up (they read as not guns).
12. Plasma weapons: a bot with a dead plasma rifle or pistol switches or
    goes for another weapon; it does not try to reload it.
13. The fields themselves: on the human's own unit, `h2 agent object`
    reads of the held weapon's `+0x22c` and `+0x22a` match the HUD's
    rounds counter, and `1 - (+0x184)` the plasma pistol's battery meter.

Cheap wins (§3.3):

14. A bot with its shields popped backs away from the human while
    shooting, and does not chase out of sight until they recharge.
15. Bots throw a grenade into two humans standing together more often
    than at one; never with no grenades left.
16. A bot that loses sight of someone round a corner sometimes lobs a
    grenade there.

Weapon skill ("Built: weapon skill" in §10), Heroic and Legendary, Slayer
on Lockout and Midship with a human:

17. Before throwing, a bot looks up (or down) along its throw for a
    moment, then throws; the grenade lands at or rolls to where the
    human stands, more often than it falls short or long. Note the map,
    the distance and where it landed against where you stood: the throw
    speed, gravity, bounce and fuse are assumed numbers.
18. Behind a pillar or round a corner, a Heroic or Legendary bot's frag
    sometimes comes off a wall or rolls round the corner to you. It does
    not throw into a wall in front of its face.
19. Within about 8 a bot with plasmas sometimes sticks you (it presses
    Black first when frags are selected). A bot never kills itself or,
    in a team game, its teammate with its own grenade.
20. A bot with a shotgun or SMG throws a grenade before closing in.
21. A bot with a plasma pistol and a battle rifle or magnum charges the
    pistol (its glow), fires the overcharge at you, switches and shoots
    for your head; Legendary does it nearly every fight, Easy almost
    never.
22. A bot that has hit you a few times and is at arm's length melees.
23. A bot with two short guns walks to a battle rifle nearby after a
    fight, and one with two long guns to an SMG or shotgun.
24. After a fight a bot draws its other gun when that one's magazine is
    low, and reloads it.
25. Put a second SMG beside a Normal or higher SMG bot, then try plasma
    rifle + SMG, needler + needler and plasma pistol + SMG in both orders.
    Check the left-slot pickup, both triggers and the charged pistol shot.
    At long range the bot should keep its rifle. Present a group at grenade
    range and check that a short Y drops the left gun before the throw.
    Repeat with a friendly in the blast area and with a wall across the arc.
26. A magnum bot backs off to its rifle at a shorter range than before;
    a bot with an SMG stops spraying at you across the map.

Natural feel ("Built: natural feel (sol)" in §10):

27. Compare Easy and Legendary on the same map. Peek from the edge of a
    doorway, then cut sideways across the bot's view. Watch for a pause
    at first sight, a quick turn with a small correction, and more lag
    on fast crossings. Easy takes longer to settle.
28. Strafe behind a pillar and change direction while hidden. Watch the
    bot hold or scan the opening, then pick you up when you reappear.
    Fire from behind a nearby wall. A short look toward the report can
    happen before the bot has a target.
29. Follow a bot through a doorway and around a corner. Look for small
    changes of pace, a brief look into the opening, and a path that
    leaves the frame and railings clear. Compare two bots strafing.
30. Throw a grenade into plain view near a bot on level ground. Look for
    one quick escape movement and, up close, a single jump when the
    route is clear. In an ordinary gunfight, hits lead mostly to strafes
    or crouches. At a genuine gap the bot takes a run-up.
31. Watch a phantom in a quiet CTF or Hill approach, near an empty
    magazine and during a close melee encounter. Look for a slower
    crouch-walk, a brief delay before X or Y, and spaced melee swings.
    Leave it waiting and check the lobby selection and respawn remain
    under the human's control.

Finishing fights (§3.3, "Finishing and breaking off"), Slayer on Midship
and Lockout:

32. A bot that has your shield down closes in and melees instead of
    strafing at range while your shield comes back.
33. A plasma rifle bot does not fire at you across the map; it closes to a
    few metres first.
34. A bot that loses you round a corner holds the corner or follows you
    before it answers a teammate's call or walks back to a teammate.
35. In Juggernaut, hunters head for the Juggernaut even when it is out of
    sight, and the Juggernaut does not back away from two hunters.
36. No bot stands on a weapon, or walks back and forth to one, for more
    than a couple of seconds.

### 11.2 Objectives in play (checklist)

For the user, or a coordinator run. Two teams of two or three (a human
and Clankers), Normal. Each line is a yes or no.

CTF on Lockout or Beaver Creek:

1. With both flags home and out of view, the attackers leave the spawn
   for the enemy base; the nearest goes for the flag, escorting names
   follow it, and (with three or more on the team) one stays near its
   own flag. They fight whoever they meet on the way.
2. A bot reaching the enemy flag takes it even with a full inventory.
   Check the X hold while the flag is loose and its release on pickup.
3. A bot carrying the flag runs home along the floor, does not shoot,
   does not drop it (no weapon switch) and does not stop to reload or
   for a gun, even with its shields down. It melees a human standing in
   its way.
4. Reaching home with our flag there, it scores (the score goes up).
5. Take a bot team's flag: its defender, and an attacker nearer to you,
   come after you; when you drop it, the nearest bot walks over it and it
   goes home. Break line of sight with it: the defender looks where you
   were, then heads for your base.
6. A bot whose teammate has the enemy flag runs near them.
7. Spawns and returns are right on both maps: the bots' "home" is their
   own base, not the enemy's (the designator map, `E+8`/`E+0x10`).

Oddball (any small map):

8. When the ball spawns, the bots run for it; in teams two of a side run
   and the others stay near it.
9. A bot with the ball takes cover or moves away from an approaching enemy.
   It keeps its trigger up and melees a human who comes close.
10. When a human has the ball, the bots come after them, together rather
    than one at a time, even with the human out of sight; in teams, a
    bot's teammate with it is screened, the escorts standing between it
    and you.

King of the Hill (Midship or Lockout):

11. The bots go to the hill and stay in it, fighting from it and strafing
    inside it, rather than standing still or chasing out of it. With the
    hill held, a cautious bot covers it from its edge.
12. When the hill moves, they go to the new one within a few seconds.
13. A bot in the hill counts as in it (the hill's colour and the score
    say so). Check the score on a different floor over the same XY and after a move.

Assault (if tried):

14. Check which side picks up a team bomb and a neutral bomb, and whether
    the carrier scores at the plate its escorts defend (§3.7). After the
    plant nobody, of either side, stays within about 6 world units of the
    bomb when it goes off.

All:

15. No bot teabags while carrying, guarding or in the hill.
16. Rotate a one-bomb round. Check both teams swap roles, the carrier
    stays until armed and plants on the active plate, and escorts cover
    a planted bomb while recovering a loose one.
17. In team Territories, contest a friendly site while a neutral site
    is closer. Watch two bots relieve it and hold through a capture. A
    team's bots take different sites rather than stacking on one. In
    FFA, check that another player's territory counts as hostile.
18. Kill the Juggernaut in front of two hunters. Check they switch to
    the new Juggernaut and leave one another alone. As Juggernaut the
    bot should close on a visible opponent and melee.
19. In Oddball check the first spawn and a reset against a neutral type-4
    marker on the map.
20. In Oddball, kill the bot carrier: the nearest bots of both teams run
    onto the dropped ball and one picks it up.

### 11.3 Decisions boost in play (checklist)

Use Heroic with at least two Clankers on one team. For the route and
Oddball checks, use a map with a nav mesh and more than one way through.

1. **Team Slayer.** Present two similarly distant targets. Watch the bots
   split fire, then focus fire when one teammate takes damage. Break the
   receiver's line of sight. It may move toward the call. Its trigger
   should stay up until it can see an opponent. Kill the caller in view
   of the other bot and look for an immediate target switch after its
   normal reaction delay.
2. **Odds and openings.** Isolate one bot against two nearby opponents.
   Watch it back off, then regroup when sight breaks. Try a one-on-one
   with a depleted shield or an empty magazine hidden from the bot. Changing
   those values alone should produce the same decision. In free-for-all, fight
   another player in sight of a third bot and watch for it to close.
3. **CTF.** Drop the home flag while a teammate is carrying the enemy
   flag. Check the return, then the escort and home guard assignments.
   Cause a death at one entrance and watch a carrier try the other one.
   Repeat later to see the danger expire.
4. **Oddball and King.** A ball holder should settle behind cover and a
   hunter should favor the visible carrier. Shoot from outside the hill.
   Watch for a bot to enter while returning fire, remain inside while
   scoring and move when the hill changes.
5. **Assault.** Try a team bomb and a neutral bomb. Follow one carrier to
   the opposing plate. Watch teammates cover the plant and defenders
   intercept the carrier. Drop the bomb on the plate and check who goes
   to it.
6. **Supplies.** Let a calm bot see you take a sniper rifle, rockets,
   overshield or camouflage. Watch it return near the next spawn. Repeat
   with the pickup hidden. An unseen disappearance cannot date a return.
   Contest a due pad with a sword or rockets and check the bot can fight
   from pickup range. Try half, double and disabled respawns. Compare its
   arrival with the actual service tick. Hand a sniper to another player
   in view and check the bot recognises the new wielder. Repeat with a
   hidden handover and check that the old sighting decays.
   Repeat on Easy with the same roster to compare the timing.
7. **Outdoor map knowledge.** On H1 Blood Gulch and Sidewinder, give a
   Clanker a sniper and watch which lane it faces. Repeat from the other
   base, with a lead and with a visible opponent. Verify both flanks are
   reachable, a lead can produce a hold, and combat interrupts it.
   Let it inspect an empty sniper pad and turn away. Swap the hidden
   respawn state while preserving all cues, then expose the pad and
   look for a change only when the bot can inspect it.

### 11.4 Vehicles in play (checklist)

For the user, or a coordinator run. Two or three Clankers on Coagulation
or Headlong (Slayer), Normal, then Legendary. Each line is a yes or no.

1. Soon after spawning, a bot with no enemy in view walks to a nearby
   Ghost or Warthog and gets in from the side.
2. Two bots on one team do not both run to the same vehicle; with a
   teammate driving a Warthog, another bot gets on its gun.
3. A bot driving a Ghost follows the map's roads and paths, turns
   without circling, and does not sit against a wall for more than a few
   seconds (it backs up, then gets out).
4. A driving bot drives at a human on foot in front of it and runs them
   over, firing the Ghost's guns; a Warthog driver does not fire.
5. A bot gunner shoots at the human; when its driver gets out, it gets
   out within about 4 s.
6. A bot in a vehicle about to blow up, or flipped, gets out.
7. A bot holding the rocket launcher, at a human driving a vehicle, keeps
   the reticle on the vehicle for about a second and fires a rocket that
   follows it; at a human on foot it fires at once.
8. The human driving a Ghost slowly past a Normal bot within about 1.5
   (about 3 on Legendary) gets boarded; driving fast past it, not.
9. The fields: on the human's own unit in a Warthog, `h2 agent object`
   reads `+0x1fc` 0 as the driver and the gun's seat on the gun; the
   Warthog's `vehi` seats' `+0` flags have bit 2 on the driver's seat and
   the gun's bit 3; `+0x1f0` reads 1 for the Warthog, 4 for the Ghost, 5
   for the Banshee.

### 11.5 Traversal and sandbox movement in play (checklist)

1. On Coagulation or Beaver Creek, put a goal closer through a
   teleporter and watch a bot enter the sender, arrive and resume walking
   on the correct floor. Repeat with the chosen receiver blocked or on a
   map with two receivers on one channel.
2. On Ascension and Colossus, start below the lift exit. Check that the
   bot enters the launch area, steers after pickup and walks off at the
   upper landing. On Cyclotron, repeat for the moving elevator and check
   the bot stays aboard while it rises.
3. On a low-ledge route, check that A starts the jump, crouch follows
   takeoff and the bot reaches its landing. On a low-roof route, check
   that the bot turns away from the standing-height obstruction.
4. On Ascension, watch a bot enter the Banshee, climb, make a strafing
   run and boost out. Damage it above the ground and check the evasive
   roll. Check secondary fire with a variant that enables the fuel rod
   bomb. Check the bot descends before exiting for an objective.
5. On Zanzibar or Headlong, watch a bot take a standalone gun covering
   the fight, then dismount when approached from the side. Repeat with a
   teammate driving a Warthog using a gauss or chaingun turret.
6. On Coagulation or Headlong, approach a Scorpion or Wraith from either
   side and a Warthog from beside the driver door. Compare the bot's X
   press to the actual seat marker in each case. In a Ghost or Warthog,
   choose a goal with a pedestrian shortcut and a wide detour. Check
   the bot follows the wide route and clears the turns.
7. Repeat traversal jumps, Banshee button actions and vehicle entry and
   exit in a hosted custom game and Clankermatch. Inspect playback through
   `h2 agent` and the existing `sim.*` and world methods (§10.1).
8. On H1 Blood Gulch verify both outbound portals from their bases. On
   Sidewinder exercise both directions between the paired pads and each
   outbound-only pad. Try a crouch-jump route above standing reach on
   both maps. Check whether Sidewinder's blast-door scenery moves.
9. On Containment watch both high receivers through free fall. Check
   the actual player exit on all four Warlock lifts, the Lockout tube and
   Midship's midpillars. Try Gemini's columns with a player. On Relic
   compare traversal before and after activation of the portal switch.

## 12. Next tracing (offline)

Done for stage 1 (§4.1 to §4.3): the pad seam, the slot flags and the
sign-in function, the players screen's join path, and the readers of the
local-user map.

Still open:

- For phantoms (§5.1, "Open"): the lobby's ready check `0x5f970`, the
  join room check `0x62e30` and the veto object, `0x5fe20` on a peer with
  no channel, and `0x14c090`'s body.
- Verify the phantom's per-tick hold transition, zoom release and
  grenade throw after dropping a dual weapon (§5.1 and §8.5). The button
  masks and selection fields have static traces.
- Which font slots draw player names in the lobby, scoreboard and carnage
  report.
- For objectives, the traced fields and their writers are in §3.7. The
  outstanding checks are pickup behaviour, rotated one-bomb rounds, hill
  transitions and capture volumes in play (§11.2).
- For vehicles (§3.9): verify the rocket launcher's lock time and zoom
  requirement, the units of `O+0x88` (velocity), the Banshee's secondary
  fire and roll in phantom play, and whether tag dimensions need a
  larger margin on tight turns. For lifts (§3.2), measure the launch
  trajectory and ride time on the retail maps.
- For weapons (§3.6), trace plasma heat and compare the predicted service
  phase with a witnessed pickup and actual return on phantom pads,
  including alternate H1 sandboxes and multiplayer variants.

## 13. Testing (`h2 bots test`)

`h2 bots test` runs §9.5's game run with no one at the controller: one
command that ends in a verdict. It is built on `h2 coop test --drive`'s
pieces (docs/live/live-coop.md section 12): the same process supervisor,
account making and `input.pad` presses.

```sh
h2 bots test [--count 5] [--map lockout] [--keep] [--timeout 360]
             [--wait-clang] [--settings-from DIR] [--no-drive]
h2 bots verdict <bots.log>   # the verdict of a log again
```

**What it does.**

1. Starts a Live server in the harness process on `127.0.0.1:8077`
   (`--server-port`), with a throwaway store under
   `$TMPDIR/h2-bots-test/run-<time>/` (`--root`), and makes the account
   **Tester** there.
2. Opens one `h2 play` window signed in as Tester (`--live-server`, its own
   data and Live directory, `--no-mappack --no-halo1`), with
   `--set clankers.enabled=true --set clankers.count=N`. So Clankers is On
   at Count N for this run with no menu work (the page's rows show it
   locked). `--count` defaults to five phantom peers. The window builds
   the game into `--out` first when `--out`
   has none, as `h2 play` does. `--wait-clang` passes `--tier clang`, so
   the run uses clang's full build and never the Cranelift fast tier; it
   first waits while a background clang build of `--out` is still writing
   `clang-build.log`. The run's data starts with the `[graphics]` table of
   `--settings-from`'s `settings.toml` (by default `~/Library/Application Support/h2recomp/data`,
   the data root `h2 play` uses), so the window renders with the user's
   own video settings.
3. Presses through the menus over the game's agent socket
   (`crates/apps/cli/src/bots_drive.rs`). The game runs with `H2_UI_WATCH=1`, and
   every press waits on a `ui: screen` line, a game line or the agent API's
   phase. A fresh data folder stops on the title screen and the new
   profile's dialogs: each gets A once it has held 4 s. The main menu is
   the window 5 screen the game names with `coop-live: lobby mode
   4294967295`; its cursor row is the low 16 bits of the `list [mask/value]`
   value on its `ui: screen none` line. Xbox Live is row 1. If Split Screen
   or System Link opens, the driver uses the players' screen mode to
   return to Xbox Live. If the profile needs to sign
   in, Xbox Live opens the game's account picker (`dialog_live_signin`,
   dialog `0x14`, loader `0x24b4a9`), as a new profile's boot does. Its
   row 0 is the no-account row. The driver selects Tester and opens
   the Xbox Live menu. It moves to Custom Game on row 4 and checks
   that the game advertises a session in lobby mode 2. Once the
   phantoms have joined the lobby, or after 90 seconds, it presses A
   on Start Game. Each press is a `bots-drive:` line.
4. Watches the game's `bots:`, `clankers:` and Live lines, the agent API's
   `session.status` (phase, mode, map), the server's session for Tester
   (`GET /v1/sessions?host=Tester`: filled slots) and, once in gameplay,
   the newest game record (`GET /v1/games`: players with `bot: true`).
   What it reads goes into the merged log as `bots-test:` lines.
5. Stops once every check has passed (3 s more for the last lines), the
   game closes or stops, or after `--timeout`, counted from the game's
   agent socket coming up (a build before that is not counted, up to an
   hour). It prints the verdict,
   closes the game, deletes the run's data and exits non-zero if a check
   failed. With `--keep` the game stays open, and the command with it (it
   is the Live server), until the window closes; the data is kept.

**`--no-drive` and `progress.json`.** `--no-drive` launches and watches
exactly as above but presses nothing: the scripted driver is off, and the
verdict is the same. At the start it prints the agent socket and a hint
(`h2 agent --socket <sock> ...`). `--timeout` still counts from `agent:
listening`; a drive by hand or by a model needs longer, so it defaults to
900 s with `--no-drive` (360 s otherwise; an explicit `--timeout` wins).
Either way the run's log directory gets `progress.json`, rewritten
whenever a check, the status or the last lines change (written through a
temporary file, so a poll never reads half of it):

```json
{ "finished": false, "passed": false,
  "next": { "step": 3, "name": "on Xbox Live ..." },
  "phase": "menu", "mode": "-", "map": "-",
  "last_ui": "ui: screen none (slot 4: ...)",
  "last_bots": "bots: ...",
  "checks": [ { "step": 1, "name": "...", "state": "pass|pending|fail",
                "detail": "..." } ] }
```

`checks` contains the nine checks below. `state` is `pending` while
the run goes and `fail` for what did not pass once it has ended. `next`
is the first check not passed. An outside driver loops: read
`progress.json` (stop on `finished`), `h2 agent --socket <sock> ui` and
`h2 agent --socket <sock> frame snap 1` to see the screen, then
`h2 agent --socket <sock> press down` (or `a`, `b`, `up`) as the first
pending check and `last_ui` call for, and read `progress.json` again.

**The checks**, in the order a run reaches them. Each prints as passed or
failed with the log lines that show it, and the verdict names the first
failure:

| # | Check | Passes on |
|---|---|---|
| 1 | Signed in to Xbox Live | `live: playing as Tester` |
| 2 | Clankers on | `clankers: G (phantom...) on` for each bot |
| 3 | On Xbox Live | the Xbox Live players screen or menu |
| 4 | Hosting a Live custom game | the game advertises a session in lobby mode 2 |
| 5 | Live roster of bot accounts | `clankers: asking Live for N bots for session S`, then `clankers: Live rostered bots for session S` |
| 6 | Gameplay on a map | `session.status` reports multiplayer gameplay, on the requested map if given |
| 7 | Phantoms joined the pregame lobby | `bots: phantom G joins (peer K, in the pregame lobby)` for each bot |
| 8 | The Live session holds 1 + N players | the greater of the filled-slot count and the number of players the host reported |
| 9 | The game record has the bots | at least N players with `bot: true` in the server's game record |

A failed check includes the relevant lines, such as the phantoms'
`waits` messages if they never joined. The verdict also lists panics,
guest faults, refusals, stuck drives and timeouts under `trouble:`.

**Where things go.** The merged log is
`<root>/logs/<time>/bots.log` and the game's whole stderr
`<root>/logs/<time>/Tester.log`; both paths print at the start and the
end. `h2 bots verdict bots.log` prints a log's verdict again (and exits
non-zero on a failure).

**Notes.**

- `--map NAME` picks the custom game lobby's map before Start Game, through
  the game's own screens (`bots_drive/map.rs`): DOWN, A to Game Setup
  (dialog `0x17`), A on Change Map, A on a category, then DOWN to the map's
  row and A. The game names each Select Map list's rows in order
  (`map select: rows: ...`, `map_select::row_names`) and says
  `map select: picked NAME`, so the driver counts its presses off those
  lines and a category without the map is backed out of for the next. A
  screen that does not come stops the drive and names it. The gameplay
  check then confirms the map played. The path is from the docs, not yet
  seen in a run.
- The Clankers fill runs twice in a log: first as host-made bots
  (`clankers: no roster yet: this console hosts no Xbox Live session`), then
  again once the lobby is hosted, when Live's roster replaces them (the
  first set logs `off`, the rostered set `(phantom, rostered)`). Peers 1 to
  15 are reused. Only the second set is the game's.
- The phantoms join the pregame lobby (§5.1, "Phantoms in the pregame
  lobby"). Step 8 counts the most filled slots and the most joined
  players, the server saw over the whole run. Retail Halo 2 fills no slots (0 in run
  20261119-005001), so on a Live run the joined players are what counts.
- `h2 play` builds the game into `--out` when `--out` has none, or has one
  from another image. Run it with the same `H2_OPT` as that build, or the
  window recompiles first. The timeout starts once the game is up, so a
  build doesn't eat it.
- One game at a time: the server port and Tester's account are the run's
  own, but a second `h2 bots test` needs another `--server-port` and
  `--root`.

### 13.1 Clankermatch (`h2 bots test --clankermatch`)

The same harness runs §8's Clankermatch end to end, so the §8.5 checklist's
steps 4 to 9 have a verdict instead of a person at the controller.

```sh
H2_OPT=2 ~/.cargo/bin/h2 bots test --clankermatch [--playlist slayer|teams|doubles] \
    --disc disc/halo2.iso --wait-clang [--keep] [--timeout 900] [--no-drive]
```

`--playlist` implies `--clankermatch` (slayer by default); `--count` and
`--map` are refused, since the playlist sets both. Everything else
(`--keep`, `--root`, `--out`, `--server-port`, `--settings-from`,
`--dry-run`, `--no-drive`, `progress.json`, `h2 bots verdict`) is as
above; the merged log's plan line is `bots-test: clankermatch plan: ...`,
which is how `h2 bots verdict` knows which verdict to print.

**Server state.** None to set up: the run's fresh store has no published
hopper file, so the server serves the built-in lineup
(`live_server::default_hopper`), whose hoppers 8..15 are the eight clanker
playlists, marked on `GET /v1/hoppers/clankers`. The harness reads the marks
before it starts the game, checks the playlist asked for is marked and on
the row the plan expects (the Clankermatch list shows only the clanker
hoppers, in index order), and stops at once if not. The game runs with
`--set clankers.enabled=false`: the page's Clankers stay off and only the
fill seats bots.

| `--playlist` | Hopper | Row | Players | Phantoms |
|---|---|---|---|---|
| slayer | 8 Rumble Pit (FFA) | 0 | 2 to 8 | 7 |
| teams | 9 Team Slayer (2 teams) | 1 | 2 to 8 | 7 |
| doubles | 10 Team Skirmish (2 teams) | 2 | 2 to 4 | 3 |

**The drive** (`crates/apps/cli/src/bots_drive/clankermatch.rs`): boot, sign-in
and the Xbox Live menu as above, then row 2 (Clankermatch), A. The playlist
list is the first settled screen after that A; its row is read off its list
value (or counted from the DOWNs sent, when the value reads as no row), and
A goes on the playlist's row. Then nothing is pressed: the search (4 to 20
s), the lobby this console hosts, the fill and the countdown run by
themselves. A human playlist's search, no search 15 s after A (A again, 4
tries), no hosted lobby 90 s into the search, or a lobby that has not
started in 180 s stops the drive, saying which.

Once the game has played and the phase is back in the menus, it leaves: B (A
on a dialog, and DOWN, A on the next one; A after a B that changed nothing,
as on the carnage report) until this console hosts no session, then waits
for the fill to end (30 s, `FILL_GONE`). A game that starts again first is
waited out. The game runs to its own end: Rumble Slayer's 6 minutes, Team
Slayer's 8, or the score, so the timeout is 900 s (1200 with `--no-drive`).

**The checks:**

| # | Check | Passes on |
|---|---|---|
| 1 | Signed in to Xbox Live | `live: playing as Tester` |
| 2 | Clanker hoppers listed (server and game) | `bots-test: server clanker hoppers [8, 9, 10, 11, 12, 13, 14, 15]` and `live: clankermatch: clanker hoppers [8, 9, 10, 11, 12, 13, 14, 15]`, both with the plan's hopper |
| 3 | The Clankermatch row opened the clanker playlists | `clankermatch: the playlists are the clanker hoppers`, and no `the server named no clanker hoppers: the list is empty` |
| 4 | The clanker search started | `live: clankermatch: searching clanker playlist H` with the plan's H |
| 5 | The search ended with this console hosting | `live: session S advertised` after the search, and `clankers: clankermatch: hosting clanker playlist H (session S)` |
| 6 | Phantoms planned for the lobby | `clankers: clankermatch: N phantoms join the lobby` with the expected count |
| 7 | No bot in the party | no `bots: signing ...` line for a controller port |
| 8 | The phantoms have rostered accounts | one `clankers: G (phantom, rostered) on (...)` line per planned phantom |
| 9 | The lobby reached the playlist's fewest players | the most players the game's own session had (`bots: lobby players: N in the game's session`, the game session's `+0x1118`) between the hosting line and the start; the Live server's count (`bots-test: session ...`) is what the host told it and no longer counts (cm-run2 listed "4 joined" with no bot in the lobby) |
| 10 | The countdown ran and the game started | `session.status` phase `loading` or `gameplay` on a multiplayer map after the hosting line: not `map mainmenu` or mode `main_menu` (cm-run2's `loading` was the main menu coming back) |
| 11 | Gameplay on a map | phase `gameplay`, not campaign, after the start |
| 12 | Phantoms joined the lobby before the start | `bots: phantom G joins (peer K, in the pregame lobby)` for each phantom before the start, and no phantom failure |
| 13 | Game record: kind clankermatch | a game record from this console has kind `clankermatch` and at least as many `bot: true` entries as planned phantoms |
| 14 | After the game, the phantoms leave | the fill ends and each phantom has a `clankers: G off` line |

A failed check includes the relevant server, game or driver lines. The
join check includes any phantom wait or failure reasons.

**Tests:** `cargo test -p cli bots_` (the verdict from log lines, the
driver's presses, the arguments; Clankermatch's under `bots_drive::clankermatch`,
`bots_verdict::clankermatch` and `bots_test_cmd`).
