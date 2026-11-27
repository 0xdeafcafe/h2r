# Bots: way in 1, local slots

The controller-port path and what retail traces say about it (section 4). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 4. Way in 1: local slots

This section records the retired local-slot design and its retail
controller, render and audio traces. Current bots use §5.

### 4.1 The bot's pad (traced, Repo)

A bot's controller is a pad on a free port, not a change to the agent tap.
`input::InputManager::sync` (`crates/platform/input/src/device.rs`) polls every
`HostBackend` in order and puts each new pad on the lowest free port. So a
**bots backend**, added last, makes its pads appear on ports 1 to 3 after
the human's real pads. They show up in `XGetDevices`, so the game sees an
inserted controller and opens it with `XInputOpen`, and every
`XInputGetState` reads it. Two more points:

- `supports_rumble` is false, so rumble is dropped.
- The agent tap's `input.pad` still merges over the bot's pad. Tests and
  scripts can use that to steer a bot.

The brain gives the local backend a pad report after its motor delay.
The backend's `poll` hands it to the host's regular controller input path
without an additional network delay.

### 4.2 Signing a bot in (traced)

**The slot flags.** In a local slot (`0x54e8e0 + i*0xc70`), `+0` holds two
flags:

- `0x10`: a profile is loaded. It is read by `0x1902c1`, which returns the
  name at `+0xc1c`, and set by `0x18fb34`.
- `0x20`: the slot is signed in to Live. It is set and cleared by
  `0x190074`. The Live account record is at `+0x470`; `0x1900a5` tests its
  `+8 & 3`.

**The sign-in function.** `0x18fb34(slot, block, file)`, stdcall, `ret 0xc`,
signs a profile into a slot:

- If the slot had no profile, it initialises `+8` (`0x14b410`) and sets
  `+4` from `0x64610`.
- It copies the 0x1e0-byte block (0x78 dwords) to `+0x18`, sets `0x10`, and
  stores `file` at `+0x1f8`. `-1` means no saved file.
- It then builds the controls table with `0x190c34` (eax = slot), and runs
  `0x190eb3`, `0x53810` and `0x54fc0` with values from the block (`+0x148`,
  `+0x14c`).

Its callers are all "a profile was chosen" paths: the profile UI
(`0x24b6ba`, reached only when the chooser's file at `+4` is not -1, and
`0x24b75b` after loading a file), the settings commit (`0x18fd0c`,
`0x18fd59`) and `0x148b6b`. The sign-out is `0x18fc08`.

**The file, and what "in use" means (traced 2026-11-19).** `file` is a
saved-file handle from the saved-file manager (`[0x51ea14]`: the
built-in content first, at most `0x65` records of `0x1e4` bytes, then per
device a list of up to `0x1000` saved files of `0x40` bytes from `+0xbef8`,
`0x40004` apart). A slot's handle at `+0x1f8` is everything that ties it to
a saved profile:

- **Chooser lists.** The list builder `0x24a190` takes the saved files of
  a type (`0x215900`) and leaves out each one a signed-in slot's `+0x1f8`
  holds (`0x18fc75`; a handle with bit 21 set, as -1 has, never matches).
  So a slot holding a saved profile hides it from every chooser.
- **The open-profile list** (`0x55c160` count, entries of `0x44` bytes at
  `0x55c164`, room for four before `0x55c274`). The sign-in appends the new
  handle when it is not -1 and differs from the slot's (`0x18fbd1`, no
  bound check); `0x2172a0` removes one (the sign-in's old handle,
  `PROFILE_SIGN_OUT`). The menus' memory mode keeps the saved-file manager
  (`0x18ef00`'s table at `0x453c00`: `0x2157e0` builds it in mode 1,
  `0x215810` frees it and freezes the list in the others); back in the
  menus `0x18f5e3` re-finds each slot's handle (`0x2170e0`).
- **Saving.** A profile commit (`0x18fcc4`: the block changed) signs the
  block in again with the slot's own handle and sets slot flag `0x40`;
  the save passes `0x190e37` and `0x190e71` write each such slot's block to
  its file (`0x1a0660`, `0x2153dd`), and `0x1a0660` returns at once for -1.

A bot's `file` is always -1 (`guest::NO_FILE`), so it never hides, holds
or writes a saved profile. Nothing on the sign-in or sign-out path deletes
a save: the game's `XDeleteSaveGame` callers are a failed create's clean-up
(`0x215c00`), the profile UI's delete (`0x215e60`) and `0x2ad480`. A new
profile is refused with error 1 (dialog `0x4a`, `0x148ca8`) when a
device's list or its `XFindFirstSaveGame` count is `0x1000` or more
(`0x215c55`, `0x215c65`, `0x217590`).

**Never a saved profile (2026-11-19).** The user's report: the profile
list showed only "Default" (a built-in) and a new profile was refused,
and they thought the bots had taken their profiles. The data root's saves
were untouched (one profile, unchanged since before the bots signed in;
no save directory made or removed since). The guards that keep it so:

- `guest::sign_in_then` and `guest::sign_in` never call the sign-in on a
  slot that has a profile: over a saved one it would detach the user's
  file and put the bot's block in its place;
- a bot's slot that holds a saved profile anyway (a chooser opened for
  its controller and took a stray A, as bots used to tap A before the
  glue signed them in) is signed out once no game runs
  (`local::release_step`, `guest::saved_profile`): `SLOT_SIGN_OUT` gives
  the handle back to the open-profile list and saves nothing, so the
  profile is offered again before a save pass can write the bot's block
  over it, and the join gate signs the bot in again with its own block.

**The players screen does not help.** In `0x1e`'s join path (`0x2528e5`):

- `0x1900ff(port)` counts the other slots that are signed in to Live with
  the same account (`+0x470`). If there are any, it opens dialog `0x88`.
- Otherwise it opens the profile UI for that port: `0x24b6ea` (or
  `0x24b779` in mode 4).
- Then it sets the port's bit in the screen's mask (`or [screen+8], 1<<port`).

So "press A" on a bot port opens a profile chooser that the bot would have
to navigate, and it could pick a real profile.

**Design:** the bot driver signs the bot in itself. On the players screen
`0x1e`, it calls the game's `0x18fb34(port, block, -1)` with a guest call
(`ExecCtx::call_guest`, as `coop_live` and `level_boot` use), then sets
the port's bit in the screen mask.

The block is a copy of the human's block (slot 0 `+0x18`) with these
changes:

- the profile name at block `+8` (UTF-16, 32 units; the profile file's
  `+8`, "Halo0001" for the user's) set to the bot's name. The game rebuilds
  the slot's display name `+0xc1c` from it (`0x190eb3`: slot `+0x20`, 32
  units) at sign-in and whenever the slot's Live state or account changes
  (`0x190074`, `0x18fee9`), so the name the driver writes to `+0xc1c` alone
  did not last. In the user's Xbox Live lobby every bot showed as the
  human's "Halo0001", and the session's unique-name pass (`0x7f660`, from
  `0x65e37`) renamed the copies from the game's default names (`0x19fd00`,
  string ids `0x02000320` "Shadow" and `0x0200032b` "Wilshire");
- the 16-byte field at block `+0x48` (single-byte characters: `0x190001`
  copies 16 and zeroes the 16th; the profile UI's commit fills it at
  `0x24b9ce`, and a saved profile has it empty) also set to the bot's name;
- the control fields set so the brain's sticks mean the same on every
  machine (block `+0xfc` = slot `+0x114`, the record `0x190c34` reads):
  - flags bits 0 and 2 cleared (look inversion and the other flag `0x190c34`
    reads);
  - layout `+0x100` = 0 (Default);
  - thumbstick `+0x101` = 0;
  - look sensitivity `+0x102` = 3.

  The brain's aim spring is tuned against that sensitivity.
- the look at block `+0x118` (colours and emblem) set to the bot's own,
  by full gamertag (see “Clanker appearance and Live player options”).
- The name the game shows comes from slot `+0xc1c` (UTF-16, 32 units,
  `0x1902c1`). The driver writes it after the sign-in.

Because the file is `-1`, nothing is saved. The bot leaves through
`0x190d4b` (`SLOT_SIGN_OUT`, slot in EAX), called the same way, once
it is removed and no game runs; its pad is unplugged only after that
(stage 7, "a lower count, or Off"). `0x18fc08` alone (`PROFILE_SIGN_OUT`)
is not enough: it only zeroes the block, clears `0x10` and sets `+0x1f8`
to -1, so the slot keeps its local user and Live identity, and the
lobby's session keeps its player ("Leaving the lobby (traced)" below).

Whether a signed-in slot with its mask bit is enough for the game to make a
local player in Split Screen, System Link and Live custom games is the
first Run item of stage 1.

**Xbox Live guests (traced).** On an Xbox, every profile in an Xbox Live
lobby is signed in to Live: the account's own player, or a guest of it
("afr(1)"). The profile UI's Live commit (`0x24b971`, once the logon task
succeeded) runs `0x190074(port, 1)` (sets the slot's `0x20`), the profile
sign-in, `0x18fee9(port)` with the account record in EAX, `0x190001`
(the gamertag into block `+0x48`; nothing for a profile with no file) and
`0x190728`. `0x18fee9` copies the `0x70`-byte record (`XONLINE_USER`) to
slot `+0x470`, rebuilds the display name (`0x190eb3`), takes a local user
if the slot has none (`0x64610`), copies the record's first 12 bytes (the
XUID: user id, then user flags) to slot `+8`, and gives them to the slot's
local user (`0x64820`: local users at `0x4cd904`, 4 of `0xd0`, `+0` in use,
`+1` the id). A guest's record is its host's with the guest number (1 to
3) in the user flags' low two bits: `0x1900a5` tests `flags & 3`,
`0x190eb3` names a guest "gamertag(n)" (string `0x2d000231`), the
duplicate-account check `0x1900ff` (dialog `0x88`) skips guests, and when
the host's logon fails the profile UI signs out every Live slot with the
same user id (`0x190186`), guests included.

**Design: the bots are the human's Live guests.** When a human is on Xbox
Live as their own account (`guest::live_host`) and no game runs, the glue
signs each bot whose slot has a profile and is not on Live in as the
human's guest (`guest::go_live`, every 30 frames before `SOUND_UPDATE`,
`bot_glue::to_go_live`). It writes what `0x190074` and `0x18fee9` write:
the `0x20` flag, the host's record with the lowest free guest number
(`guest::free_guest_number`) at `+0x470`, its XUID at `+8` and in the
slot's local user. It leaves out the name rebuild, so the bot stays
"Clanker 1" (not "afr(1)"), and makes no logon: the Xbox Live service, and
our server through the Live HLE, only ever see the human's account (the
HLE's `XOnlineChangeLogonUsers` maps a guest to its host's account by user
id and keeps the guest number; it makes no accounts). Offline and in
System Link with no one on Live, nothing changes. Telemetry finds the bots
by their controller ports (`Bots { ports }`), so a guest goes out as
`bot: true` with no xuid, never as the human. The log says it once per
change: `bots: Clanker 1 on port 1 is on Xbox Live as guest 1 of port 0's
account`.

**Design (§9.5): a bot with a bot account is its own Live player.** The
guest sign-in above is now only the fallback until the server leases the
bot an account (or when it leases none). Once the Clankers driver has a
roster for the hosted session, each local bot takes one of its bot
accounts (`Local::set_account`), and the glue signs it in as that account
(`guest::go_live_as`): the same writes as for a guest, with
`guest::bot_account_record` (the host's record with the bot's xuid, user
flags with **no guest number**, and the bot's gamertag at `+0xc`, at most
15 bytes and a NUL) in place of the guest's. A bot on Live as a guest, or
as an older roster's account, is moved to the new one
(`bot_glue::to_switch_account`); the slot's name becomes the gamertag.
Game-side checks, from the traces above:

- **Guest numbering** (`free_guest_number`, the game's own) counts only
  slots with the host's user id, so a bot account takes none: a real
  guest ("afr(1)") still gets guest 1, with up to three bots beside it
  (all four ports are still the cap).
- **The duplicate-account dialog** (`0x1900ff`, dialog `0x88`) compares
  user ids; each bot's is its own, and a roster never repeats one.
- **A failed host logon** (`0x190186`) signs out only the slots with the
  host's user id, guests included, so it misses bot accounts: the glue
  does it instead. Out of a game, with no human on Live any more, a bot
  on Live as its account leaves (`bot_glue::to_leave_live`,
  `guest::leave_live`: the flag cleared, the record zeroed, and the
  player id its profile sign-in gave it, kept before it went on, back at
  `+8` and in its local user; its name back to "Clanker N").
- **The Live HLE's `XOnlineChangeLogonUsers`** maps each user id to an
  account in the accounts file and refused any other. It now also takes a
  bot xuid (only in the server's `0x000B` range) that a roster this logon
  leased to this console (`State::leased`, filled when `roster_bots`
  answers, cleared at sign-out), as an account's own player with no key.
  The service is never asked anything for it: no logon, no presence, no
  friends (`State::service_user` is 0 for a leased bot). Any other xuid is
  still `INVALID_USER`.

Offline and in System Link nothing changes (no human on Live: no host to
stand beside, and no roster).

**Joining in the pregame lobby (traced, the user's run 2026-11-18).**
Live, Clankers On, Count 5, hosted from the Live menu: the game started
on Ivory Tower with no bots. The players screen is never shown on that
path (Clankers is set in Game Setup, over the lobby), and the join gate
opened only on it, so every port logged `join gate port N: top none,
dialog none, signed_in false (slot 0x1): waits`. `ui_watch` reads the
lobby as no screen: it shares its window and slot with the screens over
it.

- **What retail does when a second controller joins a Live lobby.** The
  lobby's input handler `0x250b1e` (ECX the screen) takes A or Start from
  a controller with no profile (`0x250b53`: slot `+0` bit `0x10` clear)
  to `0x250c80`. With room (`0x199fd6`: 16 less the session's players),
  outside matchmaking (`0x19a902`) and in a pregame lobby (`0x19a279` 3,
  which session types 1 and 2 both give), it opens the profile chooser
  for that controller (`0x1905bf(port, …)`, screen constructors
  `0x18f42d`/`0x18f474`); otherwise error dialog 2 (`0x250ccd`). The
  chooser's commit signs the profile in (`0x18fb34`, which takes a local
  user, `0x64610`) and, on Live, logs it on (`0x24b971`: `0x190074`,
  `0x18fee9`). **Nothing calls the session from there.** The squad
  session's per-frame local-player pass (`0x65770`) walks the four local
  users (`0x4cd904`) and asks each one's status (`0x646b0`); its jump
  table (`0x65898`) is:

  | status | meaning (`0x646b0`) | action |
  |---|---|---|
  | 0 | no local user, or on a Live session (`0x6cd50` 1) a user with the offline id's flags `0xBAD00000` (`0x14b49f`, `0x646df`) | `0x65b80`: its session player, if any, out |
  | 1 | the session is not in states 3 to 8 | none |
  | 2 | no session player for it, none asked for (`+0x4cd9d0`) | `0x65a60`: ask to add it (`0x5a880`), with room (`+0x54` peers against `+0x4990`, `+0x1118`+1 players against `+0x4994`), rate-limited by `[0x4cecc4]` |
  | 3 | asked for, not there yet | none |
  | 4, 5 | a session player whose id or properties differ | `0x65c10`: update it |
  | 6 | in step | none |

  So the session adds any signed-in local user by itself; in System Link
  at once, on Live once the user has its Live identity.
- **Design: the gate opens in the pregame lobby too.** The gate reads the
  lobby from the squad session, not the screens
  (`bots::lobby::pregame`: `[0x4d8ba0]`, networking `[0x527330]`,
  session type `[0x527334]` 1 or 2, session `+0x741c` 3 to 8), and only
  out of a game (a running game keeps the session in those states).
  `bot_glue::screen` names what is up: `players` (the players screen,
  which wins, and gets its join mask), `lobby`, or the top screen's id
  (`none` when none). In the lobby the bot is signed in with the same
  `PROFILE_SIGN_IN` guest call and no mask write
  (`guest::sign_in_then(…, None, …)`); whatever sits over the lobby (the
  Clankers page, Game Setup) does not hold it back, since the sign-in
  presses nothing. Within 30 frames `guest::go_live_as` (or `go_live`)
  gives it its Live identity, and `0x65770` adds it on the next frame it
  runs. No game function past the sign-in is called: none is called on
  an Xbox either.
- The gate line names the screen:
  `bots: join gate port 1: screen lobby (top none), dialog none,
  signed_in false (slot 0x1): signs in`, then `bots: signing Clanker 1
  in on port 1 in the pregame lobby`.

**The start check (traced).** Start Game (`0x251050`, from the lobby's
handler `0x250b1e`: A or Start from a controller with a profile, A on row
0 at `0x250fb3`) acts only when the start state `0x19a2ce` is 9, and says
nothing otherwise; the lobby's status line (`0x24ffc9`) shows the state's
text. With state 9 it asks for the countdown (`0x25106c` → `0x19a78e`:
the pressing slot's local user at `+4` must not be -1, then `0x64900` →
`0x5bf10` → `0x5df30` sets session `+0x49a8`), and only when that fails
outside a lobby of session type 2 or 3 does it raise error dialog 2
(`0x236299`). A or Start from a controller with **no** profile raises the
same dialog (`0x250ccd`) when it cannot open the chooser. `0x19a2ce`'s
checks, in order (`bots::lobby`):

| state | check | text |
|---|---|---|
| 0 | `[0x4d8ba0]` and the session type `[0x527334]` (`0x19a279`), then a squad session in state 3 to 8 (`0x59670`, `0x58d70`) | "No established session" |
| 1 | the session type is not 1 or 2 | "Session not in the pregame mode" |
| 11+ | lobby mode 6, matchmaking (`0x199971`): the hopper's states (`0x19a5fd`) | "Guests are not allowed in ranked matchmade games" and the rest |
| 4, 5, 6 | each peer's map status (peer `+0x88`, `0x652a0`): loading, a peer failed, this console failed | "Waiting for everyone to load the map...", "A player in your party failed to load the map" |
| 7 | co-op modes (0, 2, 4): exactly two session players, both local; a team game: 1 to the map's most teams (`0x251139`) | "coop_takes_two", "You have too many teams (N maximum)" |
| 10 | a countdown runs (`0x64a60`) | the countdown |
| 8 | this console does not lead the party (`0x592f0`: session `+0x72d8` against `+0x50`) | "Waiting for ... to start the game" |
| 9 | ready | "Waiting for you to start the game" |

The custom lobby's status line passes a countdown string to `TEXT_SET`
at `0x25012a`. While the host's countdown is active at zero, pending
phantoms or seated phantoms in the load and simulation-start stages
select **Setting up bots…** through the shared string table
(`crates/extensions/game/src/lobby_status.rs`, `layout_glue.rs`). The
asynchronous nav cache load can finish after play starts.

So the earlier reading, that a bot with a profile but no Live sign-in
keeps the state from 9, does not hold: no check reads a slot's `0x20` or
`+0x470`. `0x59670` and `0x58d70` are about the session, and
`LOBBY_ALL_READY` (`0x592f0`) compares two peer indices (the leader
check), not player masks. A bot can only reach state 7 (as a session
player with a team) or, as a phantom peer, the map states. Signing the
bots in as guests is still what an Xbox does, and the only off-retail
state a bot was in; whether it was the cause is the game run's question.
For that, a Start press (A or Start from a controller with a profile, on
`PREGAME_LOBBY_INPUT`; or A on the lobby's Start Game row, which the
list's row handler takes before the screen sees it: `LOBBY_ROW_PRESS`,
`0x250fb3`, registered at `0x24fb40`, `ecx` the screen, stack 0 the event
record whose `+4` is the controller, stack 1 a pointer to the row word,
0 for Start Game, which calls `0x19a2ce` at `0x251051`; `ret 8`) while
bots are in logs, before the next
`SOUND_UPDATE`, the game's own start state (a guest call to
`LOBBY_START_STATE`) with the check that holds it and each bot slot:
`bots: start state 8, blocked by the party leader check: ...; port 1 0x30
Live guest 1, port 2 0x30 Live guest 2`.

### 4.3 Hiding the views (traced)

The host's views and HUD follow the local-user map. Each Clanker is a
remote peer (§5.1), so it has no local view. The local-user mapping used by
the Phantom's input builder exists only in its private copy of guest RAM.

**Listeners.** `SOUND_LISTENERS_UPDATE` fills one record per mapped user
from the user's camera and marks it active. For an unmapped user, the
update clears the unit and cluster fields but leaves the active byte.
The listener selector trusts that byte when it picks the closest listener
for a world sound. A stale active record can use its last camera position.

**Whose sound.** An attached sound starts as kind 1 (positioned).
`SOUND_OWNER_USER` walks the local-user map and tests each first-person
user's unit against the source object and its attachment chain. A match
makes the sound kind 0 (unpositioned), which plays at distance zero. The
attached sound part of an effect and a weapon's animation sound event
both pass through this check. See §5.1 for the Phantom audio hooks.

**The controller speaker, haptics and rumble (Repo).** Nothing there
goes by port:

- The controller speaker (`dsound::padroute`, `boot::pad_speaker`)
  routes by **sound tag name**, never by player: one DualSense or DS4
  speaker, chosen by device name, gets every routed sound, and the Steam
  Controller's trackpads (`input::steam_pad`, "as haptics") play the
  routed tone on every connected controller. There is no port-to-pad
  map for sound. A Clanker's attached sounds stay positioned in the
  world and play on the main output.
- **Rumble** goes by port: `XInputSetState` on a port goes to the pad on
  it (`input::device`, `HostBackend::set_rumble` of the backend that owns
  it). A bot's pad says it has no motors (`supports_rumble: false`), so
  the game's rumble for a bot's port is refused (`NotSupported`, as an
  Xbox pad with none) and never reaches the human's pad.
- **Voice (the communicator).** With a microphone chosen, every port with
  a pad used to get a communicator, bots' included, so the game opened a
  headphone for each bot and played the other players' voices to it on
  the one host output. Now a bot's port gets none
  (`voice_glue::player_pads`).

**The players screen (traced, the user's ask 2026-11-19).** "I don't want
bots to appear in the other player slots on the Xbox Live screen." The
screen's per-frame update `0x252554` (`PLAYERS_SCREEN_UPDATE`, vtable
`0x45a490` slot 3) lays out one pane per port: a port is joined (its name,
no prompt) when its controller handle is non-zero (`[0x4e61cc + 4 *
port]`, `0x18f8b8`; `0x1249d0` reads it too) and its slot has a profile
(`+0` bit `0x10`); with a controller and no profile it says "PRESS A TO
JOIN" (`0x0f00028a`); with none, "CONNECT CONTROLLER" (`0x1100028b`).

- **Design:** around that update the bots' controller handles read as 0
  (`guest::hide_pads`, put back after unless the game wrote one
  meanwhile). A bot's pane is then an empty port's, "CONNECT
  CONTROLLER", whether or not the bot is signed in: what a one-pad Xbox
  shows. Nothing else is touched, so the bots stay joined.
- **Where bots join.** The players screen is a join screen only in Split
  Screen (mode 1, `bot_glue::SPLIT_SCREEN`). In Xbox Live (3), System
  Link (2) or an invitation's mode (4) no bot signs in there: they join
  in the pregame lobby that follows (§4.2, "Joining in the pregame
  lobby"), as opponents, which is where the game wants them anyway.
  With a pregame session up under the players screen it reads as the
  lobby (no mask write). Split Screen has no networked lobby
  (`0x199df9(1, 0)` makes a local session), so its bots still join on
  the players screen, with their mask bits, and are hidden from its
  panes like any others: the screen shows the human alone, the lobby
  and the game show the bots.
- Not changed: B on the players screen (`0x25286d`) backs out only with
  one profile signed in (`0x18f911` counts slots with `0x10`, bots
  included); with bots joined it opens the human's profile menu
  instead, as before. A game run says whether it matters.

### 4.4 What a local slot cannot do

- On **Live**, extra local players are guests of a signed-in account. Run
  item: whether the game's matchmaking allows guests at all, and how many.
  Retail OptiMatch allowed split-screen in some playlists. Custom games
  allow guests.
- With a human on port zero, at most three bots can use local ports.
  All-phantom mode also reserves those three ports for humans.
- With a roster (§9.5) a local bot is its own bot account, not a guest,
  so it takes no guest number and has a page of its own on the web. Still
  only 3 per machine: the controller ports are the cap. More bots than
  free ports are phantoms, which take the roster's later bot accounts.
