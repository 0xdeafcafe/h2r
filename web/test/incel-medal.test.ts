// The Incel medal (docs/game/teabag.md, "The medal"): the copy of the rule
// in derive.ts, from a game's stream and from its snapshot, and the
// teabags the reducer keeps.
import { describe, expect, test } from "bun:test";
import { applyEvents, applyGameSnapshot, initialState } from "../src/state/reducer";
import { MEDAL_BY_ID, countMedals, gameAwards, gameIncel } from "../src/state/derive";
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

  test("goes at the end to who completed the game on neither side of a teabag, a bot included", () => {
    const g = world().games.g!;
    expect(g.teabags.map((t) => [t.tick, t.player, t.victim])).toEqual([[40, 0, 1]]);
    for (const t of g.teabags) expect(g.positions).toContain(t.position);
    expect(gameIncel(g)).toEqual([{ medal: "incel", slot: 2, tick: 900 }, { medal: "incel", slot: 4, tick: 900 }]);
    const awards = gameAwards(g);
    for (const slot of [0, 1, 3, 5]) expect(countMedals(awards, new Set([slot])).get("incel")).toBeUndefined();
  });

  test("is not given in a game that did not finish, or before the end", () => {
    expect(gameIncel(world("quit").games.g!)).toEqual([]);
    expect(gameIncel(world("lost").games.g!)).toEqual([]);
    resetPos();
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-h", { game_id: "h", map: "lockout", mode: "multiplayer", team_play: false, players: [
        { index: 0, name: "a", xuid: A, team: 0 },
      ] }),
    ]);
    expect(gameIncel(s.games.h!)).toEqual([]);
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
    expect(gameIncel(g).map((a) => a.slot)).toEqual([0]);
  });

  test("teabags are stats events of game streams only", () => {
    expect(STATS_EVENT_KINDS.Teabag).toEqual({ stream: "game-", category: "kill" });
    expect(isStatsEvent(ev("Teabag", "game-g", {}))).toBe(true);
    expect(isStatsEvent(ev("Teabag", "session-g", {}))).toBe(false);
  });
});
