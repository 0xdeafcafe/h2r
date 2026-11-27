# Live co-op: h2 coop test

Part of [Campaign co-op over Live (design; phase 0 slice, the phase 2 probe and Co-op over Live built)](live-coop.md). Covers: 12. One command on one Mac: `h2 coop test`.

## 12. One command on one Mac: `h2 coop test`

Section 10.4 takes three terminals and a list of variables. `h2 coop test`
(`crates/apps/cli/src/coop_test_cmd.rs`) does all of it and reads the logs back:

```sh
h2 coop test --disc disc/halo2.iso --auto
```

It:

1. Starts a Live server in its own process on `127.0.0.1:8075`
   (`--server-port`), with the file store under `--root` (by default
   `%LOCALAPPDATA%\h2recomp\coop-test` on Windows and
   `~/Library/Application Support/h2recomp/coop-test` on Unix), so accounts,
   friends and profiles last between runs. The web app is served there
   too when it is built.
2. Makes an account per game: Alpha hosts, Bravo joins (`--players N`, up
   to 16, adds Charlie, Delta and on through the NATO alphabet to Papa;
   past 2 the host is started with `H2_COOP_MAX_PLAYERS=N`, §13). Each game's key goes in its own Live
   directory (`<root>/<gamertag>/live`, given to the game as
   `H2_LIVE_DIR`), never in your own `accounts.json`. Every guest is made
   the host's friend. A second run reuses them; `--fresh` forgets the
   server and the accounts (not the game data).
3. Opens the host's window with `h2 play <disc> --data <root>/alpha/data
   --live-server http://127.0.0.1:8075 --gamertag Alpha`, and once it says
   `live: playing as Alpha` (after any build into `--out`) the guests'
   windows, the same way with their own data. `--dry-run` prints these
   lines, variables included, and starts nothing.
4. Once the host is at its main menu (it asks the host's agent socket:
   `session.wait {"phase":"menu","map_ticks":60}`, up to 10 minutes),
   prints one line saying what to press next, for example:
   `coop test: next, on Bravo's window: Xbox Live (to sign in). Then on
   Alpha's window: Xbox Live > Co-op Campaign > pick a level and
   difficulty; Bravo get the invitation and join by themselves; then Start
   Game on Alpha.` If the socket can't be asked, the same line comes with
   "once at the main menu".
5. Writes every line of each game to `<root>/logs/<time>/<gamertag>.log`
   and its co-op lines (`coop-live`, `coop-probe`, `coop-auto`, `skulls`,
   `session`, `game start: options`, `live: playing as`, `custom games`,
   `agent: listening`, the join's steps `live: join`, `live: qos` and
   `live: xnet`, `guest thread exited`, panics) to `coop.log` there, as `[+SECONDS]
   GAMERTAG | line`, also shown on the terminal as they come. Boot-time
   dumps that only name a co-op hook are left out: the `feature plan:`
   lines, each hook's first `feature: ... active`, `health:` lines and the
   features' `hooks installed via feature plan` summaries.
6. When the last window closes (Ctrl-C closes them; twice exits at once),
   prints the checklist and the log's path.
7. When a game stops while others run (its `app: guest thread exited`
   line, or its process ending in failure), closes the others (SIGTERM)
   and prints the checklist at once. The stopped game's window stays open
   on its last frame until you close it; a game closed normally is no
   stop.

### 12.1 What `--auto` does

The host's Live code invites the guests to its advertised lobby as soon as
it has one and they are online, and each guest's Live code accepts the
host's invitation as soon as it arrives, through the same requests as the
friends list's Invite and Accept (`crates/live/live-hle/src/auto.rs`; switched
on by `H2_COOP_AUTO_INVITE=<gamertags>` on the host and
`H2_COOP_AUTO_ACCEPT=<host>` on a guest). Once per lobby and per
invitation. They print `coop-auto: invited Bravo to session <id>` and
`coop-auto: accepting Alpha's invitation to session <id>`.

The lobby's session is the newest advertisement carrying a co-op campaign,
else the one advertised last (`Sessions::lobby_id`), so a session a game
opens on its way (the Xbox Live menu's, a custom game's) is never taken for
it:
a lobby that re-advertises gets a new random id, so the host invites
again to that one, and a guest accepts only an invitation newer than the
last it accepted. An older invitation names a session the host left; a
newer accepted one replaces it in the queue the title reads.

When the title reads the accepted invitation
(`XOnlineFriendsGetAcceptedGameInvite`, from its reader `0x0022f3b5`), the
guest prints `coop-auto: joining Alpha's session <id> (the game took the
invitation)` (without `--auto`: `live: join: the game took ...`). If it
hasn't after 20 s, it says so once: queued but untaken, or never confirmed
by the server (the 011442 run said neither, as the wait was dropped before
the server's answer queued it). If the title took it but never looked the
session up (FindFromID) within 15 s, it is offered again, up to three times.

The reader is a delegate on UI screen 9 (made by `0x0022f11e`); when it
finds an invitation it opens the two-button dialog for error `0x7d`
(`0x0019b5af`, first button `0x0022f36f`). Screen 9 is the main menu:
`0x001483c3`, which the shell calls to put up the root menu (at boot and
after a game), opens it unless it is the first boot's intro
(`0x0014741b`) or the case at `0x001484c0` (`0x002310b7`). The delegate
is an event, not a per-frame poll: in the 023123 run Bravo sat on the main
menu from before the acceptance until it pressed Xbox Live, and it was
never called.

So `h2 coop test --auto` tells the guests to join from the
friends list (Y, the host, Join Game), the path the 023244 run took to the
host's QoS.

Two paths look a session up (`0x000900e0`, FindFromID at `0x00090121`):
the friend view's updater (`0x002b65b8`, call `0x002b6690`), which looks a
friend's session up, probes it (QoS) and decodes its data to show it; and
the join itself (`0x0006f4b0`, call `0x0006f6d9`), which then probes the
squad's id and decodes (states 3: no result, 4: no probe, 5: no QoS info,
6: no data). The result parser (`0x00090160`) copies the slot counts
without checking them; it drops the result unless the title's own Live
logon is done (`0x000901c1`, `0x0006cd50() == 1`). On the main menu,
before Xbox Live, that fails and the friend view looks again the next
frame: the 023244 run's sixty `find session` lines a second, all `0 of 0`.

The title passes zero slot counts to `XOnlineMatchSessionCreate` for its
co-op lobby. Its join uses the host's QoS reply for admission. For a web
[join link](live-custom-games.md#join-in-halo-2), the host's lobby pinger
in `crates/extensions/game/src/coop_live.rs` publishes the observed player
count and available seats on the current Live advertisement.

A new console
needs both a free peer and a free player slot, within `coop.max_players`.
Explicit title slot counts, including reserved seats, take precedence.
A host with no observed capacity offers no link.

Repeated identical find
lines are logged once per 30 seconds.

The 023123 run's lobby was advertised twice (81113027... at +24.2,
deleted at +24.25, then 8201114c... at +34.2): both the XNetUnregisterKey
and the delete are the title's own calls (the HLE deletes a session only
when asked), as the lobby re-advertises once it settles. The host invites
again to the new one and the guest accepts the newer invitation.

The reader drops a record whose FILETIME at +100 is not before the
guest's own KeQuerySystemTime (`0x0022f3d7`) or is 15 minutes or more
before it. The guest's clock starts at the NT epoch (the kernel's
`epoch_100ns` is 0), so a record stamped with the Mac's date was in the
title's future: it read the record, dropped it, and nothing happened (the
2026-11-18 run: Bravo's `accepting` and then nothing). The HLE now stamps
the record a second before the guest's clock as it is read.

Menus are still yours: there is no input over the agent API yet (see
12.5). With `--auto`: on every window, Xbox Live (that signs the game in
to Live; until then the host sees nobody online and a guest can't take an
invitation); on the host, Co-op Campaign, a level and difficulty, then
Start Game. Without `--auto`, invite from the lobby's Y (friends list) and
accept on the guest, as on a console.

Still not established without a game run: that the title reads an invitation
accepted this way (`XOnlineFriendsGetAcceptedGameInvite`, which the
retail flow reads after its own Accept) and joins at that moment, rather
than only when its friends list is open. The test
`coop_auto_invites_the_named_friend_and_the_guest_accepts_by_itself`
(`crates/live/live-hle/tests/server.rs`) shows the accepted record is there,
naming the host's lobby, without a press.

#### "UNABLE TO JOIN ... network problems"

The dialog's title is `unable_to_join` (`0x0e00169d`, mainmenu) and its
text "You aren't able to join this party because we're having network
problems." is two strings: `error_join_qos_failure` (`0x160016e2`) and
`error_join_data_decode_failure` (`0x1e0016e3`). Both come after the
title's session lookup (`XOnlineMatchSessionFindFromID`, its only call at
`0x00090121`) found the session, at the QoS probe: `XNetQosLookup` (one
call, `0x0007b558`, synchronous: no event, no flags) to the host's XNADDR
with the session's XNKID. The HLE answers it on the host's XNet base + 2
(`crates/live/live-hle/src/qos.rs`), and only for a session the host listens
for (`XNetQosListen`, `0x00073243`: enable, then its lobby data at
`0x000732bb`). No answer leaves the target uncontacted (flags 1:
`qos_failure`); an answer without the lobby data fails the decode.

Each step is now in the games' logs and `coop.log`:

| Line | Step |
|---|---|
| `live: session <id> advertised for <ip>:<port>` / `deleted` | the host's lobby on the server |
| `live: xnet: key for session <id> registered` / `unregistered` | XNetRegisterKey on either side |
| `live: qos: listening for session <id> on <ip>:<port+2>: on, N bytes of data` | the host's QosListen, on each change |
| `live: qos: <guest> asked about session <id>, which this console isn't listening for (listening: ...)` | the host, once per unknown session: the usual cause |
| `live: join: find session <id>: <host> hosts it at <ip>:<port> ...` (or `no such session`, `the session is <state>`) | the guest's FindFromID |
| `live: join: qos lookup: <ip>:<port+2> session <id>: contacted, N bytes of data` (or `no answer in 3 tries ...`, `nothing on that port`) | the guest's QoS probe; a host behind the guest's own NAT shows its LAN address first, then the public one if the LAN address did not answer |
| `live: join: <ip>:<port> for session <id>: address made` / `connecting to ...` | XNetXnAddrToInAddr and XNetConnect |

The 2026-11-18 (011442) run with these lines showed the failing step: the
host listens on two ids, its squad's XNKID (made by the title, listened on
with no data: `XNetQosListen` flags 5, `0x00073243`) and the session id the
server gave its advertisement (with the 172-byte lobby data). A joining
guest probes the squad's id; with no data the title's join decode
(`0x0006f5f5`, state 6) fails: `error_join_data_decode_failure`. The HLE's
responder now answers an enabled listener without data with the newest
lobby data the console listens with (`qos::answer`).

The `0 of 0 public
slots` in that run's find line did not stop the join (the guest went on to
probe); the line now shows the private slots too.

That run also had all three games on one IP, which the HLE told apart by IP
alone: a host heard both guests as one peer and every game saw the others
at its own address. Peers are now told apart by IP and base port
([live.md](live.md), XNetXnAddrToInAddr): a game on this Mac's IP is
`169.254.(base >> 8).(base & 255)` to the others, so `address made`
lines show those addresses.

A peer the host first hears on a game channel arrives with key id 0, so the
host's XNetInAddrToXnAddr on the sender cannot place it in its session. The
peer carries the session that address last probed (`qos::Asked`) instead;
without it the join stops with no dialog and no XNetConnect after the squad's QoS. The
first datagram each way between a game and a peer is logged:
`live: xnet: first packet to <ina>:<port> (at <ip>:<port>)` and
`live: xnet: first packet from <ip>:<base> (as <ina>) on port <port>,
session <id>`, a failed send each time, and an XNetInAddrToXnAddr for an
address that is no peer.

A probe now tries three times, 100 ms each (once, 50 ms before): three
games on one Mac run at a few frames a second, and a single short window
could be missed. The 2026-11-18 run logged none of this (it predates the
lines), so which step failed there is for the next run's log to say:
look for the host's `asked about session` line or the guest's `no
answer`.

### 12.2 Two games on one Mac

Checked in the code, not by a run:

| Shared thing | How the games keep apart |
|---|---|
| XNet ports | Base 31000 and 31010 (`--port`, 10 up per game) on `--ip` (this Mac's LAN address, else loopback). Each game binds base to base+6 (title channels +0, +1, +5, +6; QoS and the co-op ping +2; the co-op channel +4); `NetworkConfig::validate` wants bases 7 apart on one IP. `H2_XNET_LOOPBACK_PEERS` names the others for discovery. |
| Data | `--data <root>/<gamertag>/data` each: profiles, settings, saves. |
| Live account | `H2_LIVE_DIR` each: `accounts.json` and `play-accounts.json` (the one account handed to the game). Without it two games on one Mac write the same `play-accounts.json`, and the one that reads last may sign in as the other. |
| The game image | One `--out` (default `$TMPDIR/h2boot`), read by both. Only the host may build it: the guests open after the host's `live: playing as`, which `h2 play` prints after its build. Set `H2_OPT` as you do for `h2 play`, or the image rebuilds. |
| Agent socket | `H2_AGENT` each: `~/Library/Caches/h2recomp/coop-<gamertag>.sock`. A live socket at a path is an error, so one each is required. `h2 agent --socket <path> status` (or `watch phase map`) reads each game. |
| Pipeline cache | `~/Library/Caches/h2recomp`'s pipeline store is made for several processes (`d3d::pipeline_store`). |
| Input | Each window takes the keyboard while focused. Pads go to the host (`--pad guest`: to the first guest); the other games get `--no-gamepads`. |
| Sound | The guests run `--no-audio`. |
| Map pack, Halo 1 | Not installed into the games' data (`--no-mappack --no-halo1`) unless `--extras`. |

Each window's title names its game and role (`Halo 2 - Alpha (host)`,
`Halo 2 - Bravo (guest)`; `h2 play --title`), and the windows sit side by
side across the main screen, the host leftmost: `h2 play --tile I/N`
sizes a window to column I of N (the largest 4:3 that fits under the menu
bar) and places it there (`app::tile`).

### 12.3 The checklist

`h2 coop verdict <coop.log> [--auto]` prints it again
(`crates/apps/cli/src/coop_verdict.rs`). Each step is seen when every game it
is for printed its line:

| # | Step | Game | Line |
|---|---|---|---|
| 1 | signed in to Live | each | `live: playing as <tag>` |
| 2 | the disc's campaign levels read | each | `coop-live: N campaign levels (` |
| 3 | Co-op Campaign opens the game's own lobby | host | `coop-live: Live menu row ... in co-op mode` |
| 4 | hosting in that lobby | host | `coop-live: hosting a co-op campaign in the game's own lobby` |
| 5 | the lobby is in co-op mode | host | `... lobby: mode 2 (custom game) set as 0 (co-op campaign)` |
| 6 | level and difficulty advertised | host | `coop-live: the lobby holds ... ; advertised` |
| 7 | the host invited the guest | host | `coop-auto: invited` (only with `--auto`) |
| 8 | the guest accepted | guests | `coop-auto: accepting` (only with `--auto`) |
| 9 | the guest is in the host's session | guests | `coop-live: guest: joined` |
| 10 | ping | each | `coop-live: ping N ms` |
| 11 | the co-op check passed for two consoles | host | `coop-live: lobby start state 7 ... answered` |
| 12 | the session start counted the peers | host | `session start: campaign with` |
| 13 | the session started | host | `session start returned 1` |
| 14 | the host simulates | host | `simulation mode sync-server` |
| 15 | the guest follows | guests | `simulation mode sync-client` |

A step not seen comes with the likely reason in plain words, most often a
press nobody made: for step 3, "Alpha never opened the Xbox Live menu" (no
`custom games:` line from the Live menu), "opened the Xbox Live menu but
never picked Co-op Campaign" (rows drawn, no co-op row chosen), "picked
another of its rows", or that the co-op hooks are off; for steps 4 and 5,
"picked Co-op Campaign but the lobby did not open in co-op mode"; for step
7, which guest never opened Xbox Live (so the host had nobody online to
invite); for step 9, accept by hand, or (with `--auto`) accepted but not
joined; for step 11, nobody pressed Start Game, or the lobby's start check
never asked about co-op. For steps 3 to 6, when the host walked none of
its path but a guest did (picked a lobby mode, a campaign's difficulty,
one of our Live rows), it says "you drove Charlie's window (it started a
campaign or lobby itself); drive Alpha (host), the leftmost window".

It ends with the first step not seen ("stopped at step 9: ...", with its
reason) and lines
that look like trouble (`ping: no answer`, `returned 0 (refused)`,
`could not invite`, `could not accept`, `did not open`, panics). What to
send back: `coop.log`, and the two whole logs if it stopped early.

### 12.4 What is tested

Without the game (`cargo test -p cli --bin h2 coop`, `cargo test -p
live-hle`): the arguments; each game's ports, data, Live directory, socket,
pads and `--auto` variables; the shell lines; accounts and friendships made
against an embedded server, reused on a second run, and a lost key saying
`--fresh`; two stand-in games (shell scripts) through the log merger with
the host's sign-in starting the next; the checklist from canned logs (a
whole run, a run that stopped, a run without `--auto`); the reason given
for each step a missing press explains (3: no Live menu, no co-op row,
another row, hooks off; 5; 7: a guest not in Xbox Live; 8; 9 with and
without `--auto`; 11; 1); the filter over the host's log of the
2026-11-17 three-game run, cut to its main menu
(`crates/apps/cli/src/coop_verdict/host-at-menu.log`, our own lines, paths
shortened): only the co-op lines kept, no feature plan; the next-steps
line; the wait for the main menu against a stand-in agent socket;
`H2_LIVE_DIR`; the automatic invite and acceptance between two HLEs
and a server, and to the lobby's newest session when its id isn't the
smallest; a stopped game (its guest thread exited, or a failed exit)
closing the others while it stays open, and a normal exit not counting;
the accepted record stamped in the guest's clock so the reader's check
takes it, said once and offered again; a guest accepting only the newest
invitation; one log line per QoS target; and a QoS probe for a session
the host doesn't listen for coming back uncontacted after its tries,
while the current one is answered with its data.

### 12.5 Driving the host's menus (the plan; built as 12.6)

`--auto` stops at the menus. What exists to press through them:

- `--input-timeline FILE` (`input::backend::scripted::Timeline`,
  docs/input/input.md): pad frames at absolute guest times, played *instead
  of* live input, from launch. It knows nothing of the screen, so a press
  lands where the menu happens to be: the intro, the profile screen, the
  Live sign-in and the session made by Co-op Campaign take different
  times on every run (the host's first start here built for 20 minutes
  before its window even opened). The window's keyboard and pads are off
  while it plays.
- `H2_BOOT_INPUT`: presses counted in scheduler slices, for headless
  runs; the same blindness, and slices are not even time.
- The agent API (docs/runtime/agent-api.md) designs `input.pad` (timed
  packets through the normal input path) and the `ui` domain (which screen
  is open, which row has focus) and events like `screen`, but phase 1 has
  neither: only `session.*`, `world.*`, `profile.counters` and
  `features.list`. `session.wait` can wait for `phase: menu`, not for
  "the Xbox Live menu with Co-op Campaign focused".

What it would take, in order: (1) `input.pad` on the agent socket, a queue
the xapi input glue drains ahead of live input (the boot already merges a
keyboard pad for `H2_BOOT_INPUT`, so the path exists); (2) enough of `ui`
to know the screen and focused row: the open screen's tag id (the Live
menu is screen id `custom_games::SCREEN_ID`) and the list's cursor, read
on the tick like the other answers; (3) a short script in the harness:
wait for the main menu, press down to Xbox Live and A, wait for the Live
menu, move to the Co-op Campaign row and A, wait for `coop-live: hosting`,
then the level and difficulty screens, then Start once every guest is in.
Each step waits on what the screen says, not on time.

Risk: (1) is small and testable without the game; (2) needs the screen
stack's layout, which is partly traced (`layout_glue`, the screen loaders)
and would need a game run to confirm; (3) depends on row order, which our
own Live menu rows change (Co-op Campaign moved the rows down), and on
the profile screen, which a fresh data dir shows and a reused one may not.
A press in the wrong place is harmless in the menus but confusing in a
report. Until (1) and (2) exist, the line in step 4 above is the guide.

### 12.6 `--drive`: nobody presses

`h2 coop test --players 3 --auto --late --drive` runs the whole test
without a press. The three parts of 12.5:

1. **Input.** `input.pad` already exists on every game's agent socket
   (the agent's tap at XAPI, merged over the live pad, so the keyboard
   still works). The harness sends one short step per press: the button
   for 150 ms and at least two pad reads.
2. **Screens.** With `H2_UI_WATCH=1` (which `--drive` sets), a game prints
   `ui: screen 0x09 (window 5, slot 4)` whenever the screen that takes
   player one's input changes, and `ui.screen` answers it on the agent
   socket (`h2 agent --socket <path> ui`). On the Xbox Live menu the line
   adds the row the menu restores its cursor to (`, live menu row 3`,
   `0x510a18`). A `ui: screen none` line with a dialog up (windows 0
   to 4, which read as no screen) ends with the dialog's id and tag
   name (`, dialog 0x14 dialog_live_signin (window 3, slot 4)`), and
   `ui.screen` answers it as `dialog`. `boot::ui_watch` holds the layout. [local] It is read
   statically: the input dispatcher `0x147dbe`, the push `0x147f6d`, the
   channel test `0x1473b6` and the delivery `0x23515d`. [guess] It has
   not been seen in a run. The focused row is not read: list focus goes
   through widget virtual calls (`0x24c447` → `0x24bae6` → `0x22ed7a`).
   The driver counts its own moves from the restored row instead.
3. **The script** (`crates/apps/cli/src/coop_drive.rs`). There is one driver
   per game. Each reads its game's `ui: screen` lines and every game's
   co-op lines, and asks for one press at a time, at least 0.7 s apart.
   A press happens only once a screen has held for 1.5 s, or after a line
   it waited for. Every step has a 15 s timeout and up to 4 tries; after
   that the driver says `coop-drive: <tag>: stuck: ...; press on its
   window` and stops for that game. Each press goes into coop.log as
   `coop-drive: Alpha: press a on screen 0xba row 3 (Co-op Campaign)`.

| Game | Path |
|---|---|
| every game | No press until the main menu, which the coop-test data reaches by itself about 19 s after the agent socket is up (presses START, A, B only if it hasn't a minute after the first chance). Main menu: DOWN, A (Xbox Live). That opens the players' screen `0x1e` in mode 3; A there opens the Live menu. If what opened says the cursor was on another row (`0xee` for Campaign, mode 1 for Split Screen, mode 2 for System Link), the driver presses B and moves to Xbox Live. The guests come back to Xbox Live a second time with A, corrected the same way |
| host | Live menu (`0xba`): from the restored row to row 3 (Co-op Campaign, directly above Custom Game), A. Wait for `coop-live: hosting`; `custom games: opening the browser` means it landed on row 7, so B and try again. Wait for every lobby guest's `coop-live: guest: joined`, then A on Start Game (the lobby's row 0, `0x250fb3` → `0x251050`). If `session start returned 1` or `simulation mode sync-server` doesn't follow, UP, A |
| guest | Live menu: B back to the main menu, where the invitation reader runs (12.1). Once `coop-auto: accepting`: DOWN, UP (the reader runs on an event, perhaps a controller event). If the game hasn't joined 20 s after the acceptance (or 5 min with no acceptance), A for Xbox Live, row 7 (Custom Game Browser), A. Once the browser has probed a game (`live: join: qos lookup`) and 1.5 s have passed, A on the first game. Done at `coop-live: guest: joined` |
| late guest | As a guest, but the browser fallback waits until 120 s after the host invited it, so it joins in progress |

**What the first runs showed** (2026-11-18, 083611 and 092343, with
`H2_UI_WATCH=1`):

- No boot presses are needed. Each game shows `0xb6` (window 1) for a
  moment, then no screen, then says `coop-live: lobby mode 4294967295`:
  that is the main menu.
- The main menu reads as no screen. It is pushed as id 9 on window 5,
  slot 4 by `0x1483c3`. Why it reads as none is open, so a none line now
  says what each slot held (`ui: screen none (slot 4: window 1 active ...
  list [mask/id ...]; slot 0: ...)`). The driver takes "no screen after
  `lobby mode`" as the main menu.
- The read now follows the dispatcher more closely: a window whose list
  has no screen for player one passes the input on to player one's own
  slot, not to the next window.
- Campaign has focus when the main menu opens. The retail rows (`0x230827`,
  chosen row at `0x510a14`) are 0 Campaign, 1 Xbox Live, 2 Split Screen,
  3 System Link and 4 Settings. With Theater installed, it occupies row 4
  and Settings row 5, so the rows the driver presses keep their places.
  The driver uses the players' screen mode to correct the path back to
  Xbox Live.
- A on Campaign gives `0xee` (window 1), then `0xd1` (window 5), then the
  level. The driver backs out of `0xee`.
- Xbox Live (when signed in, `0x2308e0`) gives `0x1e` on window 5. That
  is the players' screen (`0x2524cc`, vtable `0x45a490`), one class in
  five modes (`+0x610`, now on the line): 1 Split Screen, 2 System Link,
  3 Xbox Live, 4 an accepted invitation (from the invitation dialog's
  first button `0x22f36f`). A or START (`0x25286d`, buttons 0 and 0xc)
  goes on by mode: mode 3 opens the Live menu (`0x2310b7`) and mode 4
  joins. B with one player goes back.
- A guest waiting for the invitation presses A on any screen that comes
  up after the acceptance (the invitation dialog) and on the players'
  screen in mode 4, at most 4 times. It presses B on the players' screen
  in any other mode.

[guess] Still not seen in a run:
- whether `0x1e`'s A opens the Live menu at once (static: mode 3 →
  `0x2310b7`);
- the lobby's id;
- whether the Live menu reads as a screen (`0xba`) or as none, like the
  main menu.

Tests (`cargo test -p cli --bin h2 coop_drive`, `-p boot --lib ui_watch`):
- A model of the menus runs three games without the invitation reader
  (the 023244 run's case: both guests join through the browser, the host
  starts after both).
- The same model runs with a working reader (no browser) and with a late
  guest (it joins after the start).
- A main menu whose cursor comes back on Campaign is corrected.
- The players' screen in mode 2 is read as the System Link row.
- No boot press comes in the first minute, then START, A, B, then
  `stuck`.
- A wrong Live menu row is backed out of.
- The 092343 run (`coop_drive/run-092343.log`) is replayed: Bravo makes
  no press before the main menu, then DOWN, A, and A on the players'
  screen.
- The 023244 run's own lines (`coop_drive/run-023244.log`) are replayed:
  the host waits in its lobby, and Bravo is nudged at its acceptance and
  goes for the browser 20 s after it.
