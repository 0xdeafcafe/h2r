# Bots: stages 6 to 9 and the library surface

Status: part of [bots-stages.md](bots-stages.md); see there for status, labels and the index. This page: stages 6 to 9, combat decisions, weapon skill and the library surface.

### Stage 6: Phantom peers

Partly built.

#### Phantom books (6a)

  - The membership path is traced (§5.1: the session layout, the
    recipe, and what is open).
  - `bots::phantom::Phantoms`: the books. `add` checks the identity (a
    gamertag and a non-zero xuid), refuses a second phantom with the same
    xuid and more than 15, and puts the phantom on `Pending`. Each
    phantom's machine id is `C1 A4` and its handle, big-endian; `C1` has
    the multicast bit set, so no adapter's address is ever one.
    Handles are never reused. `refresh` finds each phantom's player by
    machine id in the player records (`Joined`, with the index), and
    sends it back to `Pending` when the player goes; `Failed` keeps its
    reason.
  - `Frame::machine` identifies the phantom's player in the brain's perception.
  - `sim`'s `phantom_player` reads `bots::phantom::player`.
  - `live_proto::bots`: the roster (§9.1) and its request, checked by
    §9.1's rules, and `local_xuid` for host-made identities (high bits
    `0x000E`; server accounts carry `0x0009`).

#### Session recipe (6b)

  - `bots::phantom::session`: the session recipe of §5.1 as memory
    writes (peer, player, removal), the configuration, the simulation
    slot, the action record and the action from a pad.
  - In the original game-only configuration, `Phantoms::tick` maintains
    membership before each `RENDER_FRAME`. In play the server's action
    gather samples each joined source with its machine ID. A removed phantom's peer comes out. With the pregame
    join disabled, **phantoms join only once the game runs**. When
    it ends they leave the session and wait for the next game, so the lobby never holds one. On a client, or a
    local game, or before this console is the game's host (the first
    frames of a game), they wait and try again each frame, logged once
    per phantom and reason (`bots: phantom Clanker 4 waits: this
    console is not the game's host yet (the simulation is not the
    distributed server (world mode 5, …))`; `NotNow::NotHost` says which
    check: no network session, the simulation mode, the session state
    or its class). With no room, the one that does not fit fails.
  - The Clankers page's count past the free ports adds phantoms:
    "Clanker 4" and up, host-made xuids (`live_proto::bots::local_xuid`),
    the page's difficulty, the host's team. A lower count takes them
    off first.

#### Server side, both homes (6c)

The server code is `live_server::bots`, on the shared router and read model.

  - Bot xuids have a range of their own, `0x000B`
    (`live_proto::bots::SERVER_XUID_MARK`): apart from humans' `0x0009`
    and host-made `0x000E`. Each bot is a server-signed `BotIssued` on
    `bot-<xuid>` (version 0, so an xuid is issued once) with its
    gamertag, emblem, colours and personality (a seeded splitmix, so a
    persistent bot's quirks are fixed).
  - `POST /v1/games/bots` (`RosterRequest`, a `games.write` token):
    only the host of the request's Live session; `count` 1 to 15,
    cut to the session's room. With a `playlist` (the playlist id): the
    persistent bots `Ranks::nearest` the humans' mean level there, each
    at its own level and `bot_skill`; the gap made-up bots at the mean
    level. Without one: made-up bots at the request's `skill` (Normal
    when absent). `team` stays `null`; the host balances. The answer is
    the §9.1 roster with `match_id` = the session id, and the server
    writes `BotsRostered` on `bots-<session_id>`. With `carry_from`
    (the host's previous session, ended within 120 s): that session's
    bots first, as they were (§9.5, "One identity across a session
    handoff"). The session's players that are bots do not count as
    humans for the room or the mean level.
  - `POST /v1/admin/bots {count}` (the operator, up to 64): persistent
    bots, the pool rosters draw from. A persistent bot reserves its
    gamertag as an account would (no account can take it). A made-up
    bot does not: it only avoids names taken at the time, so made-up
    bots never use up the namespace.
  - Acceptance (§9.3), in `POST /v1/games` and in `PlayerJoined`: a
    host-made xuid is refused; a `bot: true` entry needs the game's
    `session_id` and an xuid in that session's roster (in force now at
    the start; in force at the start for a join), a roster the game's
    host made itself (`Rostered.host`; another account naming the
    session cannot use it); an issued bot as a human is refused. `session_id` is allowed on custom games now (a
    game with bots names the session its roster came from); still not
    on campaign. `game::Player` has `bot: Option<bool>`.
  - Nobody else writes `bot-*`/`bots-*` or the two kinds.

#### Console side (6c)

  - `live_hle::Hle::hosted_session` (the advertisement this console made
    last, while it stands) and `Hle::roster_bots`: `POST
    /v1/games/bots` on the Live worker with a `games.write` token of the
    account that made the session; the answer is
    `Roster::check`ed and handed to a callback on the worker. Never
    waits.
  - `boot::clankers::Driver::with_rosters` (installed with Live): when
    the page wants more bots than free ports and this console hosts a
    Live session, the driver asks for a roster of the phantoms' count,
    at the page's difficulty (`request_skill`: Easy 0, Normal 0.33,
    Heroic 0.67, Legendary 1; Mixed asks for none, so Normal). It asks
    again when the count, the difficulty or the session changes (the
    bots' frame hook checks the session about once a second, so a
    session made after the page was set gets one); an answer to an
    older ask is dropped.
  - Until a roster comes, or when none does (refused, Live off, System
    Link, offline), phantoms are host-made, as in 6b. Once it comes,
    they all go and the roster's bots replace them
    (`clankers::roster_seats`): xuid and gamertag as the `Identity`; the roster's team when not null; a brain at the
    nearest named skill (`skill_of`), with the gamertag's profile and the
    roster seed for individual choices. Taunts off suppresses taunting. The
    page's difficulty never changes a roster bot's skill.
  - Telemetry (`live_telemetry::Bots`, from the driver's
    `bots_in_force` through `Telemetry::set_bots_locator`, asked at
    each multiplayer start): a custom game names the roster's session
    as `NewGame.session_id`, and each player whose machine id
    (`PlayerState::machine`, player `+0x14`) is a rostered phantom's
    goes out with its xuid and `bot: true`, on `GameStarted` and on a
    later `PlayerJoined`. Local-port Clankers, and phantoms with a
    host-made xuid, go out with `bot: true` and no xuid (§6): a
    `0x000E` xuid is never sent.
  - `teams` is always false in the request (the page does not know the
    gametype). Local-port Clankers are rostered as bot accounts (§9.5).
    Clankermatch's `MatchProposed` carries the same roster (stage 8,
    with sbmm-ranked).
- In-memory phantom machines, peers and players; action conversion and
  storage; network drop and keep-alive.
- **`bots::phantom::add` and `remove`** keep the signatures they have
  now (§10.1); `sim.*` already calls them (`boot::sim_bots`, the xuid
  given as hex). `add` returns once the phantom is on the books. The
  phantom's machine, peer and player are made at the next tick on the
  game thread, so `add` never waits on the game. A phantom asked for
  outside a host's session fails then, not at `add`; `list` reports
  that, as below.
- **From a phantom to its player.** `bots::phantom::list() ->
  Vec<PhantomInfo>` gives each phantom's handle, identity, team, and
  its game player once the membership update has made it (`player:
  Option<u32>`, the player datum's index, as `sim.view`'s `player.<i>`
  uses), or why it has none (`state: Pending | Joined | Failed(reason)`).
  `bots::phantom::player(handle) -> Option<u32>` is the short form, for
  `sim`'s `phantom_player`. The index is found by the phantom's machine
  id in the player records, not guessed from the order of joining.
- **Identities from the local embedded Live server.** Bots are accounts
  (§9.1), so a phantom's xuid has to be one the server issued:
  - `live_server` gets `POST /games/bots` (§9.1) on the embedded server
    first (`h2 play --live-local`). It makes `count` bot accounts for the
    session (made-up gamertags from a word list, xuids in a range of
    their own, `bot: true` on the account), and answers with the roster.
    It refuses unless the asking host has a session.
  - Bot accounts are flagged on the server, so a game record's `bot:
    true` is accepted only for an xuid the server issued as a bot.
  - The host asks for the roster when the Clankers page wants more bots
    than free ports (or always, with Live on). Each roster entry becomes
    a `bots::phantom::Identity` (xuid and gamertag) and a
    brain at the entry's skill.
  - `sim` can ask the same endpoint for identities and pass them to
    `sim.player.add {kind: phantom, identity}`. The bots crate never
    talks to the server; the host glue does.
  - System Link and offline games make their own identities, with xuids
    in a local-only range that the server never accepts.

#### Hosting the game (6c)

  - The phantom check takes world mode 4, the distributed server (the
    game numbers it 5; §5.1, "Hosting the game"): `world_mode`,
    `WORLD_DIST_SERVER`; test `cargo test -p bots --lib
    phantom::session`.
  - `SESSION_GAME_START` (`0x6ec80`): with phantoms planned and no
    other console in the session, the session's host peer is made this
    console's before the game picks its mode, so a lone Live host runs
    the distributed server (§5.1, "Hosting the game"). Each start logs
    `bots: game start: ...` with the fields it read.
  - `LOBBY_ROW_PRESS` (`0x250fb3`): A on the lobby's Start Game row
    logs the start state (§4.2), as a Start press does.
  - The session's players (§9.5): the host reports its bot accounts
    as `PlayerJoined`, which the server takes for its leased bots.

#### Session handoff (6c)

  - The roster carries across the handoff (§9.5, "One identity across
    a session handoff"): `RosterRequest::carry_from` on the server and
    worker, `clankers::ask_for` on the console.
  - The record marks every phantom-machine player as a bot.

### Stage 7: Clankers page (Game Setup)

`boot::clankers`.

- **Settings** `clankers.*`, kept in the profile store
  (`setting_defs::ALL`):
  - `enabled`: Off or On; default Off;
  - `count`: 1 to 15;
  - `difficulty`: Easy, Normal, Heroic, Legendary or Mixed. Mixed is
    Normal, Heroic and Easy on ports 1, 2 and 3;
  - `team`: Auto-balance, Red or Blue;
  - `aggression`: Calm, Normal or Wild;
  - `aim_style`: Steady or Twitchy;
  - `taunts`: Off or On; default On;
  - `glitches`: Off, Some or Lots; default Off (§3.8, used).

  The custom-game driver applies Aggression, Aim style and Taunts to
  existing seats as described in §7.
- **The page** is the shared settings dialog, `clankers`
  (`content/ui/screens/clankers.yaml`; docs/game/ui-builder.md "The
  settings dialog"): eight `setting:` rows over the settings above, on
  the lobby's own settings dialog, Quick Options, inside and out:
  Quick Options' panel, header ("CLANKERS"), list (its `setting_lists`
  skin, the name left and the value alone in the value column), and
  the legend in its footer. Its top is the same at every focus: the
  Spartan icon in Quick Options' game type picture, "Bots in this
  game", and the Game Setup row's help. The focused row's help is
  under the rows. A on a row opens the value picker, which explains
  the option and its choices (the row's `explain`). Its texts are the
  `clankers` id family. In the preview it is `h2 ui preview clankers`.
- **The row.** A custom game's Game Setup dialog (any list with Change
  Map) gets a "Clanker Options..." row with action 11, under Quick Options and
  the Halo 1 Options row's 10, above the "Switch To:" rows (retail
  actions 6 to 8; `h1_options_row::insert_row` writes the rows'
  actions back in that order). It uses the same three points, and each
  hook is ordered after the Halo 1 row's. A press opens the page on the dialog's own
  stack. The row's text has a slot of its own after the page's footer
  (`KitScreen::row_label`, "Clanker Options..."), so the page's title
  stays "CLANKERS" (in capitals, as Quick Options' header is). The
  Halo 1 row does the same with its id 2 (`h1_options::ROW_LABEL_ID`,
  "Halo 1 Options..."), its screen's title staying "HALO 1 OPTIONS";
  both read like the dialog's other "...Options..." rows.
- **Seats.** Clanker Options configures the requested phantom roster.
  The match host creates the peers as the hosted game starts
  (`crates/extensions/game/src/clankers.rs`,
  `crates/extensions/game/src/bot_glue.rs`). See §5 for peer admission
  and §9.1 for the Live roster. The page's bots sit only in a custom
  game's lobby (retail mode 2: the party lobby's CUSTOM GAMES, FORGE
  and FIREFIGHT modes, `clankers::admits_page`). In the co-op and the
  matchmaking lobby they stand down and come back in a custom mode
  (`Driver::set_lobby`, [party-lobby.md](party-lobby.md#clankers-by-tab)).
- **Nav meshes early.** A map picked in the lobby's map list with
  Clankers on, or Clankers coming on after a map was picked, asks
  `bots::autonav::build_soon` for its mesh (`clankers::map_picked`).
- **Game check.** In a System Link or Live host lobby, open
  **Clanker Options**, enable Clankers and set Count to 2. Start the
  match and confirm two phantom players join the roster. Section 11
  gives the combat checks, and §13 covers the hosted harness.

### Combat decisions

`bots::mind` uses `bots::arms` and
`bots::tactics` to pick weapons, targets and fighting positions.

- **The weapon is known** (§3.6, "Handling"): role, band, the range
  it shoots to, its trigger, its rate and its projectile's speed.
- **Melee.** Within arm's reach all difficulties attempt a beatdown
  when facing the enemy. A visible shield break or a plasma weapon in
  hand favors a strike at the outer edge. A sword uses B for a quick
  slash at arm's length and RT to lunge at a target in its lunge band. The chest direction gates the strike with any gun.
- **The trigger.** A once-per-pull weapon (magnum, battle rifle,
  carbine, shotgun, sniper, rockets, the plasma pistol's quick
  shots) pulls the trigger for 2 ticks and let go, every `30 / rate` ticks (at least
  4; 8 when the rate is unknown) plus the skill's pace (Easy 12,
  Normal 6, Heroic 3, Legendary 1 tick). Automatics are held inside
  the gun's band and burst past it from Normal up (§3.4). A blade uses
  the sword slash and lunge inputs above.
- **No dry fire.** The trigger stays up with an empty magazine or a
  dead battery; it reloads or switches instead. It fires only in the
  weapon's range: a shotgun no further than 8, rockets never nearer
  than 2.5.
- **Weapon choice**: every difficulty switches to the gun that suits
  the distance. Normal and up can draw the other gun instead of reloading;
  puts rockets away close up and the sword away for a far enemy;
  reloads in a lull or when the enemy is out of reach; goes for a
  power weapon (sniper, rockets, shotgun, sword) within 25 when out of
  a fight, dropping the worse gun for it; dry, goes for the best
  nearby gun, not merely the nearest.
- **Targets** (`tactics::choose`): the nearest and most in front
  (distance plus 4 per radian off the aim), sticking with the one it
  has (its cost times 0.6), so a close threat takes over from a far
  target but two at about the same range do not flip-flop. An enemy
  back in view within 1.5 s of being seen needs no new reaction (so a
  strafe past a pillar does not freeze it); a new one
  in the middle of a fight takes half the reaction.
- **Aim.** Lead uses the held weapon's projectile speed (400 a second
  for a bullet) in place of 90 for every gun. Heroic and Legendary aim at the head with a
  precise weapon or a sniper rifle, and at the feet with a blast
  weapon; Easy and Normal at the chest.
- **Grenades** (all difficulties): none at a target with a teammate
  within 3 of it.
- **Pre-aim** (Normal and up): chasing, it looks where the enemy went
  out of view (within 15, in clear sight, and not more than 45 degrees
  off the way it walks), else into the coming corner.
- **Signals for the movement** (`Mind::fight`): `fight.band`, the
  range to keep the target at (the held gun's band; arm's length with
  nothing to shoot), also `arms::preferred_range(&arms)`; and
  `fight.wants_cover`: shields down, or reloading or switching with
  the enemy within the band.
- **Not traced, wanted next:** whether a shot hit or was aimed at the
  bot, a projectile's
  gravity for lobbed weapons, the energy sword's lunge range from its
  tag, and the flag, ball and bomb tags' aim assist (assumed none).

### Weapon skill

Grenades, ranges, combos and using both guns. The
code is `bots::grenades`, `bots::weapons`, `bots::arms`,
`bots::tactics`, `Walls::hit` in `bots::los`, and small hooks in
`bots::mind`.

- **Grenades fly** (`grenades::fly`). From the eye along the aim, under
  gravity, in steps of 1/60 s against the map's collision triangles
  (`los::Walls::hit`: where along the step and the surface's normal; a
  flat floor at the target's height when the map has no data). A frag
  bounces (35% of its speed into a surface comes back, 30% of its speed
  along it is lost at each touch; slower than 0.4 on a floor it lies
  still) and goes off 1 s after its first touch; a plasma sticks to
  the first surface or player it touches (a player is an upright line
  0.1 to 0.65 over the feet, 0.25 thick) and goes off 1.2 s later. Halo
  2 has no cooking: nothing the bot does moves the fuse. **None of
  these numbers is read from the game** (below); they are named
  `ASSUMED_GRAVITY` (3.2 world units a second squared), `FRAG` (6 a
  second) and `PLASMA` (5.5 a second) for a run to check.
- **The aim** (`grenades::solve`). Pitches from -20 to 70 degrees in 5
  degree steps, the best refined by 1 degree; the throw goes off
  nearest where it should, within 1 world unit or not at all. A moving
  target is led (its velocity times the flight time, then again with
  the flight time of that throw). With none landing straight and banks
  allowed (Heroic and up, also Normal at last-seen cover), tries 8 to 60 degrees to either side: off a
  wall, or rolled past a corner to where someone went out of view. A
  plasma that sticks the target counts as a hit. The arc blocked, it
  does not throw (tries again in 2/3 s).
- **When and what** (`grenades::Thrower`, the mind's `grenade`). The
  ordinary rate increases for a cluster, recent cover or a closing enemy
  (§3.4). It aims between members of a group and favors a frag. Normal
  and up can stick a lone visible opponent within 8. Black changes
  grenade type when necessary. A blast within 3 of the bot or a
  teammate cancels the throw on every difficulty. It turns to the
  throw (the mind looks along it) and lets go once its aim is within
  the skill's tolerance, giving up after 1 s; a human hand: the throw's
  aim is off by the skill's noise. At most one every 8 s.

  | | Easy | Normal | Heroic | Legendary |
  |---|---|---|---|---|
  | banks and rolls | no | at last-seen cover | yes | yes |
  | sticks plasmas | no | yes | yes | yes |
  | lead | 0 | 0.5 | 0.85 | 1 |
  | aim noise (degrees, std. dev.) | 6 | 3 | 1.5 | 0.8 |
  | lets go within (degrees) | 8 | 5 | 4 | 3 |
  | spares teammates | yes | yes | yes | yes |
  | more before pushing in | no | yes | yes | yes |

- **Weapon ranges** (`weapons::reach`). A gun's band and fire range
  are its role's (§3.6) refined by its tag's aim assist reach
  (`cone_range`, `pull_range`): a magnum, shotgun, plasma or
  rifle or SMG is good to its reach (kept between half its role's
  band end and the role's band end). A sniper or splash weapon starts
  with its role's band. Every gun fires to 1.25 times its band's end
  (`weapons::FIRE_PAST_BAND`), one rule for all classes. The first
  barrel's projectile caps all firearm ranges at the nearer positive
  value of its air damage endpoint and maximum travel. With both
  fields unset, the role and aim assist decide. The movement band
  shortens when the projectile requires a closer shot. The aim point
  needs a clear line from the bot's eye, including for the off hand
  and a passenger using a carried gun. A sword's lunge uses its aim
  assist reach (1 to 3, or 1.5 when unread), and it swings at that.
- **Using both guns** (`arms::plan`, Normal and up, out of a fight).
  Two short guns or two long ones: a gun of the other kind within 15
  is taken, the worse of the two dropped for it (switching first when
  the better is in hand). A full magazine in hand and the other gun
  wanting a reload: switch to it, so it is reloaded out of the fight.
  In a fight, the switches by range and instead of a reload (§3.6)
  use each gun's reach.
- **Combos** (`weapons::Combo`). With a plasma pistol at or above 15%
  battery, a loaded precision weapon and a visible opponent 1.5 to 14
  units away, the Clanker rolls the name's combo probability once for
  the encounter. Briefly losing sight releases the controls and keeps
  the roll. If needed it draws the pistol, charges for 24 ticks, then
  releases on aim or at 36 ticks. It draws the rifle or magnum after
  the skill's switch delay (18, 10, 6 or 3 ticks on Easy through
  Legendary). A visible shield break brings that switch forward, and
  every difficulty aims its loaded precision weapon at the head if
  that point is visible. The follow-up holds the weapon choice for up
  to two seconds. The melee finish also uses recent pulls on target
  (5 on Normal, 4 on Heroic, 3 on Legendary, one per 8 ticks for an
  automatic).
- **Dual wielding** (unit `+0x213`, the left hand's slot, traced in
  docs/live/live-gamestate.md and `boot::touch_glue`). The left gun
  reads as `Arms::left`; the left trigger fires it with the right's
  aim (`weapons::LeftHand`: a charge gun charges and lets go on target,
  a once-per-pull gun is pulled at its rate). An automatic in the left
  hand goes down and up with the right trigger when the right is held
  on an automatic too, and otherwise on the right's hold pattern and
  hysteresis (§3.4). An empty left magazine with rounds in reserve gets
  one short pull to start its reload, again after 1.5 s if it is still
  empty. No grenades then.
  The pickup policy, hold-Y action and grenade trade are traced in §3.6.
- **Not traced, wanted** (addresses for a tracing lane):
  - the globals (`matg`) grenades block: its offset in the globals tag,
    stride, and each entry's projectile reference (then the frag's and
    plasma's `proj +0x17c` initial velocity, already confirmed, gives
    the throw speed);
  - the gravity the projectile update takes off a projectile's z
    (`0x000f9726`, docs/live/live-telemetry.md): the global it loads
    and the `proj` field that scales it (air gravity scale);
  - in `proj`: the detonation timer and whether it starts at the first
    touch, the material response block (offset, stride, the
    perpendicular and parallel friction or restitution fields), and the
    sticky flag;
  - how a throw leaves the hand: the projectile's spawn point and
    direction against the unit's aim (`+0x168`) and whether the unit's
    velocity is added;
  - the plasma pistol trigger's charging time (the `weap` trigger
    block, `+0x40` stride; which field);
  - the damage effect radius of the frag and plasma explosions (`jpt!`
    upper radius) for the safety distance (3 assumed).

### Natural feel

Aim, walking and buttons are driven by
the simulation tick with separate seeded random streams. The full
pad is reused on repeated frames within a tick.

- Aim ramps into flicks and corrects overshoot. Tracking trails a fast
  crossing more than a slow one, and brief wall occlusions discard
  velocity. A gunshot may draw a delayed, coarse glance. Visible
  enemies alone supply live targets and shot lead.
- The feet accelerate in world space, vary strafe deflection and
  ease off near a destination. Cameras look into clear turns and
  doorways before the feet finish the turn, and on a ramp pitch toward
  the next waypoint (`look::SLOPE_PITCH`). The navigation follower
  checks the corner cut for footing and keeps gap approaches at pace.
- Bots crouch-walk on quiet objective approaches. Hits alone can
  change the strafe or start a crouch at range. Jumps are single
  short presses on planned routes, during stuck recovery, at the
  start of a strafe, on an open run as the name likes (§3.2, "Pace and
  hops"), or while escaping a visible nearby grenade.
- Weapon taps have a short intent delay, a hold and a release gap.
  Melee needs a sustained chance in front of the bot. Semi-auto
  pulls vary within the weapon's cadence and bursts start when the
  bot lines up a shot.
- Small right-stick fidgets happen when an actor is standing idle
  and on signed-in local pads in a network pregame lobby or while
  awaiting a unit. They send zero left stick, triggers and buttons.
  The retail action builder applies the turn curve and translates the
  hand's button timing as described in §5.1.
- Seeing a live grenade and getting clear of it is `bots::incoming`
  (`bots::grenades` owns throwing). The live grenades
  come from the same object scan as the pickups and supplies
  (`arms::ground`, with the bot's feet). In a fight the aim
  hand also carries a throw: while `grenades::Thrower` aims, the hand
  tracks the throw's line instead of the target. Only a throw, not the
  left gun's trigger when dual wielding, holds off the right trigger
  and the weapon taps.
- Focused unit-test filters are `mind`, `tactics`, `moves`,
  `nav::tests`, `incoming`, `sound` and `phantom::session` with
  `cargo test -p bots --lib`. The game-run checklist is §11.1,
  "Natural feel".

### Team decisions

Local slots and hosted phantoms on the same team exchange target calls. Each
bot hears completed ticks, so polling order and repeated rendered frames at
one tick give the same claims. Calls expire after 2.5 seconds.

If two healthy bots have comparable targets, the higher player slot yields
the contested target. A hurt teammate can call for focus fire.

A recent friendly death makes its visible killer a trade opportunity for
three seconds. In free-for-all, a visible pair aiming at each other presents
an opening to a third player.

A visible numerical disadvantage prompts withdrawal. Weapon minimum
ranges still apply. CTF and Assault carriers try routes away from
recent sightings. The bot's own death and visible teammate deaths can
mark a dangerous approach for up to 20 seconds across respawns.

See §3.2 for the host perception boundary and supply beliefs, and §3.6
for the pickup observers. Map loads and tick rewinds clear the radio,
supply observations and danger memory. The coordinator's unit-test
filters are `decisions`, `objective::tests`, `arms::tests::decisions_`,
`guest::tests::decisions_` and `mind` under `cargo test -p bots --lib`.
The in-game checks are in §11.3.

### Stage 8: Clankermatch

Partly built (§8.3, §8.4; run checklist §8.5).

- Built: the Live menu row (row 2; the rest moved down), the playlist
  filter at `0x00192db0`, the clanker hoppers in the default set and
  `GET /v1/hoppers/clankers` (both homes), the empty search with its
  delay, the lobby fill from the playlist's roster (one at a time,
  balanced teams, skill from the server), `kind: "clankermatch"` in the
  record and in the server's read model and stats (both homes).
- Also built: the ⚙ glyph in every font and the marked bot names
  (§8.2.2), Quickmatch kept off clanker hoppers (§8.3), the cursor
  back on the browser's and co-op's rows (§8.3), the balance around
  the host's real team (§8.4).
- Next:
  1. A party of more than one human (the balance takes the host's
     player only).
  2. The search delay from the playlist's population; continuous skill
     in the brain (§8.2.3: the brain still takes the nearest of four).
  3. A clanker game's rank needs an arbitration round as ranked
     matchmaking does: check whether the console registers one in an
     unranked clanker hopper, or have the server rank without (with
     sbmm-ranked).
  4. `MatchProposed` with `host = me` and the bots as members, when
     the matcher exists (sbmm-ranked).
  5. Clanker marks for a published hopper file (an admin mark, as
     pins have): today only the default lineup's clanker playlists,
     kept where a published file has the same playlist id at the same
     index.

### Stage 9: Vehicles

Seat entry and boarding from live tag
markers, rocket lock-on, chassis clearance for ground routes, Banshee
flight and turret take/leave decisions. Fixtures live under
`vehicle::` and `mind::ride::`. The game-run checklist is in §11.4.

## 10.1 Library surface (for the agent API)

- `bots::phantom::add(Identity, team: Option<u8>, Box<dyn PadSource>)`
  registers a phantom. It joins a System Link or Xbox Live session this
  console hosts (§5.1). `Identity` holds the XUID and full gamertag,
  which determines the phantom's appearance.
- `bots::phantom::remove(handle)`, `list()` and `player(handle)` use the
  process-wide registry. `list()` returns each `PhantomInfo` with its
  handle, identity, team, machine id, optional player index and
  pending, joined or failed state.
- `bots::pad_source::PadSource` produces one `HostPadState` from a
  `Frame`. Phantom gather samples a spawned player's source once per
  fresh simulation tick. `Frame.frame` counts samples and `Frame.tick`
  carries the simulation tick. The source can find its player through
  the phantom's machine id with `bots::guest::perceive`. A source must
  keep registry operations out
  of its `pad` callback.
- `bots::pad_source::Scripted` holds pads for timed steps and can loop.
  `bots::pad_source::Recorded` plays each supplied pad for one sample,
  then idles. `bots::Brain` uses the loaded map's nav mesh and collision
  data. A closure taking `&Frame` can also implement `PadSource`.

See [the agent API](../runtime/agent-api-sim.md#sim-simulated-players-and-direct-state)
for script, recording and brain sources.
