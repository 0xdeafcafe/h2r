// The superbounce medals (docs/game/superbounce.md, "Medals"): the copy of
// the chain table, a game's bounces from its events and its snapshot, and
// their medals in the game, the service record and the medal art.
import { describe, expect, test } from "bun:test";
import type { Bounce } from "../src/state/reducer";
import { applyEvents, applyGameSnapshot, initialState } from "../src/state/reducer";
import {
  BOUNCE_MEDALS, MEDALS, MEDAL_BY_ID, bounceMedals, countMedals, gameAwards, gameBounceMedals, isBounceMedal, serviceRecord,
} from "../src/state/derive";
import { PACK_MEDALS, medalArt } from "../src/lib/art";
import { medalKey } from "../src/lib/gameart";
import { STATS_EVENT_KINDS, isStatsEvent } from "../src/stats/eventKinds";
import { ev, resetPos } from "./helpers";

const A = "0009000000000001", B = "0009000000000002";

describe("the bounce medal table", () => {
  // `bounce_medals` in crates/live/live-proto/src/film/events/medals.rs.
  test("matches the canonical table: each chain medal once, 7 and over Superbounce only", () => {
    expect(bounceMedals(0)).toEqual([]);
    expect(bounceMedals(1)).toEqual(["superbounce"]);
    expect(bounceMedals(2)).toEqual(["superbounce", "double-bounce"]);
    expect(bounceMedals(3)).toEqual(["superbounce", "triple-bounce"]);
    expect(bounceMedals(4)).toEqual(["superbounce", "quad-bounce"]);
    expect(bounceMedals(5)).toEqual(["superbounce", "chain-bounce"]);
    expect(bounceMedals(6)).toEqual(["superbounce", "bouncetacular"]);
    for (const chain of [7, 8, 20]) expect(bounceMedals(chain)).toEqual(["superbounce"]);
    expect(bounceMedals(Number.NaN)).toEqual([]);
  });

  test("ids, names and the medal list", () => {
    expect(BOUNCE_MEDALS).toEqual([
      "superbounce", "double-bounce", "triple-bounce", "quad-bounce", "chain-bounce", "bouncetacular", "bounced",
    ]);
    expect(BOUNCE_MEDALS.map((id) => MEDAL_BY_ID[id].name)).toEqual([
      "Superbounce", "Double Bounce", "Triple Bounce", "Quad Bounce", "Chain Bounce", "Bouncetacular", "Bounced",
    ]);
    expect(MEDALS.filter((m) => isBounceMedal(m.id)).map((m) => m.id)).toEqual(BOUNCE_MEDALS);
    expect(isBounceMedal("twofer")).toBe(false);
  });

  test("a chain of eight is eight Superbounces and one of each other chain medal", () => {
    const bounces: Bounce[] = [1, 2, 3, 4, 5, 6, 7, 8].map((chain, i) => ({ position: i + 1, at: "", tick: 10 * i, player: 0, chain }));
    bounces.push({ position: 20, at: "", tick: 500, player: 0, chain: null });
    const m = countMedals(gameBounceMedals(bounces));
    expect(Object.fromEntries(m)).toEqual({
      "superbounce": 8, "double-bounce": 1, "triple-bounce": 1, "quad-bounce": 1, "chain-bounce": 1, "bouncetacular": 1, "bounced": 1,
    });
  });
});

describe("a game's bounces", () => {
  function world() {
    resetPos();
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-g", { game_id: "g", map: "lockout", mode: "multiplayer", team_play: false, players: [
        { index: 0, name: "a", xuid: A, team: 0 }, { index: 1, name: "b", xuid: B, team: 1 },
        { index: 2, name: "Clanker 2", team: 1, bot: true },
      ] }, "2026-11-15T10:00:00.000Z"),
      ev("Superbounce", "game-g", { tick: 40, player: 0, chain: 1 }),
      ev("Superbounce", "game-g", { tick: 70, player: 0, chain: 2 }),
      ev("Superbounce", "game-g", { tick: 80, player: 2, chain: 1 }),
      ev("PlayerKilled", "game-g", { tick: 90, victim: 1, kind: "world", weapon: "fall" }),
      ev("Bounced", "game-g", { tick: 90, player: 1 }),
      ev("GameEnded", "game-g", { tick: 900, reason: "finished", winner: { player: 0 } }, "2026-11-15T10:05:00.000Z"),
    ]);
    return s;
  }

  test("the stream's events become the game's bounces, in log order", () => {
    const g = world().games.g!;
    expect(g.bounces.map((b) => [b.tick, b.player, b.chain])).toEqual([[40, 0, 1], [70, 0, 2], [80, 2, 1], [90, 1, null]]);
    expect(g.last_tick).toBe(900);
    // The events count towards the game's signature summary.
    for (const b of g.bounces) expect(g.positions).toContain(b.position);
  });

  test("a snapshot's bounces merge with the stream's by log position", () => {
    const s = world();
    const g = s.games.g!;
    const position = g.bounces[0]!.position;
    applyGameSnapshot(s, {
      game_id: "g", map: "lockout", mode: "multiplayer", started_at: "2026-11-15T10:00:00.000Z", players: [],
      bounces: [{ tick: 40, player: 0, chain: 1, position, at: "" }, { tick: 950, player: 1, chain: 3, position: 999, at: "" }],
      bounced: [{ tick: 960, player: 0, position: 1000, at: "" }],
    });
    expect(g.bounces.map((b) => [b.tick, b.chain])).toEqual([[40, 1], [70, 2], [80, 1], [90, null], [950, 3], [960, null]]);

    resetPos();
    const fresh = initialState();
    applyGameSnapshot(fresh, {
      game_id: "h", map: "lockout", mode: "multiplayer", started_at: "2026-11-15T10:00:00.000Z", players: [],
      bounces: [{ tick: 5, player: 0, chain: 2, position: 3, at: "" }],
    });
    expect(fresh.games.h!.bounces).toEqual([{ position: 3, at: "", tick: 5, player: 0, chain: 2 }]);
    expect(Object.fromEntries(countMedals(gameAwards(fresh.games.h!)))).toEqual({ "superbounce": 1, "double-bounce": 1 });
  });

  test("the game's medals hold every player's bounces, a bot's included", () => {
    const g = world().games.g!;
    const awards = gameAwards(g);
    expect(Object.fromEntries(countMedals(awards, new Set([0])))).toEqual({ "superbounce": 2, "double-bounce": 1 });
    expect(Object.fromEntries(countMedals(awards, new Set([1])))).toEqual({ "bounced": 1 });
    expect(Object.fromEntries(countMedals(awards, new Set([2])))).toEqual({ "superbounce": 1 });
  });

  test("a career holds the player's own bounce medals", () => {
    const s = world();
    expect(serviceRecord(s, A).medals.get("superbounce")).toBe(2);
    expect(serviceRecord(s, A).medals.get("double-bounce")).toBe(1);
    expect(serviceRecord(s, B).medals.get("bounced")).toBe(1);
    expect(serviceRecord(s, B).medals.get("superbounce")).toBeUndefined();
  });

  test("bounces are stats events of game streams only", () => {
    expect(STATS_EVENT_KINDS.Superbounce).toEqual({ stream: "game-", category: "bounce" });
    expect(isStatsEvent(ev("Bounced", "game-g", {}))).toBe(true);
    expect(isStatsEvent(ev("Superbounce", "session-g", {}))).toBe(false);
  });
});

describe("the bounce medals' pictures", () => {
  test("no art yet: no pack file and no game counterpart, so the badge shows the name", () => {
    for (const id of BOUNCE_MEDALS) {
      expect(PACK_MEDALS.has(id)).toBe(false);
      expect(medalArt(id, 30)).toBeNull();
      expect(medalKey(id)).toBeNull();
    }
    // The kill medals keep their pack art, named after the ids.
    expect(medalArt("twofer", 30)).toEqual({ src: "/art/medals/twofer.webp", srcset: "/art/medals/twofer.webp 1x, /art/medals/twofer@2x.webp 2x" });
    expect(MEDALS.filter((m) => !isBounceMedal(m.id)).every((m) => PACK_MEDALS.has(m.id))).toBe(true);
  });
});
