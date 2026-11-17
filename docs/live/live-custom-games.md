# Custom games browser over Live

Status: the server, HLE and boot pieces are in and synthetically tested.
The owner has Live working in the game (2026-11-17); §7 is the in-game
checklist.

What it adds:

- An Xbox Live menu row, **Custom Game Browser**, directly above Content
  Download. It opens Halo 2's own System Link game browser in the Live mode
  the title shipped with but never opened. The list fills with the custom
  games the server lists for you. The menu shows all five rows without
  scrolling (§5).
- **Content Download** opens a short dialog saying how to install the
  Multiplayer Map Pack ([map-pack.md](../game/map-pack.md) §4) instead of leaving
  for the dashboard's download area, which does not exist here.
- A **visibility** on every session: `public`, `friends` or `invite`. The
  host sets it with the party privacy screen the title already has.

Joining uses the browser's own join and the existing XNet path: direct UDP to
the host, as described in [live-matchmaking.md](live-matchmaking.md) §3.6.
Invitations keep working whatever the visibility.

Labels follow [live-boundary.md](live-boundary.md): **confirmed** means read
statically in the retail code, **likely** means consistent with that code but
not proven. Addresses are guest VAs in the retail 5849 image.

## 1. How the System Link browser works

**Discovery on a LAN (confirmed).**

- `0xb2ea0` runs every tick. About every 1500 ms it sends search message 2,
  `{u16 2, nonce}`, to `255.255.255.255:1001`. The call site is `0xb2f29`.
- Hosts answer in `0x93fa0`..`0x940a1` with message 3 to the same broadcast
  address and port. The reply is 0x720 bytes: version, the search's nonce,
  then 0x714 bytes of game description built by `0x64d70`.
- The description holds:
  - the XNKID at `+0x58`;
  - the XNKEY at `+0x60`;
  - the host XNADDR at `+0x70`;
  - the privacy at `+0x14`;
  - a name, player counts and per-player arrays (likely).
- The receiver at `0x940b0` matches the nonce and stores entries with
  `0xb2fc0`. Entries live at `[0x4d8ec8]`, stride 0x784, and are deduplicated
  by host XNADDR. An entry not refreshed within 2000 ms is dropped at
  `0xb2f60`.

**Who answers (confirmed).** `0x59840` passes only when the squad session's
class `[session+0x18]` is 1 (System Link) and its state is 5..8.

- The classes are 0 offline, 1 System Link and 2 Live. `0x59890(class, ...)`
  is called with 2 from the Live paths `0x199e1f`, `0x23100d` and `0x23107b`.
- So **a Live party never answers a LAN search**. Relaying broadcast between
  internet hosts would not work for Live parties.

**The Live mode (confirmed).**

- The browser screen is `0x0d`: vtable `0x45a558`, 0x940 bytes, constructor
  `0x253221`. It has two loaders that differ only in the byte at
  `screen+0x93c`:
  - `0x253185` (0) is what the main menu's System Link row opens, via
    `0x252433` → `0x25286d`;
  - `0x2531c9` (1) is referenced only by the screen's own re-open getter at
    `0x253214`, so retail never opens it.
- On activation, `0x2532b7` → `0x199b33` → `0xb3570` stores the flag at
  `[0x4d8f08]`. `0xb3610` then runs the Live search `0xb3100` instead of the
  broadcast `0xb2e30`.
- The Live search:
  1. `XOnlineMatchSearch`, procedure 1, 50 results (`0x8fa80`).
  2. Results are read by `0x8fb30`, which keeps only rows with exactly seven
     attributes. Each becomes a record with the XNKEY at `+0`, the XNKID at
     `+0x10`, the XNADDR at `+0x18` and the slot counts.
  3. `0xb3500` turns the records into entries.
  4. Each host is probed with `XNetQosLookup` (`0x7b4c0`).
  5. The host's QoS listener data is decoded by `0x7c530` into the same
     0x714-byte description.
- Hosts publish that description as listener data: `0x738f0` bit-encodes the
  `0x64d70` output and `0x731d0` passes it to `XNetQosListen` (likely). Since
  `0x64d70` needs only class != 0, Live hosts publish it too.

**Join (confirmed).**

- From an entry, `0x700f3` calls `0x59e50(session, class, XNKID, XNKEY,
  XNADDR, ...)`. That function:
  - registers the key with `XNetRegisterKey` (wrapper `0x7a920`);
  - resolves the XNADDR (`0x7ab10`, port 1001);
  - connects (`0x76aa0`).
- The host (`0x94100`) finds its session by XNKID. Its validator `0x62fe0`:
  - accepts only state 5 (the lobby);
  - refuses privacy above 1 with code 2;
  - refuses a full lobby with code 4.
- No class check was found on the host side (likely).

## 2. Design

The browser's Live mode already does everything a custom games browser
needs. Phase 1 already implemented each piece it calls:

| Browser step | Already implemented by |
|---|---|
| Search: procedure 1, seven integer columns for IDs 1, 2, 3, 5, 6, 7, 8 | `GET /v1/sessions` ([live.md](live.md), "Client HLE: sessions and server discovery") |
| Each row's XNADDR | The HLE writes the session's endpoint into it, so the QoS probe and the join go to the advertised address |
| QoS listener data | Answered between HLE peers on UDP base+2 ([live.md](live.md), sockets) |
| XNKID | The server's session ID. The host registered the same ID through `XOnlineMatchSessionGetInfo` (`0x72f39`), so its find-by-XNKID matches |

Two things remain:

1. **Reaching the screen.** The Xbox Live menu gets a fifth row that opens the
   Live-mode loader (§5). No new screen and no packet synthesis are needed.
2. **Who sees what.** The session gets a visibility. The server filters its
   list by the reader, and the HLE already sends a signed `read` token with
   every search, so the browser shows exactly what the server lists for that
   player.

Rejected alternatives:

- **Answering LAN searches with entries built from server sessions.** That
  needs the title's bit-packed message-3 format and a fake nonce echo, and
  Live hosts' real data would never be in it.
- **Fanning broadcast out to internet hosts.** Live hosts do not answer
  searches (class 2, above), and a host's reply XNADDR is its LAN address.

## 3. Visibility

`live_proto::session::Visibility` has three values:

| Value | Who `GET /v1/sessions` lists it for |
|---|---|
| `public` | everyone (the default, and how every older session reads) |
| `friends` | the host and the host's friends (the friends graph of [live-api.md](live-api.md)) |
| `invite` | the host only |

- Joining by ID (`GET /v1/sessions/{id}`, `XOnlineMatchSessionFindFromID`,
  invitations) ignores visibility: the 64-bit ID is the capability.
- A `read` bearer is optional on a public server. When one is sent it is
  checked, and the account it names is the reader. A bad token is a 401, not
  an anonymous read.
- `NewSession.visibility` is omitted on the wire when public, so older
  servers and logs read the same bytes. `SessionUpdated` may carry
  `visibility`. The HLE sends it only when it changed.
- `?visibility=` filters the list further.
- **Limit:** session events are in the public event log like every other
  stream, so an invite-only lobby's existence is visible to someone reading
  `/v1/events` on a public server. Use `--private` if that matters.

**The host's choice (confirmed retail UI).**

- The party privacy screen is screen `0x19`: loader `0x2325fb`, list "squad
  privacy setting list".
- It offers item 0 (open), and items 1 (friends) and 2 (invite only) only
  when signed in to Live (`0x6c7e0`). Its text ids are `0x04000201`,
  `0x0f000202` and `0x06000203`.
- It opens from the squad settings screen, button event 5/2 at `0x2bbed1`
  (likely X).
- A press calls `0x19a148` with the item id in EAX. That stores the privacy
  into the squad session at `+0x498c` (`0x64230` → `0x5bde0`).

Boot ([`crates/boot/src/custom_games.rs`](../../crates/boot/src/custom_games.rs))
does two things with it:

- It follows that press into the `live.visibility` setting (`H2_LIVE_VISIBILITY`,
  `public`/`friends`/`invite`). The setting is saved, and the HLE pushes it to
  any open lobby at once as a signed `SessionUpdated`.
- It installs an HLE probe that reads the squad session's privacy (gates as
  `0x64230`: life cycle ready `[0x527330]`, session `[0x527364]`, state 3..8)
  every time the game creates or updates its advertisement. The title's
  value wins over the setting, so a new party's default and the advertised
  visibility cannot disagree.

The host's join check (`0x62fe0`) refuses with code 2 while the privacy is
invite only. Whether an invited joiner bypasses that was not traced; §7
checks it. Friends-only is enforced by the server's lists, not by the host.

## 4. Address restriction

A listed session points every browsing console's QoS probes and joins at its
endpoint. The server previously took any declared IPv4. Now:

- **Listed sessions (`public`, `friends`) take the observed address.** When
  the server sees the host from a non-loopback IPv4
  (`PublicUrl::client_ip`, so a configured Cloudflare tunnel's
  `CF-Connecting-IP`, [live-hosting.md](live-hosting.md)), that address is
  the endpoint, with `source: "observed"`. Any declared address is ignored.
  This also fixes the common case of a host behind NAT declaring its LAN
  address.
- **A declared address is kept otherwise:** invite-only sessions, a host on
  the server's own machine (loopback), or no observation. Such a session is
  listed only if the address is local-network (RFC 1918, loopback,
  link-local, 100.64/10): `Endpoint::listable`. A declared internet address
  stays invite-only in effect, even if updated to `public`.
- The port is still the host's declaration. Direct play still needs UDP
  base..base+6 reachable on the host.

## 5. The Xbox Live menu

Static reading of the retail XBE (`h2 disasm`) and the menu's screen tag
(`h2 ui`); no game run. Addresses are confirmed unless marked.

**Rows.** The Live menu (screen `0xba`, loader `0x2310b7`, constructor
`0x2310f9`) keeps its list handler at `screen + 0x818` (vtable `0x458b48`,
constructor `0x230ea2`). The handler builds the four-row array "xbox live
menu list" and embeds four item widgets at `+0x88`. The rows are
Quickmatch, OptiMatch, Custom Game and Content Download. Content Download
asks to leave for the dashboard (`0x19b5af`, error `0xa3`, then
`0x230e98`).

The list hook makes it five rows, and the row hooks show them in this
order:

| Row | Shows | Text | Press | Pane |
|---:|---|---|---|---|
| 0–2 | Quickmatch, OptiMatch, Custom Game | retail | retail | retail 0–2 |
| 3 | Custom Game Browser (ours) | ours | Live browser (§2) | 4 (ours) |
| 4 | Content Download | retail id `0x10000445` | Map Pack dialog | 3 |

Rows 3 and 4 never reach the retail row text or press. The retail press
remembers rows 0 to 2 for the next visit (`[0x510a18]`, read by `0x231170`
when it is under 4). Our row's press stores 3 there, so the cursor comes
back to it (below, "Back from the browser"). Content Download stores
nothing, as in retail.

**The fifth item widget.** A list scrolls when it has more rows than child
item widgets. `0x24c515` compares the row count with the children that
`0x24c177` counts. So the fifth row needs a fifth child, made the way the
Controls list's added rows are (`speaker_row`):

- the shared "add items" method (`0x24bb60`, vtable slot 17) links the
  handler's item widgets as children;
- the hook runs it, then, for a handler with the Live menu's vtable, builds
  one more 0x80-byte item in our memory: zeroed, the item constructor
  (`0x24abae`, the same one the handler uses), the local player
  (`0x22ee92`). It links the item after the four (`0x22eb18`);
- one item per open screen (8 slots), rebuilt when a handler is new, and
  moved back to the end when the original re-links the four.

The tag's list already shows up to seven items (`+6` of the list block),
so its visible count need not change. The tag copy raises it to five if
it were lower.

**Positions.** `0x24bc12` lays the children out from the list's anchor,
each one item height below the last. The height is the item's bounding box
(`0x2bafa4`). With the default list skin that is its text (top 28) down to
its background bitmap (bottom −15): 43 units. The end-cap bitmap is flagged
out of the box. In the retail tag the hint text starts 21 units below the
fourth item, in the list's column, so a fifth item 43 units lower would
cover it. The screen-tag hook (`0x22f871`) therefore gives the tag copies
of its panes (`custom_games::extend_tag`):

- the list's anchor moves up by 21;
- every text and bitmap that starts below the fourth item, in the list's
  column (less than half an item right of the anchor), moves down by 22.
  Those are the two hint texts and the Y callout. The message panel to the
  right and everything above stay put;
- together that is one item height, so the five rows keep the retail
  spacing. The gap under the last row stays 21, and the first row stays
  36 units under the header (57 before);
- a fifth pane is added for our row: a copy of the Custom Game pane, its
  picture included, with our help text in place of the one text that
  differs between panes.

The copies (about 2.7 KB) live in our data page. The retail arrays are not
written, and the copies are reused when a map load brings the tag back.
`0x230427` (pane = base + row, no bounds check) is how the list shows a
row's pane when its focus moves (`0x24c50a`). The pane hook hands it 4 for
our row and 3 for Content Download, through a word of ours rather than the
caller's. On a tag that was not extended our row keeps the shown pane.

**Back from the browser.** The game keeps no history of screens. A
screen's back handler opens its parent by loader, with a back request
(kind 7, built by `0x149f49`), and the screen push (`0x147f6d`) replaces
the window's screen. The browser's handler (slot 10, `0x2536a6`) does that
on B or Back (event 5, button 1 or 13) unless `0x148044` says otherwise,
and it always names the System Link screen's loader `0x252433` (that
screen in its LAN mode, `[screen+0x610] = 2`), whatever the byte at
`+0x93c`. On the LAN path that is where the player came from. From our row
it is not, and in game back landed on the controller screen.

The Live menu's own children are opened the way our row opens the
browser (OptiMatch: `0x149f1e(1 << player, 0x230c8d)`), so nothing about
the opening needed to change. Retail goes back to the Live menu with a
kind-7 request for `0x2310b7` (`0x19b0ad`). So boot hooks two functions:

- the browser's event handler: when the screen's vtable is `0x45a558` and
  its `+0x93c` byte is set (only our row opens that mode), it raises a
  flag, runs the original and clears the flag;
- the System Link loader `0x252433`: with the flag up and a kind-7
  request, it points the request's loader (`+0x1c`) at `0x2310b7` and
  calls the Live menu's loader with the same request. Otherwise it is the
  original.

A LAN browser never raises the flag, so main menu → System Link → back is
retail. Our row press also stores row 3 in `[0x510a18]`, the row the Live
menu's cursor starts on (`0x231170`, rows 0 to 3; the retail press stores
rows 0 to 2 the same way), so back lands on Custom Game Browser.

**The Map Pack dialog.** The game shows its errors in two dialog classes
that look an error id up in the shared UI globals (`0x23661f`: ESI = a
0x20-byte entry, the id on the stack, `ret 4`; an unknown id gets error 1's
entry). The entry's string ids (title `+0xc`, message `+0x10`) are
resolved by the text fill (`0x19b72a`) through the localized-string
lookup (`0x1a0180`), which serves our ids. Content Download calls the
one-button class (`0x19b527(1, error, 4, mask, 0, 0)`, `ret 0x18`), the
way `0x2308ab` shows its own errors. Its button closes the dialog when
there is no press callback (`0x19b97a`). The error id is
`custom_games::MAPPACK_ERROR`, one the table does not have, and the lookup
hook writes our title and message ids into its entry. The rest of the
entry stays error 1's. The one-button dialog takes its button label from
its own screen tag. This does not touch the Live notice
([live.md](live.md), "Live not set up"), which wraps the fill and changes
only error `0x23`.

```text
Multiplayer Map Pack

Put mappack.iso in the same folder
as halo2.iso, then start the game.
The maps install on that start.
```

Each line renders under 220 units in conduit-12, inside the dialog's
roughly 240-unit message box. "Custom Game Browser" renders 136 units wide
(180 in conduit-16), inside the skin's 190-unit text rectangle, so the
label is not shortened.

## 6. Implementation map

| Piece | Where | Tests |
|---|---|---|
| `Visibility`, `Endpoint::listable`, `Session::listed_for`, update | `crates/live-proto/src/session.rs` | unit: defaults and wire omission, the visibility matrix, declared addresses |
| Create rule, reader-filtered list, `?visibility=` | `crates/live-server/src/sessions.rs`, `Live::read_caller` | `crates/live-server/tests/session_visibility.rs`: anonymous/stranger/friend/host lists, update, observed vs declared, loopback |
| HLE: advertisement and change-only update carry it; `Hle::set_session_visibility`; `Hle::set_visibility_probe` | `crates/live-hle/src/sessions.rs` | `crates/live-hle/tests/server.rs`: `browser_search_rows_follow_the_hosts_visibility_setting` (procedure 1, 50 rows, seven columns, as the browser asks; friend sees it, stranger not; live switch; FindFromID still works), `the_titles_privacy_probe_decides_a_new_advertisement` |
| `live.visibility` setting, HLE binding | `crates/boot/src/setting_defs.rs`, `live_glue::bind_visibility` | setting env and steps reach the HLE |
| Live menu rows, fifth item widget, tag, Map Pack dialog, back from the Live browser, privacy hook and probe | `crates/boot/src/custom_games.rs`, `layout_glue::install` | count grows only the Live list; row order, texts and panes; the fifth child item; the tag's fifth pane and moved blocks on a synthetic tag (and what is left alone); the dialog entry; row/mask argument reads; only a Live-mode browser is redirected, only a back request is rewritten, our press stores a row the cursor restore takes; privacy mapping and squad gates; install without a game |
| Web "Find a game" shows "open to", hides invite-only | `web/src/lib/sessions.ts`, `web/src/dash/Sessions.tsx` | `web/test/sessions.test.ts` |

`H2_NO_CUSTOM_GAMES_ROW=1` leaves the Live menu alone: four rows, and
Content Download as retail.

With Live on, a sixth row, **Co-op Campaign**, sits between Custom Game
Browser and Content Download, with its own pane (the Custom Game pane with
its help text); the list keeps five item widgets, so Content Download
scrolls into view ([live-coop.md](live-coop.md) section 10).
`H2_NO_COOP_ROW=1` takes it out (five rows, as above).

## 7. To check in game

Nothing here ran in the game. With two consoles (or two instances, per the
[live.md](live.md) recipe), each signed in to the same server, make A and B
friends. Then check:

1. **The rows.** Xbox Live menu: Quickmatch, OptiMatch, Custom Game,
   Custom Game Browser, Content Download, all five on screen with no
   scrolling.
   - The five rows are evenly spaced, the first a little higher than
     before, and the hint text and Y callout under the list sit a little
     lower, with the same gap under the last row as before. Nothing
     overlaps; the callout stays inside the screen's safe area.
   - "Custom Game Browser" fits on its row without being cut off.
   - Moving onto Custom Game Browser shows the Custom Game picture with our
     help text ("Find custom games hosted on your Live server and join
     one."); onto Content Download, its own picture and help.
   - Leaving the menu and coming back (and after playing a game, which
     reloads the main menu) still shows five rows.
   - stderr says `custom games: xbox live menu tag has 5 panes and room for
     5 rows`, `speaker row: add items active` and `custom games: row text`
     the first time.
   - If only four rows show and the list scrolls, the fifth item widget was
     not linked (the list handler's vtable differs from `0x458b48`). If the
     panes do not follow the rows, the list does not report focus through
     `0x230427` (likely, not traced for this list).
2. **Content Download.** Pressing it shows a dialog titled "Multiplayer Map
   Pack" with three lines on `mappack.iso` and one button. The button closes
   it and the Live menu is still there. stderr: `custom games: showing the
   map pack dialog`. If the dialog is blank, the text lookup did not take
   our ids (check `custom-games:dialog-text`).
3. **The browser.** Pressing Custom Game Browser opens the System Link browser screen.
   - It should search over Live: the HLE logs a session search, and no LAN
     broadcast goes out.
   - The title may still say "System Link" in its header.
   - Press B. The Xbox Live menu comes back with the cursor on Custom Game
     Browser, not the controller screen. stderr: `custom games: back from
     the live browser to the xbox live menu`.
   - From the main menu, System Link, then B in its browser: the same
     screens as before this change (retail), and no such stderr line.
4. **A friend's game.** A creates a party and starts a custom game lobby. On
   B, the game appears in the browser with A's name, map and player count.
   This proves the QoS listener data is decoded.
5. **Joining.** B joins from the browser and lands in A's lobby, then plays.
   Watch for a class mismatch: the join-target class from a Live-mode entry
   was not traced (§1).
6. **Visibility.** On A, open the party privacy screen from squad settings
   (likely X) and pick Friends Only, then Invite Only.
   - stderr shows `live.visibility = ...`.
   - The web's Find a game shows "open to: friends only", then the game
     disappears.
   - A third, non-friend account stops seeing it in the browser at Friends
     Only. B stops seeing it at Invite Only, but an invite from A still
     works.
7. **Across the internet.** A behind a forwarded UDP 3074–3080. The session's
   endpoint on the web/`h2 live q session` is A's public address
   (`source: observed`), and B joins from outside.

## 8. Open questions

- How the fifth row and the moved hint text look on screen (§7, 1).
- The joiner's session class when joining from a Live-mode entry, and
  whether class-2-only paths after admission (`0x6e468`) misbehave.
- The meanings of the browser's seven search attributes, and whether the
  browser should filter out hopper (matchmaking) lobbies. The server lists
  every open session today; a `flags=custom` filter is one query parameter
  away once procedure 1's parameters are identified.
- Pushing the saved `live.visibility` into a new party's privacy, so a
  player's choice carries to their next party. Today the title's default
  wins.
