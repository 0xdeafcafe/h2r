# Rules start gate: a map must satisfy the script's labels

Status: Design, with the check (`rules::labels::check`), both census builders and the host's gate built (slice 3); the lobby's and Live's are not.
Code: `crates/extensions/game/src/rules/` (the host runtime), `crates/extensions/game/src/lobby_mode.rs` (the lobby's start state), `crates/live/live-server/src/admin.rs` (the hopper builder). Tests: `crates/formats/forge-doc/tests/census.rs`, `crates/extensions/game/src/rules/runtime/tests/gate.rs`; the other slices in [rules-language-types.md](rules-language-types.md#implementation-plan) name theirs.
Switch: none.

A rules script declares the Forge labels its map must hold
([rules-language-types.md](rules-language-types.md#labels)). A map that
lacks or breaks a requirement blocks the game: it does not start, and it
does not fall back to the carrier engine's rules. The lobby says why
before Start Game, the host decides at the game's start, and Live keeps
blocked pairs out of its hoppers. The rule that must hold: no script
runs on a map the check has not passed on the host. Every other refusal
of a script ([rules-engine.md](rules-engine.md#the-host-runtime)) keeps
that page's rule, the carrier's own rules.

Everything not marked **Code** is **Design**
([docs-style.md](../meta/docs-style.md)).

## The check

**Code**, `crates/formats/rules/src/labels.rs`.
`rules::labels::check(ir, census, teams)` (pure)
compares the IR's label table and tag table with a census of the map
(`labels::Census`: for each label text, its members' class, number and
team; and the object type of each tag the map holds) for the teams in
play. It returns every problem (`labels::Problem`: a `ProblemKind` and
its text), labels in table order, then tags:

| Case | Text |
|---|---|
| none of a required label | `the map has no zone labelled "goal"; Siege needs exactly 1` |
| too many or too few | `the map has 3 objects labelled "core"; Siege needs exactly 1` |
| wrong class | `"goal" on this map is an object; Siege needs a zone (a trigger volume or a flag)` |
| a missing number | `no zone labelled "zone" has the number 1; Siege needs 0 and 1` |
| a number outside the range | `a zone labelled "zone" has the number 3; Siege needs only 0 and 1` |
| an unnumbered member | `a zone labelled "hill_corner" has no number; King of the Hill needs every one numbered` |
| a team without one | `team 1 has no zone labelled "ctf_flag_spawn"` |
| a tag of the wrong group | `"<path>" on this map is a vehicle; Siege gives it as a weapon` |

A member whose class does not satisfy the label is reported once per
label and is not counted; a label with no member of its class reports
the class alone. A stated `max` blocks a map over it; a label of `max`
256 states none. `labels::minimal(ir, teams)` is the smallest census
the check accepts.

**Code.** A census has two builders with the one result type, in
`crates/formats/forge-doc/src/labels.rs`, on the trait `CensusOf`
(the `Census` type is the `rules` crate's): `Census::of_file` from a map
file's netgame flag and starting-location blocks
(`crates/formats/map/src/scenario_layout.rs`, read by `rows_of_file`),
its tag paths and the lobby's Forge document, and `Census::of_game` from
the rows the game reads of the loaded scenario in guest memory
(`game::rules::census::rows`) and the session's document. Both go
through `forge_doc::labelled`: a flag takes its own label from the
document, else its type's built-in one; a starting location or an object
placement is labelled only by the document; the document's placement ops
are replayed over the flag and starting-location rows. A member's team
is its placement's team field as a designator, 0 to 7; neutral and none
are no team. The game holds no tag paths, so the host's census has no
tags; no entry a host makes takes a tag. A test builds both from one
fixture and asserts they are equal.

## Where it runs

| Where | Function or hook | Census | On a problem |
|---|---|---|---|
| The lobby, when the map, the Forge map or the variant changes | `rules::gate::refresh` (`crates/extensions/game/src/rules/gate.rs`, new), called from the `party-lobby` feature's `before` on `LOBBY_VARIANT_PANEL` (`lobby_mode.rs`). Its key is the lobby's map (`h1_options_row::LOBBY_MAP`), `lobby_start::forge_map()` and the script prefix of the variant's block: the record's own, or the store bundle's that `game_variants::inject::apply_block` writes at activation. A changed key runs the check once and keeps the verdict | `of_file` | the verdict is `Blocked(problems)` |
| The lobby, at Start | the `after` on `LOBBY_START_STATE` in `lobby_mode.rs`: `lobby_mode::start_state` answers `REFUSED_STATE` in place of ready when `lobby_mode::start_refused` reads `gate::blocked()`, the path a Forge lobby with a remote member takes. Start Game acts only on ready, so no countdown runs | the kept verdict | Start Game does nothing |
| The host, at the game's start: the final authority | `Runtime::start` (`rules/runtime.rs`), from the `rules` feature's `on_start` observer after a successful `GAME_ACTIVATE` (`rules/feature.rs`), after `verify::load` and `verify::unhostable` and before `runtime::host_refusal` | `of_game` | `Status::Blocked`, and the game is ended (below) |
| Live, when a hopper is edited | `admin::check_pairs`, called by `admin::put_hopper` after `admin::apply_patch` (`crates/live/live-server/src/admin.rs`) | the map's census on Live | the edit is refused |
| The workbench | `workbench::run` (rules-language-types.md, "The workbench") | the scenario's map | problems of kind `map`, and no run |

The lobby's check is a courtesy to the player; only the host's decides.
It runs on the machine that starts the game, and a guest runs neither
check: a guest runs no script. Joining a lobby or a game is not gated by
labels. The gate for a guest whose build lacks a capability is the one
rules-engine.md's "Versioning" leaves open.

## What a blocked start shows

**The lobby shows the reason** on the custom lobby's status line:
`lobby_status::custom_game_text` answers the gate's text while
`gate::blocked()` holds, after the mode strip's notice and before the
Forge refusal. The text is the first problem, with ` (and 2 more)` when
there are more, written through `ui_kit::screen::SlotTexts` as the Forge
map's help line is. `rules.state` (the agent API) lists every problem,
in the lobby and in a game, as `blocked`. A guest's status line shows
retail's text.

**When the host blocks a start (Code).** The start gets past the lobby when no
lobby ran (a level boot, the light host's `Start` in `host_lobby.rs`, a
matchmade game), when the Forge document changed after the lobby's
check, or when the file's census and the game's differ. `Runtime::start`
then keeps `Status::Blocked { hash, problems }`, logs
`rules: blocked, the game does not start: <problems>`, and queues one
`Call::End`. The tick hook's first `pre_tick` makes it
(`firefight::guest::end_game`, the call `end_round` uses), so the game
ends in its first tick and the players return where a game's end returns
them: the lobby they started from. No VM is made and no handler runs.
`gate` keeps the problems as the lobby's verdict, so the status line
names them until the map or the variant changes. A Live host puts no
script hash on its session (`Hle::set_session_rules(None)`).

**A Forge session is not blocked (Code).** In the lobby's Forge mode
(`lobby_start::Mode::Forge`) the map is what is being built, so the gate
does not hold Start Game or end the game. The script is refused with the
same problems, which `rules.state` lists, and no script runs until the
map satisfies it. The check runs at the game's start, when the session
has no document yet, and is not run again as the map is edited: the
script stays refused for that game.

## Hoppers, System Link, offline and Theater

**Live hoppers.** A hopper entry is a map id and a variant record
(`live_proto::hopper::Record`, `hopper::variant::VariantRecord`).
`admin::check_pairs` takes each entry of each hopper the patch names
whose record's block names a script, finds the blob by its prefix (a
built-in, else a saved script, `Live::saved_rules`), and runs
`labels::check` with the map's census and the teams the record's team
setting gives. A problem refuses the whole edit with `invalid`, naming
the hopper, the entry and the problem's text, so the published file
never holds a blocked pair and matchmaking never picks one. An entry
whose script or whose map's census Live does not have is refused the
same way, by name. Live holds no map bytes: a census reaches it as
`PUT /v1/maps/{map}/census` (admin, a server-signed `MapCensusSet`
event, in both homes), which `h2 live maps census <map>` writes from the
admin's own copy of the map, as `h2 live maps analyze` writes a
package's contents report. A new census for a map re-checks the
published hoppers and its answer lists each entry it blocks. The host's
check still runs on a matchmade game. The context-policy lane checks a
hopper entry's script in the same place; `check_pairs` is the second
check of the pair there.

**System Link and offline.** Both checks need only this machine: the
script is a built-in or the store's (a System Link game takes no other),
and the census is the local map's. Split screen and a System Link host
run the lobby check and the host check exactly as a Live host does.

**Theater never runs the check.** Playback replays a game that passed
it on its host; the blob, the options and the seed are replay inputs.
The `rules` feature is installed with `film`, as Firefight's is
(`firefight::feature::install`), and `Runtime::start` skips the check
when it is set.

## Rules and limits

- The gate blocks on label and tag-group problems only.
- The host answers the label reads (**Code**, `rules/census.rs`):
  `objects labelled`, `o.number`, `zone.label`, `zone.id` and `o.team`.
  `runtime::host_refusal` refuses a script that calls `inside` or
  `ticks_held` or answers `on enter` or `on leave`, since no zone has a
  volume a host can test; such a script that passes the gate plays the
  carrier's rules.
- The teams `per team` reads on the host are the roster's teams when the
  variant is a team game, and none otherwise.
- The lobby's verdict is this machine's. A guest's status line shows
  retail's text, and a guest learns of a host block only by the game's
  end.

## Not verified

- Whether a netgame flag's team field is the designator the per-team
  check should compare with the game's teams: the census takes the field
  as the team, and the game maps designators to teams at `E+0x10`.
- Whether the roster holds every player's team when `GAME_ACTIVATE`
  returns, for `per team`.

- That a census from a map file equals the loaded game's on every map:
  whether the game adds or drops netgame flags at load.
- What the postgame screen, the Live game report and the film recorder
  do with a game the host's gate ends in its first tick, and whether
  ending it there returns a matchmade party to its lobby.
- Whether a `before` on `GAME_ACTIVATE` that answers failure returns the
  session to the lobby cleanly. If it does, the host's check belongs
  there, with the file census, and no map loads.
- How many characters the lobby's status line shows.
- The teams a hopper entry's game has, from its record alone, for
  `per team` in `check_pairs`.

## Related

- [rules-language-types.md](rules-language-types.md): label declarations, the IR's label table, and the slices that build the gate (3, 7 and 9).
- [rules-engine.md](rules-engine.md): the host runtime and its other refusals.
- [forge-firefight-ui.md](forge-firefight-ui.md): the lobby's modes and its start state.
- [live-matchmaking.md](../live/live-matchmaking.md): the hopper file and its builder.
