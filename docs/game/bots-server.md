# Bots: the server interface

Roster, results, trust and bot accounts (section 9). Part of the [bots design](bots.md); the labels (traced, Repo, Design, Run) are defined there.

## 9. The server interface (for sbmm-ranked)

The sbmm-ranked lane owns the server side: the matcher with skill, ranked
arbitration, per-playlist rank, and the account side of **persistent bots**
(real bot accounts with their own rank that rises and falls like a human's).
The host runs the bots. This is the one interface between them.

### 9.1 The roster: server to host

The server tells a host which bots to run with a **bot roster**. In
Clankermatch it comes in `MatchProposed` on `match-<id>`, as `bots`. In a
custom game the host asks `POST /v1/games/bots` with
`{session_id, playlist?: playlist_id, count, skill?, teams}` and gets one
back; without a
server (System Link, offline) the host makes its own from the Clankers page.

```json
{
  "match_id": "…",
  "bots": [
    {
      "xuid": "000b…",            // issued by the server; bots have a range of their own
      "gamertag": "Rusty Lugnut", // passes live_proto::Gamertag
      "persistent": true,         // a persistent bot (ranked) or made up for this match
      "emblem": { "foreground": 12, "background": 3, "colors": [1, 7, 0] },
      "colors": { "primary": 4, "secondary": 9 },
      "rank": { "level": 17, "playlist": "…" }, // what the lobby shows
      "team": 1,                  // team index, or null for the host to auto-balance
      "skill": 0.62,              // 0..1, continuous; maps onto §3.4 (0 Easy … 1 Legendary)
      "personality": {
        "seed": 1234567,          // match RNG for this account; the gamertag supplies its combat profile
        "aggression": 0.5,        // 0..1
        "caution": 0.5,
        "objective": 0.7,
        "teamwork": 0.5,
        "taunt": 0.3              // 0..1; scaled by severity; 0 = never (§3.5)
      }
    }
  ]
}
```

Rules:

- `skill` is the only difficulty input. The four named difficulties are
  0.0, 0.33, 0.67 and 1.0. For Clankermatch the server sets it from the
  human's level in that playlist.
- `team: null` means "host balances". The host balances by total skill.
- A made-up bot (`persistent: false`) still gets an `xuid` from the server,
  so the carnage report and stats can name it. The server owns name, emblem
  and rank for every bot, so the web shows the same bot the lobby did.
- The host never changes identity, rank or skill; it may only lower
  `count` if the session is smaller.

### 9.2 Results: host to server

These go in the normal game record (`games.rs`, the Live game recording),
with these additions:

- `kind: "clankermatch"`, `"custom"` or `"matchmade"` on the game;
- `bot: true` on each bot's player entry, keyed by its `xuid` when it has
  one. A rostered phantom carries its account's XUID (§9.5). A
  host-made phantom has `bot: true` and no `xuid` (§6).

Per-player stats are the same as a human's. Persistent bots are ranked
from these by the server's normal per-playlist rank update, which treats
bots like humans. Whether ranked arbitration should count a bot's report
is sbmm-ranked's call: the host is the only reporter for its bots.

### 9.3 Trust

The roster is signed by the server like `MatchProposed`. A game record with
`bot: true` entries is accepted only for xuids that the server marked as
bots in that match's roster, or in the `/games/bots` answer. A console
cannot claim a human as a bot, or a bot as a human. A `bot: true` entry
with no `xuid` is accepted with or without a session: it names no account,
so there is nothing to claim, and it counts toward no one's career (§6).

### 9.4 Interface with ranks (answers to live-matchmaking.md §8.4)

Ownership: this lane owns the bot accounts and their events (`POST
/games/bots`, the roster, `bot: true` on a game's entries, the `0x000E`
range); sbmm-ranked owns how bots are ranked and arbitration.

1. **Marking a clanker hopper.** Not in the game's `0x614`-byte
   descriptor: every byte there is a game field the validator
   (`0x193560`) reads, and the game never needs to know. The mark is in
   the server's hopper source, beside the descriptor it writes:
   `clankers: Option<Clankers>` on the playlist, with `Clankers { ranked:
   bool, ranks_as: Option<u32> }`. `ranked` says whether finished games move
   a standing; the descriptor remains an unranked type so one human and bots
   can fill the lobby. `ranks_as` is the playlist id of the human hopper
   whose standing it shares (`None`: its own rank playlist). The server
   serves the mark with the hopper list (per descriptor index), and live-hle
   reads it for the menu row's filter (§8.3) and for the search (§8.4).

   **Disagreement, small:** §8.4 has the clanker hopper carry the *same*
   descriptor playlist id (`+0x08`) as the human hopper. Two descriptors
   with one playlist id in one file is untested against the validator and
   the playlist UI (`0x65530`). With `ranks_as` the clanker hopper keeps an
   id of its own, and the server answers its stat read (`GET
   /v1/rank-stats` for that playlist id) with the `ranks_as` standing, so
   the lobby shows the same rank either way. If a test of the validator
   shows duplicate ids are fine, `ranks_as` can default to the
   descriptor's own id and nothing else changes.
2. **The read-model lookup.** A persistent bot is in many matches, so the
   level a roster showed belongs to the rostering, not the bot. Two
   lookups:
   - `bot(xuid) -> Option<&Issued>`: the server issued this xuid as a bot
     (`BotIssued` on `bot-<xuid>`, written by the server; built in 6c as
     `live_server::bots::Bots::bot`).
   - `rostered(session_id, xuid, before) -> Option<(&Rostered,
     &RosteredBot { persistent, level, team, skill })>`: from
     `BotsRostered { session_id, host, playlist?, bots }` on
     `bots-<session_id>`, written by the server when it answers `POST
     /v1/games/bots` or proposes a Clankermatch. A session can roster more
     than once (a later roster replaces the earlier), so `before` is a log
     position and the answer is the latest roster before it: `rank_game`
     passes the game's `GameStarted` position (`Game::position`), the
     roster in force when the game started. Also `roster_at(session_id,
     before)` for the whole roster. **The roster is keyed by the
     session id** (the roster's `match_id` is the session id), so
     `rank_game` finds it from `NewGame.session_id`, the same key
     arbitration uses. `POST /games/bots` already requires the host's
     session.
3. **Agreed: ranking only reads the acceptance result.** The check runs
   when a game record is taken in (§9.3): each `bot: true` entry must be
   in that session's `BotsRostered`, and a rostered xuid cannot be claimed
   as a human. A record that fails is refused whole, so `rank_game` only
   ever sees accepted bot entries, and reads `rostered` for `persistent`
   and `level`. Host-made xuids (`live_proto::bots::is_local_xuid`) are
   refused in any record that reaches the server.

Also agreed: `rank::bot_skill(level)` as the roster's `skill`, and
`Ranks::nearest(playlist, level, n, bots)` (the candidates are
`Bots::persistent`) for choosing persistent bots, which
the roster endpoint calls; gaps are made-up bots shown at the humans'
mean level.

### 9.5 Bot accounts

"Bots should not be guests but fake Xbox Live users marked as bots": every
bot the server rosters is a **bot account**, shown like an account with a
BOT mark, for matchmaking and for Live custom games.

**What a bot account is** (server, both homes, shared router and read
model, `live_server::bots`):

- An xuid in the server's bot range `0x000B…` (humans `0x0009`, host-made
  `0x000E`), issued once by the server as `BotIssued` on `bot-<xuid>`
  (§10 stage 6c) with a gamertag from the word lists ("Rusty Lugnut":
  `FIRST` × `SECOND`, a number past a few clashes), emblem, colours and
  personality. Persistent bots (the operator's pool, ranked) hold their
  gamertag as an account does; made-up ones (one roster's) do not.
- **Read as an account**: `GET /v1/accounts/{gamertag}` answers for a bot
  with `bot: true` (`AccountSummary::bot`), `online: false`, its
  `created_at` (the `BotIssued` time) and its recent games. A human
  account of the same name comes first (a made-up bot does not hold its
  name). The accounts list (`GET /v1/accounts`) never has bots, and the
  web keeps them out of the Players list and the account count.
- **Never a person**: it has no keys (`/keys` 404s), so it cannot sign in
  or post anything; recovery is by an account's gamertag and a bot is not
  one; nobody else may write `bot-*` or `bots-*`. The HLE sends no
  presence for it.
- **Leases.** A roster (`POST /v1/games/bots`, `BotsRostered` on
  `bots-<session>`) is the lease: its bots are the host's for that session.
  The server accepts a game record's `bot: true` entry with an XUID
  only from the session host and for a bot on the roster in force at
  the start (§9.3). It rejects both a bot recorded as a human and a
  human recorded as a bot.

**Where the bots come from.** The driver asks Live for a roster covering
every requested bot. A matchmade session includes its playlist so the
server can select persistent bots near the humans' level (§9.4). A
custom game requests made-up bots at the page's difficulty. Changes in
count, difficulty or session can fetch a new roster. System Link and
Live sessions without a roster use host-made phantom identities.

**The session's players.** Retail Halo 2 fills no slot counts in its
advertisement (`XOnlineMatchSessionCreate`/`Update`, `0x8fcc0`/`0x8fe20`,
slots at ad `+0x3c..+0x48`, filled by a virtual call): run 20261119-005001
showed 0 filled with six players in the game. So the host says who plays.
Every second (`clankers::frame`) while a roster is in force, the driver
hands the HLE the rostered phantoms that have joined the game
(`Driver::session_bots`). `Hle::set_session_players` tells the session the roster is for
(`Driver::roster_in_force`) of them, and each session the console hosts of
its signed-in users, as `PlayerJoined` and `PlayerLeft` on `session-<id>`
for only what changed (`sessions::session_players`, `players_diff`; zero
and host-made xuids never). Only that session: a Clankermatch host
advertises its party's lobby session (made before the search) beside the
match the search hosted, and the user's run (cm-run2) told both, so every
`PlayerJoined` and `PlayerLeft` on the party's (`84ad...`) came back 400
(`unknown player`: the bots are leased to the match alone) while the
match's were taken. One more call after the roster ends sends the
bots' leaves. The server takes a `PlayerJoined` for a human account, or
for a bot of the session's latest roster when its host posts it
(`bots::leased_to`), and a `PlayerLeft` for anyone in the session, so a
replaced roster's bots can leave but not join again. The worker shares
the code (`live-worker` `rostered_bots_read_as_bot_accounts`).

**One identity across a session handoff.** Halo 2 hands its lobby to a
new Live session as a game starts (and again as it ends): the HLE deletes
the lobby's session and advertises another (run 20261119-020015:
`8894d40b...` deleted, `83d9415d...` advertised a few seconds later).
The phantoms keep their identities through the handoff.

- **Server** (`live_server::bots::may_carry`, both homes: the worker runs
  the same code). A roster request may name `carry_from`, the host's
  previous session. The server rosters that session's latest bots again,
  in order and with their levels and skills, for the new session (a
  `BotsRostered` on `bots-<new>`), so they are leased to it: the host may
  report them as the new session's players and its game records may name
  them. "The same host's next session" means: `carry_from` was this
  author's session, it is no longer open, it ended (its last activity, the
  delete) no more than `CARRY_WINDOW_S` (120 s) before the request, its
  latest roster is this author's, and it is not the new session itself.
  A count above the old roster's is topped up with new bots after the
  carried ones; a smaller one takes the first. Anything else is a 400.
- **Console** (`clankers::ask_for`). Between the delete and the next
  advertisement (no hosted session), the roster in force stays and the
  bots do not change. When the hosted session changes and the same bots
  are wanted (count, difficulty and playlist unchanged), the driver asks
  with `carry_from` and keeps the old roster meanwhile: nothing is taken
  off, no bot falls back to host-made, the phantoms stay joined. The log
  says `clankers: the game moved from session A to B: asking Live to
  carry its bots there`. Until the server answers, the roster is not the
  new session's: no game record names it and the session is told of no
  bots (`roster_in_force` is `None`). If the server refuses the carry,
  the driver asks for a new roster, as it did before. A changed count,
  difficulty or playlist asks for a new roster, not a carry.
- **The record.** Telemetry asks for the bots once, at the game's start.
  A phantom made later (by a roster change mid-game) was then a machine
  the start's list did not name, so it went out as a human: run -020015's
  "6 players, 3 bots". A phantom's machine id now marks it as a bot
  whether listed or not (`live_proto::bots::is_phantom_machine`, the
  `C1 A4` prefix); only a listed one, with its roster in force, carries
  its xuid.

**Web.** A bot account has a player page: the identity strip with a BOT
mark, its service record and game history, no friend, message or other
social actions, never online. The page finds it in the event log
(`BotIssued`) or asks the server by gamertag. Scoreboards and carnage
reports link a bot with an xuid to it.

**Careers.** Humans' careers stay as §6 says: a kill or death with a bot
on either side is game-only. A bot account's own service record (its
page) counts all its kills and deaths. The server's stats summary has no
row for any bot, so no bot is on a leaderboard; persistent bots' ranks are
sbmm-ranked's.

**Staged (not built):**

1. Clankermatch's `MatchProposed` carrying the roster (stage 8, with
   sbmm-ranked): the matchmaking path itself. The playlist roster above
   is what it will send.
2. Bot presence ("Playing Slayer on Lockout" while its host's game runs):
   would need the server to derive it from the host's presence and the
   roster, not a bot's own signed event.
3. Made-up bot accounts pile up: one per bot per roster. A sweep (or
   reusing a session's made-up bots) when that matters; their pages stay.
4. Bots in the web's search lists and finder: kept for now (bots have
   pages), filter them there if the user prefers.

**Tests.** Coverage lives in `live-server`'s `bots` integration tests,
`live-hle`'s `players_tests`, `live-worker`'s
`rostered_bots_read_as_bot_accounts`, `boot`'s `clankers` tests and the
web's `test/bots.test.ts`.

**Game run.** Sign into Live and host a custom game with Clankers On
and Count 3. The lobby should show the three rostered gamertags.
Sign in a second human on a free controller, start Slayer and check
the game page for three BOT marks linking to the bot accounts. A
System Link game uses host-made Clanker names. The Live custom game
can also be checked with `h2 bots test` (§13).
