import { describe, expect, test } from "bun:test";
import type { Kill } from "../src/state/reducer";
import { applyEvents, applyGameSnapshot, initialState } from "../src/state/reducer";
import {
  BOUNCE_MEDALS, MEDALS, MEDAL_BY_ID, bestSprees, gameHistory, gameMedalCounts, medalDef, medalName, nextRankAt, parseAccountMedals, places,
  playedWith, rankFor, rankIndex, serviceRecord, topMedalCounts, gameWeaponReport, weaponBreakdown, weaponsAgainst, withCareerMedals,
} from "../src/state/derive";
import { damageSourceName } from "../src/lib/format";
import { ev, resetPos } from "./helpers";

let pos = 0;
const k = (tick: number, killer: number | null, victim: number, weapon: string | null = "rifle", kind: Kill["kind"] = "kill"): Kill =>
  ({ position: ++pos, at: "", tick, killer, victim, weapon, kind, pos: null });

describe("medals", () => {
  test("a spree is the longest run of kills without dying", () => {
    const kills: Kill[] = [];
    for (let i = 0; i < 15; i++) kills.push(k(i * 1000, 0, 1 + (i % 3)));
    expect(bestSprees(kills).get(0)).toBe(15);
    // A suicide resets it.
    const broken = [...kills.slice(0, 4), k(4500, null, 0, null, "suicide"), ...kills.slice(4)];
    expect(bestSprees(broken).get(0)).toBe(10);
  });

  test("the site lists Halo 2's own medals in the game's order, then our modes'", () => {
    expect(MEDALS.map((m) => m.id)).toEqual([
      "double-kill", "triple-kill", "killtacular", "kill-frenzy", "killtrocity", "killimanjaro",
      "sniper-kill", "roadkill", "bonecracker", "stealth-kill", "vehicle-kill", "carjack", "stick-it",
      "killing-spree", "running-riot", "rampage", "berserker", "overkill",
      "flag-taken", "flag-save", "flag-returned", "bomb-planted", "bomb-save", "bomb-diffused",
      ...BOUNCE_MEDALS, "incel",
    ]);
    expect(MEDAL_BY_ID["double-kill"].name).toBe("Double Kill");
    expect(MEDAL_BY_ID["bomb-diffused"].name).toBe("Bomb Diffused");
    for (const gone of ["twofer", "hat-trick", "full-house", "encore", "hot-streak", "wildfire", "untouchable", "opening-act", "last-word", "payback"]) {
      expect(medalDef(gone)).toBeUndefined();
    }
  });

  test("an id the site doesn't know is named by its id", () => {
    expect(medalName("killing-spree")).toBe("Killing Spree");
    expect(medalName("new-medal")).toBe("new-medal");
    expect(medalDef("constructor")).toBeUndefined();
    expect(medalName("constructor")).toBe("constructor");
  });

  test("a game's medals sum by slot, and the top ones sort most first", () => {
    const g = { medals: { 0: { "double-kill": 2, "superbounce": 1 }, 1: { "double-kill": 1, "incel": 1 }, 2: { "new-medal": 3 } } };
    expect(Object.fromEntries(gameMedalCounts(g, [0, 1]))).toEqual({ "double-kill": 3, "superbounce": 1, "incel": 1 });
    expect(Object.fromEntries(gameMedalCounts(g))).toEqual({ "double-kill": 3, "superbounce": 1, "incel": 1, "new-medal": 3 });
    expect(gameMedalCounts(g, [5]).size).toBe(0);
    // Ties go in the site's order, unknown ids after.
    expect(topMedalCounts(gameMedalCounts(g))).toEqual([["double-kill", 3], ["new-medal", 3], ["superbounce", 1], ["incel", 1]]);
    expect(topMedalCounts({ "rampage": 1, "overkill": 0 }, 1)).toEqual([["rampage", 1]]);
  });

  test("an account's medals read defensively, and replace a record's", () => {
    const m = parseAccountMedals({ xuid: "x", medals: { "double-kill": 4, "incel": 0, "bad": "x", "neg": -1 }, position: 9 });
    expect(Object.fromEntries(m)).toEqual({ "double-kill": 4 });
    expect(parseAccountMedals(null).size).toBe(0);
    const rec = { games: 1, medals: new Map([["rampage", 1]]) };
    expect(withCareerMedals(rec, null)).toBe(rec);
    expect(withCareerMedals(rec, m).medals).toBe(m);
  });
});

describe("weapons", () => {
  test("game report has no weapon bars when all scoring kills lack weapon data", () => {
    expect(gameWeaponReport([])).toEqual([]);
    expect(gameWeaponReport([k(0, 0, 1, null), k(1, 1, 0, "unknown"), k(2, 0, 1, " ")]))
      .toEqual([]);
  });

  test("game report keeps the missing share in the totals and in the abbreviated view", () => {
    const kills = [k(0, 0, 1, "rifle"), k(1, 0, 2, "sword"), k(2, 1, 0, null), k(3, 2, 0, null)];
    expect(gameWeaponReport(kills, 2)).toEqual([
      { weapon: "rifle", kills: 1, share: 0.25 },
      { weapon: null, kills: 2, share: 0.5 },
    ]);
  });

  test("snapshot and stream carry the killing source into the carnage report", () => {
    resetPos();
    const s = initialState();
    const shot = { tick: 10, victim: 1, killer: 0, kind: "kill" as const, weapon: "smg", headshot: false };
    const event = ev("PlayerKilled", "game-g", shot);
    applyGameSnapshot(s, {
      game_id: "g", started_at: event.at, map: "arena", mode: "multiplayer", players: [],
      kills: [{ ...shot, at: event.at, position: event.position }],
    });
    applyEvents(s, [
      event, // The stream repeats the snapshot's kill.
      ev("PlayerKilled", "game-g", { tick: 20, victim: 1, killer: 0, kind: "kill", weapon: "battle_rifle", headshot: true }),
      ev("PlayerKilled", "game-g", { tick: 30, victim: 0, killer: 0, kind: "suicide", weapon: "frag_grenade" }),
      ev("PlayerKilled", "game-g", { tick: 40, victim: 0, kind: "world", weapon: "fall" }),
      ev("PlayerKilled", "game-g", { tick: 50, victim: 1, killer: 0, kind: "kill" }),
    ]);
    const kills = s.games.g!.kills;
    expect(kills.map((k) => [k.weapon, k.headshot])).toEqual([
      ["smg", false], ["battle_rifle", true], ["frag_grenade", null], ["fall", null], [null, null],
    ]);
    expect(weaponBreakdown(kills).map((r) => [damageSourceName(r.weapon), r.kills])).toEqual([
      ["Battle Rifle", 1], ["SMG", 1], ["Unknown", 1],
    ]);
    expect(damageSourceName("frag_grenade")).toBe("Frag Grenade");
    expect(damageSourceName("energy_blade")).toBe("Energy Sword");
  });

  test("breakdown by weapon for a slot, most used first", () => {
    const kills = [k(0, 0, 1, "rifle"), k(1, 0, 2, "sword"), k(2, 0, 3, "rifle"), k(3, 1, 0, "rocket"), k(4, 0, 0, null, "suicide"), k(5, 0, 2, null)];
    const rows = weaponBreakdown(kills, new Set([0]));
    expect(rows.map((r) => [r.weapon, r.kills])).toEqual([["rifle", 2], ["sword", 1], ["unknown", 1]]);
    expect(rows[0]!.share).toBeCloseTo(0.5);
    expect(weaponBreakdown(kills).find((r) => r.weapon === "rocket")!.kills).toBe(1);
  });
});

describe("history, places and service record", () => {
  const A = "0009000000000001", B = "0009000000000002";
  function world() {
    resetPos();
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-f", { game_id: "f", map: "teacup", mode: "multiplayer", team_play: false, players: [
        { index: 0, name: "a", xuid: A, team: 0 }, { index: 1, name: "b", xuid: B, team: 1 }, { index: 2, name: "c", team: 2 },
      ] }, "2026-11-15T10:00:00.000Z"),
      ev("PlayerKilled", "game-f", { tick: 10, victim: 1, killer: 0, weapon: "rifle", kind: "kill" }),
      ev("PlayerKilled", "game-f", { tick: 20, victim: 2, killer: 0, weapon: "rifle", kind: "kill" }),
      ev("GameEnded", "game-f", { tick: 30, reason: "finished", winner: { player: 0 }, players: [
        { index: 0, score: 2, kills: 2, deaths: 0, assists: 0 }, { index: 1, score: 1, kills: 1, deaths: 1, assists: 0 },
        { index: 2, score: 1, kills: 0, deaths: 1, assists: 2 },
      ] }, "2026-11-15T10:05:00.000Z"),
      ev("GameStarted", "game-t", { game_id: "t", map: "orchard", mode: "multiplayer", team_play: true, players: [
        { index: 0, name: "a", xuid: A, team: 0 }, { index: 1, name: "b", xuid: B, team: 1 },
      ] }, "2026-11-15T11:00:00.000Z"),
      ev("ScoreChanged", "game-t", { tick: 5, players: [], teams: [{ team: 0, score: 3 }, { team: 1, score: 7 }] }),
      ev("GameMedal", "game-f", { tick: 20, player: 0, medal: "double-kill" }),
    ]);
    // The server's count for the team game: A in slot 0, B in slot 1.
    applyGameSnapshot(s, {
      game_id: "t", map: "orchard", mode: "multiplayer", started_at: "2026-11-15T11:00:00.000Z", players: [],
      medals: [
        { player: 0, name: "a", xuid: A, medals: { "superbounce": 2, "killing-spree": 1 } },
        { player: 1, name: "b", xuid: B, medals: { "double-kill": 5 } },
      ],
    });
    return s;
  }

  test("places: FFA ties share a place, teams by team score", () => {
    const s = world();
    const f = places(s.games.f!);
    expect([f.get(0), f.get(1), f.get(2)]).toEqual([1, 2, 2]);
    const t = places(s.games.t!);
    expect([t.get(1), t.get(0)]).toEqual([1, 2]);
  });

  test("history rows and totals", () => {
    const s = world();
    const h = gameHistory(s, A);
    expect(h.map((r) => r.game.game_id)).toEqual(["t", "f"]);
    expect(h[1]).toMatchObject({ place: 1, kills: 2, deaths: 0, won: true, bestSpree: 2 });
    const rec = serviceRecord(s, A);
    expect(rec).toMatchObject({ games: 2, completed: 1, wins: 1, kills: 2, deaths: 0, kd: null, exp: 3 });
    expect(rec.weapons).toEqual([{ weapon: "rifle", kills: 2, share: 1 }]);
    // The server's medals, summed over A's slot in each game; none of B's.
    expect(Object.fromEntries(rec.medals)).toEqual({ "double-kill": 1, "superbounce": 2, "killing-spree": 1 });
    expect(rankFor(0)).toBe("Newcomer");
    expect(rankFor(16)).toBe("Contender");
    expect(playedWith(s, A)).toEqual([{ xuid: B, games: 2, lastPlayed: "2026-11-15T11:00:00.000Z" }]);
  });

  test("a record can be narrowed to some games", () => {
    const s = world();
    const ffa = serviceRecord(s, A, (g) => g.game_id === "f");
    expect(ffa).toMatchObject({ games: 1, completed: 1, wins: 1, kills: 2 });
    expect(serviceRecord(s, A, () => false)).toMatchObject({ games: 0, kills: 0, exp: 0, rank: "Newcomer" });
  });

  test("rank ladder positions", () => {
    expect(rankIndex(0)).toBe(0);
    expect(rankIndex(16)).toBe(2);
    expect(nextRankAt(16)).toBe(30);
    expect(nextRankAt(100000)).toBeNull();
  });
});

describe("weapons against", () => {
  test("counts what knocked a slot out, ignoring suicides", () => {
    const kills = [k(0, 1, 0, "sniper"), k(10, 2, 0, "sniper"), k(20, 1, 0, "sword"), k(30, 0, 0, null, "suicide"), k(40, 0, 1, "rifle")];
    expect(weaponsAgainst(kills, new Set([0])).map((r) => [r.weapon, r.kills])).toEqual([["sniper", 2], ["sword", 1]]);
  });
});
