# Rules start gate: a map must satisfy the script's labels

Status: Design. Nothing here is built.
Code: `crates/extensions/game/src/rules/` (the host runtime), `crates/extensions/game/src/lobby_mode.rs` (the lobby's start state), `crates/live/live-server/src/admin.rs` (the hopper builder). Tests: none yet; the slices in [rules-language-types.md](rules-language-types.md#implementation-plan) name them.
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

`rules::labels::check(ir, census, teams)` (`labels.rs`, new, pure)
compares the IR's label table and tag table with a census of the map
(`forge_doc::Census`, new: for each label text, its members' class,
number and team; and the group of each tag the map holds) for the teams
in play. It returns every problem, each with its text:

| Case | Text |
|---|---|
| none of a required label | `the map has no zone labelled "goal"; Siege needs exactly 1` |
| too many or too few | `the map has 3 objects labelled "core"; Siege needs exactly 1` |
| wrong class | `"goal" on this map is a crate; Siege needs a zone (a trigger volume or a flag)` |
| a missing number | `no zone labelled "zone" has the number 1; Siege needs 0 and 1` |
| an unnumbered member | `a zone labelled "hill_corner" has no number; King of the Hill needs every one numbered` |
| a team without one | `team 1 has no zone labelled "ctf_flag_spawn"` |
| a tag of the wrong group | `"<path>" on this map is a vehicle; Siege gives it as a weapon` |

A census has two builders with one result type, in
`crates/formats/forge-doc/src/labels.rs`: `Census::of_file` from a map
file's netgame flag and starting-location blocks
(`crates/formats/map/src/scenario_layout.rs`) and the lobby's Forge
document (`ForgeDoc::labels`), and `Census::of_game` from the loaded
scenario in guest memory and the session's document. A test builds both
from one fixture and asserts they are equal.

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

**When the host blocks a start.** The start gets past the lobby when no
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

**A Forge session is not blocked.** In the lobby's Forge mode
(`lobby_start::Mode::Forge`) the map is what is being built, so the gate
does not hold Start Game or end the game. The script is refused with the
same problems, which `rules.state` lists, and no script runs until the
map satisfies it.

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
- `runtime::host_refusal` (**Code**, `rules/runtime.rs`) refuses a
  script that reads a Forge label, since the runtime's label reads are
  not built. A script with labels that passes the gate is refused there
  and plays the carrier's rules until they are.
- The lobby's verdict is this machine's. A guest's status line shows
  retail's text, and a guest learns of a host block only by the game's
  end.

## Not verified

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
