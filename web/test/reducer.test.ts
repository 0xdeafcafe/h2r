import { beforeEach, describe, expect, test } from "bun:test";
import { applyEvent, applyEvents, endedGames, initialState, liveGames, playerStats, standings, teamStandings, TAIL_LIMIT } from "../src/state/reducer";
import { ev, resetPos } from "./helpers";

const A = "0009000000000001", B = "0009000000000002";

function seeded() {
  const s = initialState();
  applyEvents(s, [
    ev("AccountCreated", `account-${A}`, { xuid: A, gamertag: "Mochi" }),
    ev("AccountCreated", `account-${B}`, { xuid: B, gamertag: "Noodle" }),
    ev("SignedIn", `presence-${A}`, { xuid: A, state: "online" }, "2026-11-15T20:01:00.000Z"),
    ev("GameStarted", "game-g1", {
      game_id: "g1", map: "teacup", mode: "multiplayer", variant: "Rumble", team_play: false, score_limit: 10,
      players: [
        { index: 0, name: "Mochi", xuid: A, team: 0 },
        { index: 1, name: "Noodle", xuid: B, team: 1 },
        { index: 2, name: "Guest", team: 2 },
      ],
    }),
    ev("PresenceChanged", `presence-${A}`, { xuid: A, state: "playing", game_id: "g1" }, "2026-11-15T20:02:00.000Z"),
  ]);
  return s;
}

beforeEach(resetPos);

describe("reducer", () => {
  test("accounts and presence", () => {
    const s = seeded();
    expect(Object.keys(s.accounts)).toHaveLength(2);
    const a = s.accounts[A]!;
    expect(a.online).toBe(true);
    expect(a.presence?.game_id).toBe("g1");
    // since stays at sign-in time across PresenceChanged
    expect(a.presence?.since).toBe("2026-11-15T20:01:00.000Z");
    applyEvent(s, ev("PresenceExpired", `presence-${A}`, { xuid: A }));
    expect(s.accounts[A]!.online).toBe(false);
    expect(s.accounts[A]!.presence).toBeNull();
    expect(s.accounts[B]!.online).toBe(false);
  });

  test("presence for an unseen account creates a placeholder", () => {
    const s = initialState();
    applyEvent(s, ev("SignedIn", "presence-x", { xuid: "0009ffffffffffff", state: "online" }));
    expect(s.accounts["0009ffffffffffff"]!.online).toBe(true);
  });

  test("ignores events at or below the current position (resume duplicates)", () => {
    const s = seeded();
    const before = s.eventCount;
    const dup = { ...s.tail[s.tail.length - 1]! };
    expect(applyEvent(s, dup)).toBe(false);
    expect(s.eventCount).toBe(before);
  });

  test("kills, scores, and game end", () => {
    const s = seeded();
    applyEvents(s, [
      ev("PlayerKilled", "game-g1", { tick: 300, victim: 1, killer: 0, weapon: "rifle", kind: "kill" }),
      ev("ScoreChanged", "game-g1", { tick: 300, players: [{ index: 0, score: 1, kills: 1, deaths: 0, assists: 0 }, { index: 1, score: 0, kills: 0, deaths: 1, assists: 0 }] }),
      ev("PlayerKilled", "game-g1", { tick: 400, victim: 0, kind: "suicide" }),
    ]);
    const g = s.games.g1!;
    expect(g.kills).toHaveLength(2);
    expect(g.kills[1]!.killer).toBeNull();
    expect(standings(g)[0]!.name).toBe("Mochi");
    expect(liveGames(s)).toHaveLength(1);
    applyEvent(s, ev("GameEnded", "game-g1", { tick: 900, reason: "finished", winner: 0, players: [{ index: 0, score: 10, kills: 10, deaths: 1, assists: 0 }] }, "2026-11-15T20:10:00.000Z"));
    expect(g.status).toBe("ended");
    expect(g.ended_at).toBe("2026-11-15T20:10:00.000Z");
    expect(g.scores[0]!.score).toBe(10);
    expect(g.last_tick).toBe(900);
    expect(liveGames(s)).toHaveLength(0);
    expect(endedGames(s)).toHaveLength(1);
    expect(g.positions.length).toBe(5);
  });

  test("join and leave", () => {
    const s = seeded();
    applyEvent(s, ev("PlayerJoined", "game-g1", { player: { index: 3, name: "Late", team: 3 } }));
    applyEvent(s, ev("PlayerLeft", "game-g1", { index: 2 }));
    const g = s.games.g1!;
    expect(g.players[3]!.name).toBe("Late");
    expect(g.scores[3]!.score).toBe(0);
    expect(g.players[2]!.left).toBe(true);
  });

  test("team standings", () => {
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-t", { game_id: "t", map: "orchard", mode: "multiplayer", team_play: true, players: [
        { index: 0, name: "a", team: 0 }, { index: 1, name: "b", team: 1 }, { index: 2, name: "c", team: 0 },
      ] }),
      ev("ScoreChanged", "game-t", { tick: 1, players: [{ index: 1, score: 3, kills: 3, deaths: 0, assists: 0 }], teams: [{ team: 0, score: 1 }, { team: 1, score: 3 }] }),
    ]);
    const ts = teamStandings(s.games.t!);
    expect(ts.map((t) => t.team)).toEqual([1, 0]);
    expect(ts[1]!.players.map((p) => p.name).sort()).toEqual(["a", "c"]);
  });

  test("player stats from events", () => {
    const s = seeded();
    applyEvents(s, [
      ev("PlayerKilled", "game-g1", { tick: 1, victim: 1, killer: 0, kind: "kill" }),
      ev("PlayerKilled", "game-g1", { tick: 2, victim: 2, killer: 0, kind: "kill" }),
      ev("PlayerKilled", "game-g1", { tick: 3, victim: 0, killer: 1, kind: "kill" }),
      ev("PlayerKilled", "game-g1", { tick: 4, victim: 0, kind: "suicide" }),
      ev("GameEnded", "game-g1", { tick: 5, reason: "finished", winner: 0 }),
    ]);
    expect(playerStats(s, A)).toEqual({ games: 1, kills: 2, deaths: 2, wins: 1, live: 0 });
    expect(playerStats(s, B)).toEqual({ games: 1, kills: 1, deaths: 1, wins: 0, live: 0 });
  });

  test("tail is capped and counts types", () => {
    const s = initialState();
    for (let i = 0; i < TAIL_LIMIT + 20; i++) applyEvent(s, ev("SignedIn", "presence-x", { xuid: A }));
    expect(s.tail).toHaveLength(TAIL_LIMIT);
    expect(s.typeCounts.SignedIn).toBe(TAIL_LIMIT + 20);
    expect(s.tail[s.tail.length - 1]!.position).toBe(s.position);
  });
});
