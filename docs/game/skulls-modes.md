# Skulls: effects, multiplayer and Firefight

Where each effect lives, and how skulls work in multiplayer and Firefight. Part of [skulls.md](skulls.md).

## Where each effect lives

Every read of the array, from an absolute-reference scan of the XBE (the
only writer is `0x1df3f4`), with the function around it read in
`h2 disasm`. Confirmed unless noted. Every one of them tests the active
options mode (`[[0x4e6948]+8] == 1`, campaign) first, in the reader or in
its only caller, so retail runs no skull in multiplayer (mode 2) or the
main menu (mode 3), whatever the array holds.

| n | Skull | Read at (function) | What it gates | Scope |
|---|---|---|---|---|
| 0 | Envy | `0xc50fd` (`0xc50a0`, campaign-only as a whole), `0xc579d` | the player's flashlight becomes active camo | simulation (player unit) |
| 1 | GBP | `0xca13f` (`0xca0b0`) | an effect spawn (community: confetti on a headshot) | simulation, effect only |
| 2 | Assassins | `0xc6841` | AI units get the camo flag | AI |
| 3 | Thunderstorm | `0x203759` (`0x2036c0`) | AI characters take the next rank's variant | AI |
| 4 | Famine | `0x1e2e50`, `0x1e2ed6` (`0x1e2a90`); `0xfda54` (`0xfda00`) | AI weapon drops' rounds scaled; a weapon made from a scenario placement starts with the placement's rounds (`+0x4c`, `+0x4e`) halved (`cdq; sub; sar`) | AI drops; simulation for placed weapons |
| 5 | IWHBYD | `0x20b27b` (`0x20b140`) | which dialogue line plays (rare ones far more often) | AI dialogue |
| 6 | Blind | `0x1393e1` (`0x13939b`); `0x165f31` (`0x165f16`) | the HUD draw predicate returns 0; first-person weapon render (stdcall, 4 args, `ret 0x10`) returns 0 | local (what this machine draws) |
| 7 | Ghost | `0xc986b` (`0xc9770`) | the flinch cap that applies to players also applies to AI | AI |
| 8 | Black Eye | `0xd5e35` (`0xd5de0`); `0xd9770` (`0xd9640`) | shields do not recharge on their own (`[unit+0x104] = 10`); a melee hit adds shield, up to 3.0 | simulation |
| 9 | Catch | `0x1ae51f` to `0x1ae7b1`; `0x1e2d5b` | AI grenade decisions and drops | AI |
| 10 | Sputnik | `0xd9923` (`0xd9640`) | damage impulse ×3.0 (`[0x45dbd4]`), mid-function | simulation |
| 11 | Iron | `0x1388ae` (`0x1388a0`) | co-op respawn: a death reverts to the checkpoint | campaign co-op only |
| 12 | Mythic | `0x1e0578`, `0x1e0ead` | AI vitality ×2 | AI |
| 13 | Angry | `0x1fda15` (`0x1fc7e0`) | AI actor fire rate ×2 | AI |
| 14 | Bandana | `0x25cc4d` (`0x25cb60`) | actor code sets a unit-tag short at `+0xc2` to 2; the community effect is bottomless ammo (likely, not traced to the ammo path) | AI code path; likely player ammo |

Scope: **local** changes only what one machine draws; **simulation**
changes what the host computes and every machine predicts; **AI** needs
AI, which multiplayer maps do not run; "campaign" is campaign-only logic.

## In multiplayer

Status: built, tests only; not seen running.

### What is offered

Only the skulls whose effect can be made to work in a multiplayer game
without patching a retail function's middle:

- **Blind** (local). `before` hooks on `0x13939b` (`HUD_SHOWN`, returns
  `AL = 0`, `ret`) and `0x165f16` (`FIRST_PERSON_RENDER`, returns 0,
  `ret 0x10`): the same returns the campaign path takes with the skull on,
  without the campaign test.
- **Famine** (simulation). Around `0xfda00` (`WEAPON_FROM_PLACEMENT`,
  stdcall `(object, placement)`): `before` halves the placement's `+0x4c`
  and `+0x4e` (truncating toward zero, as the campaign path's
  `cdq; sub; sar` does), `after` puts them back, so the scenario's data is
  as it was and the next weapon from the same placement is halved from the
  original count. Only `+0x4c` is read by `0xfda00`; `+0x4e` is halved for
  symmetry with the campaign path. AI drops are not touched (no AI).

Not offered: Sputnik and Black Eye act in the middle of `0xd9640` and
`0xd5de0` (each would need a port of the function or a mid-function hook);
Envy reads inside a function that is campaign-only as a whole; the rest
need AI or are co-op campaign logic.

- **Low Gravity** (simulation), one of ours: the physics constants'
  gravity, halved on every machine ("Our skulls" below).

`live_proto::skulls::MULTIPLAYER` (`[4, 6, 16]`) is the list, and the server
refuses a multiplayer game or session that names any other.

### Choosing them

A "Skulls" row is added to the game variant editor's category list
(`variant_editing_options`, screen 178: Match, Player, Team, Game Type,
Vehicle and Equipment Options; feature `skulls-mp-row`,
`boot::skulls_mp_row::VariantRow`). A on it opens the multiplayer Skulls
screen: a screen of its own (a second `SkullsScreen`,
`SkullsScreen::multiplayer`, on its own list class `skulls-mp-screen`),
laid out as the campaign page is ("The page" above): titled "SKULLS",
one row per skull offered (Famine, Blind and Low Gravity), a collected
one with its multiplayer value ("Famine: Off") and A opening the value
picker, a locked one its name alone with A doing nothing, the same
legends, and help saying what it does in multiplayer, that the host's
choice is everyone's, and on its own line where it is found or that it
is locked and where to find it. Three rows leave the list's lower part
empty; the list is the level select's, as the campaign page's. The choice is per
profile, apart from the campaign one: `skulls.mp_famine` and
`skulls.mp_blind` under `[profiles."<name>".skulls]`.

**Collecting is the rule.** A skull is locked on the multiplayer screen
until that profile has picked it up in the campaign, and A on a locked
one does nothing. There is no setting to waive it (the
`skulls.mp_require_collected` setting is gone); a value left in
`settings.toml` for a skull the profile has not collected stays locked and
is never applied or advertised. Guests apply the host's advertised set
whatever their own profiles hold.

**Not stored with the variant.** The choice stays per host profile and
travels with the Live session. The variant's record (the 0x130-byte game
variant the editor edits at `0x54e4a0`, its name at `0x54e4a4`) has room of
ours, the extension block in its spare bytes (docs/game/game-options.md
section 3), but the skulls are not stored in it.

| Address | What | Confidence |
|---|---|---|
| `0x23764f` | the variant editor's loader, screen id 178 (`ui\screens\game_shell\settings_screen\variant_settings\variant_editing_options`), constructor `0x23768d`; opened by `0x2387d8` and `0x2ba9d0`, both with window 5 (the main menu's stack) | confirmed |
| `0x2b74c3` | its list handler constructor (ESI the handler at screen `+0x614`, `ret 4`): datum array "variant editing options list" (`0x45be10`) of 6 rows, each row's word (`+2`) set to its index by a loop over the count; six item widgets | confirmed |
| `0x2b7581` | row text (list slot 20, thiscall, item and word, `ret 8`): a table of six `(word, string id)` pairs through `0x24c75c`; a word not in it leaves the text as it was | confirmed |
| `0x2b75e8` | row chosen (the press record's function, thiscall, chain and row pointer, `ret 8`): words 0 to 5 open a category's screen (`0x2c83e6`, window 5); any other word returns at `0x2b7711` having done nothing | confirmed |

The shared datum array hook grows "variant editing options list" from 6 to
7 when the row is on, so the constructor's loop gives our row word 6. Our
`before` on `0x2b7581` draws "Skulls" for row 6; our `before` on `0x2b75e8`
opens the multiplayer screen for row 6 and returns (`ret 8`), and leaves
rows 0 to 5 to retail. The tag has one pane per category and none for
ours (`0x230427` does not check), so the pane hook keeps the last
category's pane, its help text included, while our row is focused. The
list has six item widgets for seven rows, so it scrolls.

### Why the Game Setup row broke

A row on the lobby's Game Setup dialog (`squad_settings_dialog`, screen 23)
that opens the screen the way the difficulty row does, with
`0x149f1e(1 << player, loader)`, shows the campaign screen. Read
statically, the cause is the screen stack it opens on:

- A screen request (`0x149f49`) carries a window at `+4` and a slot at
  `+8`. The base screen constructor `0x22f5ca` keeps them at screen
  `+0x74` and `+0x78` (screen vtable slots 20 and 21 return them,
  `0x23012c`, `0x230130`).
- The push `0x147f6d` picks the stack by the window: 5 is the per-player
  array at `0x54d62c` (stride 0x40), 3 the one at `0x54d76c` (stride
  0x38), 1 `0x54d884`, and 0, 2 and 4 single stacks. The generic back
  handler (`0x2300ea`, B or Back) closes the screen through `0x14800c`
  with the same window and slot.
- `0x149f1e` always says window 5, slot 4. The Game Setup dialog is opened
  on window 3 (`0x25102e`, `0x2c7eef` both push 3), and every screen it
  opens itself (Change Map `0x2bb8ac`, the variant list `0x2bb8df`, and the
  others in its jump table) says window 3 too.
- So a screen opened that way goes onto the main menu's stack (window 5),
  under the lobby's dialogs, not over the Game Setup dialog: the screen
  shown is the main menu's (where Campaign is), and B closes it on window
  5, back to whatever the main menu's stack held.

So the row is in the variant editor (where the variant's settings are
edited), and our press opens the screen on the pressing screen's own
stack: it reads the variant editor's window and slot
(`+0x74`, `+0x78`) and builds the request itself
(`skulls_mp::open_request`, written to a page of ours and handed to the
class's loader), so it is right on a lobby's window 3 as well as the
main menu's 5. The multiplayer screen is its own instance and class, not the
campaign screen in another mode.

### How the host's set reaches guests

Retail's multiplayer simulation is host-authoritative, and the game
options every peer builds at session start (`0x6ec80`) carry no skulls, so
the set travels beside the game, through our Live session data:

1. **The host advertises it.** When the game advertises a session
   (`XSessionCreate`, `XSessionModify`), the HLE runs the boot's probe
   (`Hle::set_skulls_probe`): the union of the signed-in profiles'
   multiplayer choices, from a cache (the probe runs with guest memory
   held, so it never touches the settings store). It goes on the wire as
   `NewSession.skulls` and, when it changed, `Update.skulls`
   ([live-api.md](../live/live-api.md)). A press on the Skulls screen pushes an
   update to an open advertisement at once (`Hle::set_session_skulls`).
2. **A guest reads it.** The HLE remembers the skulls of every session it
   saw in a search or match result. At game start
   (`GAME_ACTIVATE`, `0x18f000`, options mode 2) a guest finds its joined
   session (the XNKID registered with `XNetRegisterKey` that is not one of
   our own advertisements), asks the server for it again
   (`GET /v1/sessions/{id}`), waits up to 1.5 s, and uses what it has
   (`Hle::host_skulls`): the fresh answer, or the set seen when it joined.
3. **Everyone applies the host's set** for that game. The host, and a
   machine playing alone, apply their own choice. A guest applies only the
   host's, never its own. Telemetry's `NewGame.skulls` is the set applied
   ([live-telemetry.md](../live/live-telemetry.md)).

The role comes from the squad session (`coop_probe::read_session`): no
live session or one machine is local, our peer the host is host, anything
else is a guest.

Limits:

- **System Link and no server.** No advertisement reaches the guest, so it
  applies none and says so. The host still applies its own: Blind on the
  host's screen only, Famine to the weapons the host makes (the host
  decides objects; whether a guest's own prediction of a fresh weapon's
  rounds then shows the full count for a moment is not checked).
- **A change after the game starts** applies at the next game.
- **A modified client** can ignore the host's set for Blind (it only
  changes its own screen). Famine is enforced by the host.
- **Older servers** refuse a session update carrying `skulls`; the HLE
  sends the field only when the set changed, so a host with no skulls
  never sends it.

### Switching it off

- `H2_FEATURES=skulls-mp=off`: no multiplayer skull is applied, nothing
  is advertised, and no AI skull's campaign gate is answered in Firefight
  (`skulls-mp: off: switched off (H2_FEATURES)`).
- `H2_FEATURES=skulls-mp-row=off`: no Skulls row in the variant editor.
- `H2_FEATURES=skulls-mp-screen=off`: the row stays, and says the screen
  is off when pressed.
- As with every plan hook, a clash at one of the points refuses the plan
  at boot rather than replacing a hook.

## In Firefight

Status: built, tests only; not seen running. The set, its per-machine
split and the answer to every AI reader's campaign gate are in.

A Firefight match ([firefight.md](firefight.md)) runs actors, so the AI
skulls mean something there, on the host only (`skulls_mp::firefight_ai`:
every machine but a guest). The director switches each skull on with a
`Skull` command at its round's first wave; the set grows until an endless
match's next set switches the previous set's round skulls off.
Score multipliers come from the definition and the director's active set
(`firefight::scoring::kill_points`).

| n | Skull | In Firefight | Test |
|---|---|---|---|
| 2 | Assassins | host's actors | `ai_skulls_apply_on_the_host_only` |
| 3 | Thunderstorm | host's actors | `ai_skulls_apply_on_the_host_only` |
| 4 | Famine | host's actors' drops, and placed weapons on every machine (the multiplayer path) | `ai_skulls_apply_on_the_host_only` |
| 7 | Ghost | host's actors | `ai_skulls_apply_on_the_host_only` |
| 8 | Black Eye | every machine: its gate and its array byte, in any role, and the multiplayer set's active bit off the guest | `black_eye_reaches_every_machine` |
| 9 | Catch | host's actors | `ai_skulls_apply_on_the_host_only` |
| 11 | Iron | a director rule (no respawn in the bonus round); no engine byte | `every_firefight_skull_has_a_scope_and_an_engine_index` |
| 12 | Mythic | host's actors | `ai_skulls_apply_on_the_host_only` |
| 13 | Angry | host's actors | `ai_skulls_apply_on_the_host_only` |
| 16 | Low Gravity | everyone (the multiplayer path) | `every_firefight_skull_has_a_scope_and_an_engine_index` |

`MpSkulls::write_firefight_array` sets the array byte (`0x4f55dc + n`) of
each AI skull and each simulation skull (`FIREFIGHT_SIM_SKULLS`: Black Eye)
on and clears the others it owns, so a skull the director switches off
(`Skull { on: false }`: a set's round skulls, the bonus round's) leaves the
array, the gate set and the multiplayer set's active bits
(`a_skull_switched_off_leaves_the_array`). Black Eye's shield reader has no
campaign gate of its own in `engine::points::skull_gates`, so its byte is
the whole of its reach until a run shows the reader (docs/game/firefight.md,
"Lives, rounds and skulls"). A round's skulls
(`a_round_switches_its_skulls_on`) and multipliers (`multipliers_multiply`)
are in `crates/formats/firefight/tests/skulls.rs`.

### The campaign gate

Each reader tests the campaign gate (`[[0x4e6948]+8] == 1`, or a register
already loaded from it) and jumps past its byte with a short `jne` when
the mode is not the campaign (confirmed, `h2 disasm` at each). The
`jne` falls through to the load of the skull's byte. So the gate is
answered at the `jne` itself: a `before` hook of `skulls-mp` at the site
resumes at the fall-through for a skull this machine runs, and the byte
decides as in the campaign. Everything the compiler scheduled between
the compare and the jump (x87 and SSE stores in Famine's, Catch's and
Angry's readers) has run by then, so no instruction is redone by hand.
For any other skull, and on a guest, the hook lets the `jne` run.

The sites are `engine::points::skull_gates::GATES`
(`crates/hooking/guest-layout/src/points/skull_gates.rs`); each site and
its fall-through is an instruction entry of the image
(`boot::image_entries`). Discovery makes such an entry a function entry
of the image, not a resume, so the hook enters the fall-through by entry
dispatch (`skulls_mp::gate_transfer`, `has_generated`). They are every absolute reference to an AI
skull's byte in the XBE, less Famine's placed-weapon reader `0xfda54`,
which the multiplayer path answers at its function's entry:

| n | Skull | `jne` → byte load | Function |
|---|---|---|---|
| 2 | Assassins | `0xc683f` → `0xc6841` | `0xc6810`, the active camouflage |
| 3 | Thunderstorm | `0x203757` → `0x203759` | `0x2036c0` |
| 4 | Famine | `0x1e2e4e` → `0x1e2e50`, `0x1e2ed4` → `0x1e2ed6` | `0x1e2a90`, the actor's deletion (its drop) |
| 7 | Ghost | `0xc9869` → `0xc986b` | `0xc9770` |
| 9 | Catch | `0x1ae51d` → `0x1ae51f`, `0x1ae568` → `0x1ae56a`, `0x1ae76a` → `0x1ae76c`, `0x1ae7af` → `0x1ae7b1` | `0x1aded0`, the grenade decision |
| 9 | Catch | `0x1e2d59` → `0x1e2d5b` | `0x1e2a90` (its grenades) |
| 12 | Mythic | `0x1e0576` → `0x1e0578`, `0x1e0eab` → `0x1e0ead` | `0x1e04a0`, `0x1e0dc0` (the mode in `edx`) |
| 13 | Angry | `0x1fda13` → `0x1fda15` | `0x1fc7e0` |

The set a machine answers is `skulls_mp::gate_bits` (the
`firefight_ai` set), stored in `engine::skull_gate` at each `Skull`
command and cleared at every game start. The active camouflage
`0xc6810` also runs as a native body (`engine::sim_objects::unit_rest::camo`), where the lifted `jne` is never
reached; the body asks `engine::skull_gate::passes` instead. The hooks
are optional: an image built without the gates' entries drops them with
a note, and the install line says how many of the thirteen are placed.

Tests: `a_hosts_ai_skulls_pass_each_readers_campaign_gate`
(`skulls_mp.rs`),
`every_ai_skull_gate_has_the_multiplayer_skulls_before_unless_turned_off`
(boot's plan), `assassins_cloaks_an_actor_in_the_campaign_and_where_the_gate_is_answered`
(the native body), and against the retail XBE (`H2_XBE`, ignored by
default) `each_gate_is_a_jne_falling_through_to_its_skulls_byte` and
`the_gates_cover_every_reader_of_an_ai_skull`
(`crates/apps/boot/tests/skull_gates_xbe.rs`).

Not verified: what each skull looks like on a host's actors in a match;
that the fast tier's and clang's images both carry the entries (the
install line's count says so).
