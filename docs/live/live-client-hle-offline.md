# Live: client HLE, offline, sessions and teams

## Client HLE: Live not set up

`crates/extensions/game/src/live_notice.rs` puts our text on the game's own "cannot
connect to Xbox Live" dialog, the one that offers the troubleshooter (UI
error `0x23`). The game draws it in its dialog and font; no tag or localized
string is edited. `boot::run_hosted_with_live` installs it after the Live
boundary and telemetry.

How the dialog is reached (static reading of the retail image, not a run):

- **With Live off** (`h2 play` without a Live flag). Nothing is registered, so the real XNet library runs.
  The default console reports no cable (`ConsoleConfig::phy_link = 0`), so
  `0x0008d690` never starts transport and `0x004d8b19` stays clear. Picking
  Xbox Live on the main menu runs the row handler `0x002308e0`: not signed
  in (`0x0019028d`) and transport unusable (`0x0008d7c0` at `0x00230928`), so
  it calls `0x0019b5af` with error `0x23` and the network-settings action
  `0x00236917` at `0x002309e1`. The dialog appears at once; there is no
  sign-in attempt to hang on, so no extra "not configured" mode is needed.
- **With Live on and a server.** If logon ends with CANNOT_ACCESS_SERVICE
  (`0x80151001`, the HLE's result for transport or protocol failures), the
  classifier gives game result 4 and `0x001486b8` shows the same error
  `0x23` (`0x001486fa`).

Dialog widgets (vtables `0x00454560`, `0x004545d0`, `0x00454640`) keep the
error ID at `+0x610`, then four 256-unit UTF-16 buffers: title `+0x614`,
message `+0x814`, first button `+0xa14`, second button `+0xc14`.
`0x0019b72a` (EAX = error ID, EBX = dialog, one stack word, `ret 4`) looks
the error up in the shared UI globals (`0x0023661f`) and resolves its
strings into those buffers; it is called from both dialog constructors
(`0x0019b56a`, `0x0019b5f3`) and from `0x001911ef`. The update method
`0x0019b682`, shared by all three vtables, copies the title and message
buffers into the text widgets every frame. The hook wraps `0x0019b72a`
(shadow entry `0xa0ff6000`): after the original fill, if `+0x610` is `0x23`,
it writes our message into `+0x814`, at most 255 units and a zero, so the
first button's buffer is never touched.

Title and buttons keep the game's
text. Finding the dialog this way needs no asset names, string IDs or
string-table offsets.

An external-service boot that has loaded an editor play copy uses this dialog
to explain that signing in requires a restart. Otherwise its text follows the
boot's `LiveConfig`:

| Live | Server | Message |
|---|---|---|
| off | (any) | "Xbox Live isn't set up in h2recomp.", then how to start with `--live` or `--live-local` |
| on | URL, no account | "No Live account on this machine yet.", `h2 live account create <tag>`, or recover at `<URL>/dash/signup/recover` |
| on | URL | "Can't reach the h2 live server at" and the URL, then the same advice |
| on | none | stock text (local sign-in cannot fail to connect) |

```text
Xbox Live isn't set up in h2recomp.
Quit and start again with one of:
h2 play --live  (live.forbes.red)
h2 play --live-local  (this machine)
Accounts: h2 live account create <tag>
```

The message box on the OK/cancel error dialog is about 244 by 91 pixels,
five lines of the body font, so each line is kept under about 240 pixels.
Lines break on `\r\n`, as the game's own strings do. The URL is shortened
to 40 characters, and `|` (which starts a markup escape in the text scanner)
is replaced.

The message does not mention `H2_LIVE_SERVER`, which `h2 play --live` and
the `h2 live` commands read.

The first button still offers the troubleshooter. It launches the
dashboard's network settings through `XLaunchNewImageA`, which ends the run
here. Other Live errors (servers busy `0x25`, invalid user `0x9a`,
connection lost `0x38`) keep the game's text.

Tests: `crates/extensions/game/src/live_notice.rs` checks the text, the URL line and
the buffer bounds on synthetic guest memory; `crates/apps/boot/tests/live_notice.rs`
runs the hook through the real Runner with the fixture module's leaf
standing in for the fill, and checks that only error `0x23` changes, that
install refuses to wrap an existing override and skips a missing original,
and that local-only Live installs nothing. They need no XBE or game build.

"No account" means the boot's accounts file (or its absence) holds no entry
with a private-JWK key (`live_hle::account_setup::load`); an unreadable file
keeps the "can't reach" text and is left to sign-in to report. Whether the
retail game reaches error `0x23` at all with zero accounts listed is not yet
checked in a run.

### In-game account setup

Making or recovering a Live account from inside the game. Built: the host
calls, and the sign-in
prompt below, which recovers an account made elsewhere and can create one
for an open server. Not built: the Create / Recover menu
(`boot::account_screen::AccountScreen`) as a screen; only its layout helpers
are used, by the prompt.

**Host side, `live_hle::account_setup`** (no guest memory; blocking HTTP, so
run each call on a `Job` thread and poll it from the frame loop):

- `create(server_url, gamertag, invite)` makes a key, signs an
  `AccountRequest` for the server id from discovery, and `POST`s
  `/v1/accounts`. It returns the `LocalEntry` and the `RecoveryInfo` to show
  once. The invite goes as typed (trimmed; empty means none).
- `recover(server_url, gamertag, code)` makes a key and `POST`s a signed
  `RecoverRequest` with the code to `/v1/accounts/{gamertag}/recover`.
- `recovery_code(server_url, entry)` and `reset_recovery` read or replace
  the secret with a self-issued `recovery.read` / `recovery.write` token.
- `needs_invite(server_url)` reads `invite_only` from `GET /v1/server`.
  It returns `None` when a private server won't say; a "needs an invite"
  refusal then turns the invite field on.
- `save(path, entry)` adds the entry to an accounts file in the CLI's shape
  (`{xuid, gamertag, id, server, server_id, kid, key}`). It replaces an entry
  with the same xuid, or one with the same gamertag on the same server. It
  writes other entries back with every field they had, and writes mode 0600
  through a temporary file and a rename. `shared_accounts_path` maps the
  per-session `play-accounts.json` to the `accounts.json` beside it.
- Each call refuses plain HTTP to anything but loopback (`Insecure`) before
  it makes a request. Its `SetupError` is one short line for the player:
  gamertag taken, needs an invite, invite not valid, no such gamertag, no
  recovery set up, wrong code, locked out for N minutes (from the server's
  429), busy, can't reach the server, or couldn't save. None of these
  messages includes a key, a reply body or the code that was typed.
- `Hle::reload_accounts()` reads the accounts file again, so a saved entry
  is listed by `XOnlineGetUsers` and signs in without a restart. Accounts
  that are signed in stay signed in, and if the file is broken nothing
  changes.

**Screen logic, `boot::account_screen::AccountScreen`** (pure Rust): a menu
with Create, Recover and Cancel. Create asks for a gamertag, then an invite
if the server needs one, then works, then shows "Write this down" (the
current code with its seconds left, the authenticator key in groups of
four, and how to recover). Recover asks for a gamertag, then the code, then
works.

A picks, B goes back, and B never skips the secret. The glue opens
the keyboard for each `Effect::Keyboard` and answers with `typed`. It runs
each `Effect::Start` as a `Job`, saves the entry, and answers with
`finished`.

On `Effect::Close { signed_up: true }` it calls
`reload_accounts`. Page text is at most five lines of 40 characters, broken
with `\r\n` and never holding `|`, as for the notice.

**Pending (needs facts from a run):** an entry point for the Create /
Recover menu, and the drawing. The sign-in prompt (below) is the entry for
an account that exists on the server. Either of these would do for the menu:

1. A list screen like Video and Skulls (`list_class`), opened from a row or
   from the error `0x23` dialog's button. This needs the retail handler for
   that button, or a menu to put the row on.
2. The keyboard as text entry: open the retail keyboard screen (factory
   `0x0023784f`, screen id `0xa7`) on demand with our buffer, and read the
   text back when it is accepted. `keyboard_glue` only types into a keyboard
   the game has already opened.

The game also has to choose which file to save into. It only knows
`LiveConfig.accounts_path`, which under `h2 play` with Live on is the session's
`play-accounts.json`, rewritten on every start. The in-game sign-in (below)
saves to that file and to `shared_accounts_path(..)`.


Tests: `crates/live/live-hle/tests/account_setup.rs` covers create, recover with
the code from the returned secret, a wrong code, a used code, an unknown
gamertag, lockout, reset, an unreachable server, saving with other entries
kept at 0600, and a saved account signing in after `reload_accounts`. All of
them run against the in-process memory server.
`crates/live/live-hle/src/account_setup.rs` maps refusals to errors.
`crates/extensions/game/src/account_screen.rs` covers every transition and checks that
every page fits.

### In-game sign-in

When the player signs in to Live in the game with an identity this machine
has no key for on the server (made on the web, another device or another
server), the game asks for the 6-digit recovery code itself.

**The account list.** An accounts-file entry with `"needs_code": true` is
listed by `XOnlineGetUsers` like any other (the same 112-byte record), so
the player picks it in Halo 2's own sign-in. Its key, if any, is never
used, and it is dropped when a ready account of the same gamertag is in
the file (`live_hle::config`). `h2 play --live` writes such an entry into
`play-accounts.json` when the gamertag is on the server already
(`play_live::write_waiting`), with the server's xuid for it.

**The logon** (`live_hle::signin`). `XOnlineLogon` with a `needs_code` user
(one per logon, and only with a server; otherwise `INVALID_USER`) returns a
task and makes no request: `XOnlineTaskContinue` returns `0` (pending), so
the game stays on its signing-in step. The prompt answers through
`Hle::signin_submit(code)`, `signin_create()`, `signin_cancel()` and
`signin_dismiss()`, and reads `Hle::signin()`:

| Phase | Shows | A | B |
|---|---|---|---|
| Code | the code being typed, the last error | check it | cancel |
| Working | "Checking with HOST..." | | cancel |
| Create | the gamertag isn't on the server | make it there (open servers) | cancel |
| Created | the new account's recovery secret, once | close | close |
| Done | signed in | close | close |

A code that is not six digits is refused without a request. Each check runs
`account_setup::recover` on its own thread; a wrong code says how many
tries are left (the server's count), a lockout how long to wait, an
unreachable server says so, and the code is asked again. No such gamertag
offers Create, which runs `account_setup::create`; an invite-only server
says to make it on `/dash` or pass `h2 play --invite CODE` (there is no
in-game invite entry).

On success the entry is saved to the session's file
and the shared `accounts.json`, the accounts are read again, the user's
record is replaced by the saved account's (the controller slot word kept),
and the service logon starts as for any account, ending in `CONNECTED`.
Cancel fails the logon with `INVALID_USER` (the user's HRESULT too).

**The prompt** (`crates/extensions/game/src/signin_screen.rs`) is our own list class
(`list_class`) with one row and a description, on the Controls layout. The
d-pad types the code (up and down change the digit, left and right move;
direction events, kinds 1 to 4, from the d-pad or the left stick, as
`skulls-arrows` read the controller update), A on the row or Start checks
it (not X: the class's action buttons are Start alone, so a stray X can't
send a half-typed code), B or Back backs out and
closes (Back cancels the logon first, so none is left pending). An error shows until the code is edited and comes back
with the next answer.

The prompt becomes a modal dialog over the screen beneath, in the large
dialog's panel with six code cells and a glyph legend
(`signin_screen::dialog_for`, docs/game/ui-builder.md "The modal
dialog"). That is in the preview, the sheet and the editor; in game it is
still the list until the dialog's tag recipe is built. The page's
behaviour is proven offline as a table
(`signin_screen::SigninPage`, following `live_hle::signin`'s own
`typed`, `refused` and `settled`; docs/game/ui-builder.md "Behaviour").

The text is served from guest buffers under our
string ids; each change goes into the other buffer of a pair, and the row
text (`Rows::live_text`) writes it and sets the description again. The
logon opens it: a wrapper around the HLE's `XOnlineTaskContinue`
(`0x003a0620`) asks `Hle::take_signin_open()` after each call, and the
first time a sign-in waits it writes the title and first page and calls
`0x149f1e(1 << controller, loader)`, as the Skulls page opens. A finished
page closes with `0x14800c` (the screen's `+0x78` slot and `+0x74` window).

Static reading (retail 5849): the main menu's Xbox Live row (`0x002308e0`),
not signed in and with transport, animates the menu out (`0x0022eeee`),
reads the accounts (`0x0018fc44`) and opens the account picker with loader
`0x0024b4a9`; the logon is the picker's. Not established without a run:
that the picker polls `XOnlineTaskContinue` each frame while signing in,
that a screen opened from inside that call draws on top and takes input,
that our screen's event hook sees the direction kinds before the list does, that a
list's row text runs every frame (so a finished check shows without a
press), and what the game shows for `INVALID_USER` after a cancel.

Switches: `features.signin-code=off` turns the class off (a waiting
sign-in is cancelled at once; `h2 play --signin-terminal` asks on the
terminal instead).
Every step logs a `live:` or `signin prompt:` line on stderr.

The app builds (iOS, tvOS) run the same HLE and boot glue, so an entry
marked `needs_code` in `live/accounts.json` asks in game the same way, and
they offer a new account in game (next).

#### A new account in game

On the app builds there is no terminal, so the account list itself offers
the way in. `live_hle::platform::config` sets
`LiveConfig::offer_new_account`, which does two things
(`live_hle::config`):

- A missing `live/accounts.json` is no accounts rather than an error (the
  first start), and
- the list gets one more identity, "New Account" (`NEW_ACCOUNT_TAG`, xuid
  `NEW_ACCOUNT_XUID`, `0x0009000100000000`, outside the stand-ins' range),
  marked as waiting.

Signing in with it starts a `Signin` that asks for a gamertag first
(`Signin::adding`). The prompt's phases before the code page:

| Phase | Shows | A | B |
|---|---|---|---|
| Gamertag | "A gamertag on HOST", the last error | open the keyboard | cancel |
| Invite | "HOST needs an invite to make TAG", the last error | open the keyboard | cancel |

The typed text goes to `Hle::signin_typed(Some(text))`, a backed-out
keyboard to `signin_typed(None)` (from the invite: back to the gamertag).
A gamertag that isn't one is refused without a request. Otherwise
`account_setup::create` runs on its own thread, with the invite when one
was typed:

- **Free:** the account is made and saved to the accounts files, the
  recovery secret is shown once (Created), and the logon goes on as the new
  account, as above.
- **Taken:** the gamertag is saved as waiting for its code
  (`account_setup::save_waiting`: `{xuid: stand_in_xuid(tag), gamertag,
  server, needs_code: true}`, added once, other entries kept), and the page
  becomes the code page for that gamertag. The right code signs in as the
  account itself. Backing out leaves the waiting entry, which the list
  shows from then on.
- **The server wants an invite:** the first refusal asks for one (Invite,
  no error), and a bad one says so and asks again.
- **Anything else** (unreachable, busy, server error) is said on the
  gamertag page.

**The keyboard** (`crates/extensions/game/src/signin_keyboard.rs`). A on either page
opens the game's own on-screen keyboard. On the app builds the host's
system keyboard types into it (`keyboard_glue`). It is opened with the
retail opener `0x00238c21`: stdcall `(context, buffer, capacity)`, `ret
0xc`, the player in CL. The opener creates the screen through its loader
`0x0023784f` and edits our UTF-16 buffer (a page of ours) in place.

Static reading of the keyboard (retail 5849):

- `0x002381de` backs the text up at `+0x618` and points `+0x818` at the
  buffer.
- B copies the backup back (`0x00237fcc`).
- Done checks the text against the context's flags in the table at
  `0x0044a9f8` and then runs the context's case of the jump table at
  `0x00238468`.

Contexts 8 to 11 and 17 to 20 only close the screen on Done. Contexts 16 to
19 swap in the keyboard's own buffers when it is laid out (`0x00237a74`).
So:

- The gamertag uses context **8**: Done needs a letter or digit, and the
  capacity is cut to 17 units.
- The invite uses context **20**: no check, up to 150 units, and no title
  of its own (string id 0).

While one of ours is open, the prompt's string source answers that
context's title ids (`0x0d00044f` and `0x12000450`, or 0) with our words.
The buffer starts empty, so empty text when the keyboard closes means B.
The prompt's row text polls for that close: once the shared channel's
screen (`0x0054dba8 + 8`) is no longer the keyboard on our buffer, the text
is read and passed on. A keyboard that never shows within 120 polls counts
as backed out.

Not established without a run:

- what contexts 8 and 20 show around our title text;
- that opening the keyboard from our screen's row press draws it on top
  and takes input;
- that row text keeps being asked while the keyboard covers the prompt
  (the close is still seen when the prompt is back on top);
- that the host keyboard bridge finds it (it accepts contexts up to 20);
- how the game takes the signed-in user's xuid and gamertag changing from
  "New Account" to the new account's under it.

**Checklist for a run on tvOS or iOS** (fresh install, or
`live/accounts.json` deleted):

1. Xbox Live on the main menu lists "New Account". Pick it. Stderr:
   `live: New Account: asking in game for a gamertag on <url>` and
   `signin prompt: opening for controller 0 (window kind 5)`.
2. The prompt reads "New Xbox Live account". Press A: `signin prompt:
   opening the keyboard for the Gamertag (context 8, controller 0)`; the
   game's keyboard opens, titled "Your gamertag", and the system keyboard
   comes up with it.
3. Type a free gamertag, Done: `the Gamertag keyboard closed with text`,
   then `live: making <tag>`; the recovery secret page; A finishes signing
   in.
4. Again with a gamertag made elsewhere: the code page for it; enter the
   6-digit code from its Account page.
5. Against an invite-only server: the invite page; A opens the keyboard,
   titled "Invite code".
6. B on the keyboard: the page is asked again; B on the prompt cancels.

**Checklist for a game run** (`h2 play --live-server URL` with an account
made on that server's web dashboard and none on this machine):

1. `h2 play` prints `... needs a code from Account on URL/dash; the game
   asks for it when you sign in to Xbox Live` and starts without a terminal
   prompt.
2. Xbox Live on the main menu lists the gamertag; pick it. Stderr:
   `live: <tag> needs a code to sign in on <url>; asking in game`, then
   `signin prompt: opening for controller 0 (window kind 5)`.
3. The prompt is drawn over the signing-in step, titled "Xbox Live
   sign-in", the row reading "Sign in as <tag>", the description the code
   page. If the description is off-screen, say where it is drawn.
4. D-pad: each press logs `signin prompt: event K, button N (Some(Up))`
   and so on (K 1 to 4), and the digits change.

5. A wrong code: "Wrong code. 4 tries left." appears without another press.
6. The right code: "Signed in. A: done"; A closes the prompt and the game
   finishes signing in; `play-accounts.json` and `accounts.json` hold the
   key. The next `h2 play --live` signs straight in.
7. Again, with B on the prompt: note the game's sign-in error and that it
   returns to the menu.

Tests: `crates/live/live-hle/tests/signin_code.rs` (the logon pending through a
wrong code, the right code against the embedded server's real TOTP, both
files saved, the countdown to a lockout, cancel, create, unreachable, no
server; New Account making a free gamertag, a taken one kept waiting and
signed in with its code, backing out of one, an invite-only server);
`crates/live/live-hle/src/config.rs` (the list, the New Account row);
`crates/apps/boot/tests/new_account_prompt.rs` (New Account opening the prompt
through the runner; A opening the retail keyboard on our buffer, with our
title); `crates/extensions/game/src/signin_keyboard.rs` (reading the text back);
`crates/extensions/game/src/signin_screen.rs` (the d-pad entry, every page fitting
five lines of 40); `crates/extensions/game/src/list_class.rs` (a class's own buttons,
live text); `crates/apps/cli/src/play_live.rs` (the hand-off).

## Client HLE: sessions and server discovery

The host/search APIs in [live-matchmaking.md](live-matchmaking.md) sections
1.1, 1.2 and 4 use the background HTTP worker when `LiveConfig.server_url` is
configured. Without a server they retain the local host/empty-search behaviour.
These APIs advertise and discover lobbies.

They do not implement a matchmaker,
ranked arbitration or NAT traversal. Invitation delivery is described next.

The game client retains the 1,024 most recently read session advertisements
for joins and playlist lookups. A successful lookup refreshes a session's
place in the cache.

Create copies the four slot counts and the attribute list before returning a
task. Update consumes only dirty attributes and replaces the slot counts;
NULL removes a value. The 16-byte XONLINE_ATTRIBUTE is ID, changed BOOL and an
eight-byte union: integer, UTF-16 string pointer, blob pointer/length, or NULL.
Limits remain 64 attributes, 400 UTF-16 units per string, 800 bytes per blob and
32 advertisements.

Server-backed lobbies have at most 16 slots. Invalid guest
ranges leave outputs and dirty flags untouched. A request that was accepted by
the worker consumes the dirty flags even if the later network operation fails.

The worker creates a server session, signs subsequent update/delete batches,
and caches the accepted advertisement when the game pumps its task. Continue
returns running until HTTP completion, then XONLINETASK_S_SUCCESS or an error.
GetInfo returns the same eight-byte XNKID/session ID and locally generated
16-byte exchange key for create/update tasks.

Closing the task keeps the lobby;
Delete removes it. Sign-out and controller removal attempt a signed delete;
server expiry handles unreachable clients. Presence pings refresh the host's
advertisements without writing events.

No-op updates add no events.

The exchange key stays in the host HLE. Public session responses and events
contain the XNKID and host XNADDR, never the exchange key. Search records use a
zero exchange key because the current HLE sockets have no encryption. The
retail host wrapper at `0x0008fd90` gets the ID/key and then reads its XNADDR;
`0x00072f39` registers the returned pair before QoS listening.

The separate
XNetCreateKey/System Link path does not supply this pair.

Search and FindFromID fetch open sessions and own their completed snapshots
until TaskClose. SearchGetResults returns an array of guest pointers. Each
84-byte header contains the ID at +4, XNADDR at +0x0c, zero key at +0x30, open
public/private counts at +0x40/+0x44, filled counts at +0x48/+0x4c, and column
count at +0x50.

An offset directory follows. SearchParse decodes integer,
UTF-8 wire string to UTF-16 guest string, blob, and NULL columns into the
caller's specifications, checking all bounds before writing the output.
Result storage is reusable when a search task closes; handles remain monotonic.

The title's procedure 1 returns seven integer columns for attribute IDs
1, 2, 3, 5, 6, 7 and 8 (`0x000b4b80`, `0x000b4a40`). Missing values default to
zero. Its twelve positional integer/NULL query parameters are not yet fully
identified: this procedure currently searches all open sessions, without
inventing filters for anonymous ID-zero parameters.

Other procedures accept
named attribute equality filters and return the stored columns in ID order.
FindFromID uses the seven-column title projection. SearchResultsLen retains
the retail capacity formula at `0x003b32f9`: 92 bytes plus the requested row
count times an 84-byte header and each column's eight-byte overhead/value
capacity.

Limits are 1000 rows, 64 columns and 16 MiB; the mapped result pool
also has a finite allocation budget and reports OUT_OF_MEMORY when exhausted.

The host address is the configured XNet LAN/public IPv4 and base port. Set
`H2_XNET_ADDR` to an address peers can reach, with distinct ports for two local
instances as described above. The server's HTTP tunnel does not forward game
UDP. Internet peers need reachable UDP base through base+6; this slice adds no
relay or hole punching.

Server clients that omit their declared IPv4 use the
trusted public-URL source-IP policy ([live-api.md](live-api-sessions.md#sessions)).

Synthetic tests cover the nine entries and their stack cleanup, local fallback,
typed values, dirty flags, stable IDs/keys, task lifetimes and capacity errors.
The server tests exercise authorisation, signed batches, projection/replay,
filters, expiry and tunnel source trust. Two synthetic guests publish/search
through an embedded memory-backed server and read the rows and parsed values
that the retail wrappers consume.

No test runs the game.

## Client HLE: Teams

The clan-create stop at `0x003afb80` is called by `0x000abe60` from the UI action
`0x00238bf1`. It passes a maximum of 100 members and creator privileges
`0xffffffff`. All thirteen linked XOnlineTeam entries use the same owned store and task seam:
create/delete, enumerate-by-user/results/details, member enumeration/results/
details, member property changes/removal, recruitment by XUID/name, and answers.

The game refreshes enumeration after mutations; no CreateGetResults or
TeamSetProperties thunk is called in the verified boundary. No guest address is
invented for those unused APIs. The host seam supports team-property changes.

The shared owned model/rules are in `live-proto::team`; guest marshalling is in
`live-hle::teams`. A team properties record is 1670 bytes (UTF-16 name 16 units,
description/motto/URL 256 each, WORD custom length and 100 bytes custom data).
A full team is 1698 bytes: 12-byte XUID, properties, DWORD flags, FILETIME and
DWORD active member count.

A member is 146 bytes: 12-byte XUID, 16-byte ANSI
gamertag, 106-byte properties, DWORD flags and FILETIME. Outputs are checked and
written exactly to these bounds. The count in enumeration results is OUT, not
an input capacity; limits keep lists within eight teams and 100 members.

Each mutation returns a task; Continue reports success or the specific Teams
HRESULT. Enumerations own snapshots with their detail records. Missing items in
a successful snapshot return E_FAIL, matching retail. The store retains owned
strings/custom data after guest buffers are reused.

It survives local sign-out;
cleanup invalidates tasks. With no server configured, recruitment resolves the host accounts file,
records a pending member and retains the outgoing message capture.
Pending teams appear for the recruit with the recruit flag; an accepted answer
moves the member into the roster, and decline removes the pending request.

Privilege bits are delete=1, modify-team=2, modify-member-permissions=4,
remove-member=8 and recruit=16. The title uses all bits for the leader, `0x1c`
for officers, and `0x10` or zero for lower ranks. The shared rules preserve one
creator/leader, reject orphaning/demoting it, require the appropriate permission
for mutations, prevent privilege escalation, and allow an ordinary member to
leave.

Duplicate names, full teams, eight-team limits, unknown users and missing
teams produce their XDK errors. Local diagnostics use `H2_BOOT_VERBOSE=1`.

`Hle::local_teams()` snapshots the cache; `local_team_action` is the trusted local
host seam, including team-property edits where no guest thunk is linked. The trusted host action seam is available only without a configured server.
Local-only records are not persistent across runtimes.

Synthetic guest
tests cover create/list/details/member editing/recruit/accept/decline/leave/
delete, packed layouts/canaries, snapshot ownership, validation/fault atomicity,
permissions and cleanup. No test builds or runs the title.

### Server-backed Teams

With `LiveConfig.server_url` set, the thirteen entries submit owned requests to
the existing HTTP worker. No HTTP work runs on the guest thread. Tasks remain
running until the response arrives; success snapshots populate the local cache,
and server permission/limit errors finish the task with the XDK Team HRESULT.
Transport failures report cannot-access-service without accepting a local write.

Closing a task drops its result; it does not undo an event already sent.

Creation, property changes, deletion, recruitment, answers and removal are
signed account events on `team-<id>`. The server owns the persistent projection
and validates the same rules as the offline store. Enumeration reads its account
and team endpoints.

Recruitment resolves registered accounts by name or XUID,
so another user's private key need not be in the host accounts file. Guest
buffers and keys never enter diagnostics or the request queue as raw pointers.
Remote records are bounded and validated before packing SDK records.

A recruit also gets `TeamRecruitReceived` on their account stream. Pending
teams appear in their team enumeration with the recruit flag; accepting makes
them active members. The HLE does not yet translate that account event into a
Live inbox message or toast, and remote recruitment does not forward optional
voice/text message attachments.

The team invitation and answer themselves are
persistent. Team-property edits are available through the server/CLI; the
verified title boundary has no linked TeamSetProperties entry to register.

`h2 live team` creates, lists, edits, recruits, accepts/declines, changes member
permissions, removes/leaves and deletes teams; `h2 live q teams [gamertag]`
reads the views. See live-api.md's Teams section for arguments and wire records.
Two simulated guests and an embedded server test this lifecycle, including
separate account files, pending flags, officer rights, denial and network
failure.

Server tests cover signed authors, mailbox append, replay, concurrent
name conflicts and private reads; a CLI test drives signed commands through the
same endpoints. These tests do not build or run the title.
