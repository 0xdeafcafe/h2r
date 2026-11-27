# Bots: Clanker appearance and Live player options

Status: part of [bots.md](bots.md); see there for status, labels and the index. This page: Clanker appearance and Live player options.

## Clanker appearance and Live player options

A Clanker's full gamertag determines its emblem and armour colours in
System Link, simulations and Live rosters. Changing hosts or account
XUIDs keeps the same look for the same name.
The profile model still comes from the host. Team games can override the
visible armour colour.

`live_proto::bots::appearance` lowercases the tag, collapses whitespace and
hashes its UTF-8 bytes with 32-bit FNV-1a and a fixed final mix. The low
bits select one of 64 foregrounds and 32 backgrounds. The two foreground
colours come from one brightness group and the two background colours from
the other, using four different retail palette indices.

Rusty Lugnut has foreground 1, background 30 and profile colours 10, 11, 13,
16. The server recomputes looks when projecting every `BotIssued`, including
old signed events, so existing accounts migrate without rewriting their log
or changing their personality. The game and both Live homes use the pure
Rust implementation.

The web mock uses a C mirror compiled as inline WebAssembly. Rust and
browser tests read the same 8,192-name fixture and check its fingerprint.

After changing `web/src/lib/clanker_appearance.c`, regenerate the browser
module with `python3 scripts/clanker-web-kernel.py` using LLVM clang and
wasm-ld. The web tests check the source digest.

**Retail trace.** The main-menu tags identify the pregame lobby as
`0x0e`, the expanded/collapsed Live Y menus as `0x1a/0x1b`, and their
`player_selected_dialog` as `0x1f`. Zanzibar's in-game Live menu is
`0x1c` and also has `0x1f`. There is a separately named
`player_selected_dialog` tag `0x20` in the main menu. The presence of
that tag alone does not establish a loader for it.

- The friend-list press at `0x2b3870` copies the selected friend (record
  kind 2), commits it at `0x2b38ed` and opens through `0x2b71f0`.
  The list handler at `0x2b3e4d` builds kind 1 from the selected row and
  opens through `0x2b7212`. Another player selection at `0x23403c`
  copies the XUID and name at `0x234100` and uses `0x2b7223`.
- Both options loader families reach `0x2b70a3` or `0x2b71a6`.
  They take the request in EAX, source in ESI, and return the screen in
  EAX. Their shared constructor `0x2b6106` builds screen 31 with its
  action list at screen + `0x614`. The list is populated at `0x2b55ae`
  and its press handler is `0x2b5d3b`.
- `0x148893` writes the selected-player record at `0x54dc6c`.
  For kinds 1 and 2, +4 holds the eight-byte XUID, +12 the flags and
  +16 the ASCII gamertag. Kind 3 has only a name. The hooks leave
  name-only rows and human accounts on the retail path. A remote
  phantom needs a matching, gear-marked session slot.
- Recent Players uses retail's history. `RECENT_PLAYERS_BUILD` copies
  its records into the list, and `clanker_card` restores the display
  prefix on a reserved Clanker XUID. Selecting the row opens the
  read-only card with Gamertag, Difficulty, Style and Back. Its large
  dialog borrows the Controls screen's localized string list so its
  labels resolve even after the phantom leaves the session. Skin 6 of
  the current interface contains the Y menu's player table. Its nested
  skin's first shader bitmap uses the medium emblem material in the
  main menu, shared map and Zanzibar. The card copies that definition
  and feeds the profile to the retail emblem setter at `0x2b12ca`.
- The profile line sits inside the dialog above the emblem and uses the
  large panel's animation. It shows the gamertag’s try-hard band and
  strongest combat tendencies. The host shows the active brain’s
  difficulty and strongest temperament trait. A remote or recent server
  bot resolves its full name and last rostered skill through
  `GET /v1/games/bots/{xuid}`. The client derives the baseline temperament
  from that name and skill. While a queued lookup runs, Difficulty and
  Style show “Loading...” and the profile uses the available name. If the
  lookup fails or no skill is known, those rows show “Unavailable”. On
  the host, a scripted or recorded seat with no brain shows “Unavailable”
  for its combat profile. The card's page model is available in the UI
  builder as `clanker_player_options`.
- Mute and feedback are omitted from the card. A Clanker has no voice
  account to mute and no human player to report. Both server homes
  refuse friend requests and mailbox writes to either reserved XUID
  range with HTTP 403. Retail text and voice sends share the HLE outbox,
  which refuses the entire recipient list if it includes a bot.

**Needs a game run.** The common constructors and record layout are
confirmed statically. Which handler each retail lobby, in-game, recent and
friends row exercises still needs input-driven confirmation. Select a bot
and a human from each available surface, including a recent bot that has
left the session.

Check `h2 agent ui` for `dialog.name = clanker_player_options`, the rendered
emblem, difficulty and style, and return to the invoking list with B or
Back. Repeat with a remote client and with a human guest signed in on a
second controller. Check the same name after restart and in a non-team
System Link game against its Live card and web emblem.

Use a legacy friends entry if available, since new friend requests to bots
cannot create one. The menu parity check visits recent entries, verifies the
Clanker card class and reports missing widget text. [Crawling the
menus](../runtime/agent-api-host.md#crawling-the-menus)
