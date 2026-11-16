// Skulls: the name table, the `?skulls=` query, filtering, the reducer's
// reading of `skulls`, a player's "played with" set, the API call and the
// mock's skulls.

import { afterEach, describe, expect, test } from "bun:test";
import {
  MAX_SKULL, RETAIL_SKULLS, SKULLS, countSkulls, isOurSkull, filterBySkulls, formatSkullsParam, hasAllSkulls, normalizeSkulls, parseSkullsParam, skullEffect, skullName, skullTier, skullTitle, skullsOfTier, tierOrder, toggleSkull,
} from "../src/lib/skulls";
import { applyEvents, applyGameSnapshot, initialState } from "../src/state/reducer";
import { campaignRecord, skullsPlayedWith } from "../src/state/derive";
import { api } from "../src/api/http";
import { targetFor } from "../src/api/server";
import { World } from "../mock/world";
import { mockSkulls } from "../mock/coop";
import { ev, resetPos } from "./helpers";

const A = "0009000000000001";

describe("skull table", () => {
  test("the fifteen skulls with the agreed names, then our six, in index order", () => {
    expect(SKULLS.map((s) => s.name)).toEqual([
      "Envy", "Grunt Birthday Party", "Assassins", "Thunderstorm", "Famine", "IWHBYD", "Blind", "Ghost", "Black Eye", "Catch", "Sputnik", "Iron", "Mythic", "Angry", "Bandana",
      "Acrophobia", "Low Gravity", "Fall Damage", "Unlimited Ammo", "Unlimited Grenades", "Invincible",
    ]);
    expect(RETAIL_SKULLS).toBe(15);
    expect(isOurSkull(14)).toBe(false);
    expect(isOurSkull(15) && isOurSkull(20)).toBe(true);
    expect(isOurSkull(21)).toBe(false);
    SKULLS.forEach((s, i) => { expect(s.index).toBe(i); expect(s.effect.length).toBeGreaterThan(10); });
  });

  test("name, effect and tooltip by index, with a fallback past the table", () => {
    expect(skullName(10)).toBe("Sputnik");
    expect(skullEffect(14)).toBe("unlimited ammo and grenades");
    expect(skullTitle(11)).toBe("Iron: a death sends you back to the last checkpoint (Gold skull)");
    expect(skullName(40)).toBe("Skull 40");
    expect(skullTitle(40)).toBe("Skull 40 (Silver skull)");
    expect(skullTitle(0)).toEndWith("(Silver skull)");
  });

  test("tiers as listed, ours included; unknown indices are silver", () => {
    expect(SKULLS.filter((s) => s.tier === "gold").map((s) => s.index)).toEqual([2, 3, 4, 6, 8, 9, 11, 12, 13, 17]);
    expect(SKULLS.filter((s) => s.tier === "silver").map((s) => s.index)).toEqual([0, 1, 5, 7, 10, 14, 15, 16, 18, 19, 20]);
    expect(skullsOfTier("silver").map((s) => s.name)).toEqual([
      "Envy", "Grunt Birthday Party", "IWHBYD", "Ghost", "Sputnik", "Bandana", "Acrophobia", "Low Gravity", "Unlimited Ammo", "Unlimited Grenades", "Invincible",
    ]);
    expect(skullTier(12)).toBe("gold");
    expect(skullTier(17)).toBe("gold");
    expect(skullTier(40)).toBe("silver");
  });

  test("our skulls by name and effect", () => {
    expect(skullName(15)).toBe("Acrophobia");
    expect(skullName(20)).toBe("Invincible");
    expect(skullTitle(18)).toBe("Unlimited Ammo: spare ammo never runs out, but you still reload (Silver skull)");
    expect(skullTitle(17)).toEndWith("(Gold skull)");
  });

  test("display order is gold first, then silver, engine order within each", () => {
    expect(tierOrder([14, 0, 13, 2, 7, 4])).toEqual([2, 4, 13, 0, 7, 14]);
    expect(tierOrder([20, 3, 20, 17])).toEqual([3, 17, 20]);
    expect(tierOrder([40, 3, 40, 37])).toEqual([3, 37, 40]);
    expect(tierOrder([])).toEqual([]);
  });

  test("unknown indices past the table are kept, up to 63", () => {
    expect(normalizeSkulls([37, 63, 64, 14])).toEqual([14, 37, 63]);
    expect(parseSkullsParam("37,63,64,2")).toEqual([2, 37, 63]);
    expect(MAX_SKULL).toBe(63);
    expect(skullEffect(37)).toBe("");
  });

  test("Famine, Blind and our Low Gravity are marked as working in multiplayer", () => {
    expect(SKULLS.filter((s) => s.multiplayer).map((s) => s.index)).toEqual([4, 6, 16]);
  });
});

describe("skulls query", () => {
  test("parses a comma list: sorted, unique, junk and out-of-range dropped", () => {
    expect(parseSkullsParam("10,14")).toEqual([10, 14]);
    expect(parseSkullsParam("14, 10,10")).toEqual([10, 14]);
    expect(parseSkullsParam("3,x,-1,64,2.5,")).toEqual([3]);
    expect(parseSkullsParam("")).toEqual([]);
    expect(parseSkullsParam(undefined)).toEqual([]);
    expect(parseSkullsParam(null)).toEqual([]);
  });

  test("formats back, leaving the param out when nothing is chosen", () => {
    expect(formatSkullsParam([14, 10])).toBe("10,14");
    expect(formatSkullsParam(new Set([6]))).toBe("6");
    expect(formatSkullsParam([])).toBeUndefined();
    expect(parseSkullsParam(formatSkullsParam([1, 2, 3]))).toEqual([1, 2, 3]);
  });

  test("normalize keeps only whole indices 0..14", () => {
    expect(normalizeSkulls([4, 4, 0, 14, 15, "3", 1.5, null])).toEqual([0, 4, 14, 15]);
    expect(normalizeSkulls("4")).toEqual([]);
    expect(normalizeSkulls(undefined)).toEqual([]);
  });

  test("toggling adds and removes, keeping the order", () => {
    expect(toggleSkull([], 10)).toEqual([10]);
    expect(toggleSkull([10], 2)).toEqual([2, 10]);
    expect(toggleSkull([2, 10], 10)).toEqual([2]);
  });
});

describe("filtering", () => {
  const games = [
    { id: "a", skulls: [10, 14] }, { id: "b", skulls: [10] }, { id: "c", skulls: undefined }, { id: "d", skulls: [] as number[] }, { id: "e", skulls: [0, 10, 14] },
  ];
  const by = (w: number[]) => filterBySkulls(games, w, (g) => g.skulls).map((g) => g.id);

  test("a game must have all the listed skulls on", () => {
    expect(by([10, 14])).toEqual(["a", "e"]);
    expect(by([10])).toEqual(["a", "b", "e"]);
    expect(by([4])).toEqual([]);
  });

  test("no filter keeps everything, including games without skulls", () => {
    expect(by([])).toEqual(["a", "b", "c", "d", "e"]);
    expect(hasAllSkulls(undefined, [])).toBe(true);
    expect(hasAllSkulls(undefined, [1])).toBe(false);
  });
});

describe("reducer", () => {
  test("a GameStarted event and snapshots carry skulls; none reads as an empty list", () => {
    resetPos();
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-g1", { game_id: "g1", map: "m", mode: "campaign", team_play: false, skulls: [14, 10, 10, 99], players: [] }),
      ev("GameStarted", "game-g2", { game_id: "g2", map: "m", mode: "multiplayer", team_play: false, players: [] }),
    ]);
    expect(s.games.g1!.skulls).toEqual([10, 14]);
    expect(s.games.g2!.skulls).toEqual([]);
    applyGameSnapshot(s, { game_id: "g3", map: "m", mode: "multiplayer", started_at: "2026-11-16T10:00:00Z", skulls: [4, 6], players: [] });
    expect(s.games.g3!.skulls).toEqual([4, 6]);
    // A detail read fills in a game first known without them.
    applyGameSnapshot(s, { game_id: "g2", map: "m", mode: "multiplayer", started_at: "2026-11-16T10:00:00Z", skulls: [6], players: [] });
    expect(s.games.g2!.skulls).toEqual([6]);
  });
});

describe("skulls played with", () => {
  function campaign() {
    resetPos();
    const s = initialState();
    const start = (id: string, extra: Record<string, unknown>, at: string) => [
      ev("GameStarted", `game-${id}`, { game_id: id, map: "harbor_dawn", mode: "campaign", team_play: false, tick: 0, players: [{ index: 0, name: "a", xuid: A, team: 0 }], ...extra }, at),
      ev("GameEnded", `game-${id}`, { tick: 900, reason: "finished" }, at.replace(":00.000Z", ":30.000Z")),
    ];
    applyEvents(s, [
      ...start("c1", { skulls: [10, 14] }, "2026-11-15T10:00:00.000Z"),
      ...start("c2", { skulls: [14] }, "2026-11-15T11:00:00.000Z"),
      ...start("c3", {}, "2026-11-15T12:00:00.000Z"),
      ...start("c4", { skulls: [0, 10, 14] }, "2026-11-15T13:00:00.000Z"),
      // A multiplayer game with a skull is not a campaign game.
      ev("GameStarted", "game-m", { game_id: "m", map: "teacup", mode: "multiplayer", team_play: false, skulls: [4], players: [{ index: 0, name: "a", xuid: A, team: 0 }] }, "2026-11-15T14:00:00.000Z"),
    ]);
    return s;
  }

  test("each skull across the player's campaign games, with its game count, in index order", () => {
    const rec = campaignRecord(campaign(), A, Date.parse("2026-11-15T20:00:00.000Z"));
    expect(rec.games).toBe(3 + 1);
    expect(rec.skulls).toEqual([{ index: 0, games: 1 }, { index: 10, games: 2 }, { index: 14, games: 3 }]);
  });

  test("a player with no skulls on has none, and a repeated skull counts once per game", () => {
    expect(skullsPlayedWith([{ game: { skulls: [] } }])).toEqual([]);
    expect(countSkulls([{ s: [3, 3] }], (g) => g.s)).toEqual([{ index: 3, games: 1 }]);
    expect(countSkulls([], () => undefined)).toEqual([]);
  });

  test("the list is gold first, then silver, and keeps unknown indices", () => {
    const rows = [{ s: [0, 14] }, { s: [13, 37] }, { s: [2] }];
    expect(countSkulls(rows, (r) => r.s).map((c) => c.index)).toEqual([2, 13, 0, 14, 37]);
  });
});

describe("the games request", () => {
  const real = globalThis.fetch;
  afterEach(() => { globalThis.fetch = real; });

  test("asks for ?skulls= only when skulls are chosen", async () => {
    const urls: string[] = [];
    globalThis.fetch = (async (u: string) => { urls.push(String(u)); return new Response(JSON.stringify({ items: [], next: null, position: 1 })); }) as unknown as typeof fetch;
    const a = api(targetFor("http://h2.test", null));
    await a.games("ended", 100, undefined, [14, 10]);
    await a.games("ended", 100);
    await a.games("live", 100, undefined, []);
    expect(new URL(urls[0]!).searchParams.get("skulls")).toBe("10,14");
    expect(new URL(urls[0]!).searchParams.get("state")).toBe("ended");
    expect(new URL(urls[1]!).searchParams.has("skulls")).toBe(false);
    expect(new URL(urls[2]!).searchParams.has("skulls")).toBe(false);
  });
});

describe("mock skulls", () => {
  test("multiplayer games only get 4 and 6, campaign any, and none is left out", () => {
    let mpSome = 0, campSome = 0, wide = new Set<number>();
    for (let i = 0; i < 400; i++) {
      const id = i.toString(16).padStart(16, "0");
      const mp = mockSkulls(id, false), camp = mockSkulls(id, true);
      expect(mp.every((n) => n === 4 || n === 6)).toBe(true);
      expect(normalizeSkulls(mp)).toEqual(mp);
      expect(normalizeSkulls(camp)).toEqual(camp);
      if (mp.length) mpSome++;
      if (camp.length) campSome++;
      camp.forEach((n) => wide.add(n));
      expect(mockSkulls(id, true)).toEqual(camp);
    }
    expect(mpSome).toBeGreaterThan(40);
    expect(mpSome).toBeLessThan(300);
    expect(campSome).toBeGreaterThan(40);
    expect(wide.size).toBeGreaterThan(10);
  });

  test("the world sends skulls in GameStarted and in the summary, never an empty list", () => {
    const w = new World();
    w.seed(90 * 60_000);
    let seen = 0;
    for (const g of w.games.values()) {
      const started = w.log.find((e) => e.type === "GameStarted" && e.stream === `game-${g.id}`)!.data as { skulls?: number[] };
      const summary = w.gameSummary(g) as { skulls?: number[] };
      expect(summary.skulls).toEqual(started.skulls);
      if (started.skulls) {
        seen++;
        expect(started.skulls.length).toBeGreaterThan(0);
        if (g.mode !== "campaign") expect(started.skulls.every((n) => n === 4 || n === 6)).toBe(true);
      }
    }
    expect(seen).toBeGreaterThan(0);
  }, 30_000);
});
