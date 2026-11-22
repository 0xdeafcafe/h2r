// The Incel medal (docs/game/teabag.md, "The medal"): its name and art, the
// teabags and late joiners the reducer keeps from a game's stream and its
// snapshot, and the server's award on the game. Live works the medal out;
// the site only shows it.
import { describe, expect, test } from "bun:test";
import { applyEvents, applyGameSnapshot, initialState } from "../src/state/reducer";
import { MEDAL_BY_ID, gameMedalCounts } from "../src/state/derive";
import { PACK_MEDALS } from "../src/lib/art";
import { medalKey } from "../src/lib/gameart";
import { STATS_EVENT_KINDS, isStatsEvent } from "../src/stats/eventKinds";
import { ev, resetPos } from "./helpers";

const A = "0009000000000001", B = "0009000000000002";

function world(reason = "finished") {
  resetPos();
  const s = initialState();
  applyEvents(s, [
    ev("GameStarted", "game-g", { game_id: "g", map: "lockout", mode: "multiplayer", team_play: false, players: [
      { index: 0, name: "a", xuid: A, team: 0 }, { index: 1, name: "b", xuid: B, team: 1 },
      { index: 2, name: "Clanker 2", team: 1, bot: true }, { index: 3, name: "c", team: 0 },
    ] }, "2026-11-15T10:00:00.000Z"),
    // Before the first kill: on time.
    ev("PlayerJoined", "game-g", { player: { index: 4, name: "d", team: 0 } }),
    ev("PlayerKilled", "game-g", { tick: 30, victim: 1, killer: 0, kind: "kill", weapon: "br" }),
    // After it: late.
    ev("PlayerJoined", "game-g", { player: { index: 5, name: "e", team: 1 } }),
    ev("Teabag", "game-g", { tick: 40, player: 0, victim: 1 }),
    ev("PlayerLeft", "game-g", { index: 3 }),
    ev("GameEnded", "game-g", { tick: 900, reason, winner: { player: 0 } }, "2026-11-15T10:05:00.000Z"),
  ]);
  return s;
}

describe("the incel medal", () => {
  test("is named and described", () => {
    expect(MEDAL_BY_ID.incel).toEqual({
      id: "incel", name: "Incel", blurb: "Completed a game without teabagging or getting teabagged", tier: 1,
    });
  });

  test("has pack art and no game-art counterpart", () => {
    expect(PACK_MEDALS.has("incel")).toBe(true);
    expect(medalKey("incel")).toBeNull();
  });

  test("the stream's teabags and late joiners land on the game", () => {
    const g = world().games.g!;
    expect(g.teabags.map((t) => [t.tick, t.player, t.victim])).toEqual([[40, 0, 1]]);
    for (const t of g.teabags) expect(g.positions).toContain(t.position);
    expect([0, 1, 2, 3, 4, 5].map((i) => g.players[i]!.late === true)).toEqual([false, false, false, false, false, true]);
  });

  test("the award is the server's: a game read carries it", () => {
    const s = world();
    applyGameSnapshot(s, {
      game_id: "g", map: "lockout", mode: "multiplayer", started_at: "2026-11-15T10:00:00.000Z", players: [],
      medals: [{ player: 2, name: "Clanker 2", medals: { incel: 1 } }, { player: 4, name: "d", medals: { incel: 1 } }],
    });
    const g = s.games.g!;
    expect(Object.fromEntries(gameMedalCounts(g))).toEqual({ incel: 2 });
    for (const slot of [0, 1, 3, 5]) expect(gameMedalCounts(g, [slot]).get("incel")).toBeUndefined();
  });

  test("a snapshot carries the teabags and the late joiners", () => {
    resetPos();
    const s = initialState();
    applyGameSnapshot(s, {
      game_id: "k", map: "lockout", mode: "multiplayer", started_at: "2026-11-15T10:00:00.000Z",
      ended_at: "2026-11-15T10:05:00.000Z", end_reason: "finished", last_tick: 600,
      players: [
        { index: 0, name: "a", xuid: A, team: 0 }, { index: 1, name: "b", xuid: B, team: 1 },
        { index: 2, name: "c", team: 0 }, { index: 3, name: "d", team: 1 },
      ],
      teabags: [{ tick: 50, player: 1, victim: 2, position: 7, at: "" }],
      joined_late: [3],
    } as Parameters<typeof applyGameSnapshot>[1]);
    const g = s.games.k!;
    expect(g.teabags).toEqual([{ position: 7, at: "", tick: 50, player: 1, victim: 2 }]);
    expect(g.players[3]!.late).toBe(true);
    expect(g.players[0]!.late).toBe(false);
  });

  test("teabags are stats events of game streams only", () => {
    expect(STATS_EVENT_KINDS.Teabag).toEqual({ stream: "game-", category: "kill" });
    expect(isStatsEvent(ev("Teabag", "game-g", {}))).toBe(true);
    expect(isStatsEvent(ev("Teabag", "session-g", {}))).toBe(false);
  });
});
