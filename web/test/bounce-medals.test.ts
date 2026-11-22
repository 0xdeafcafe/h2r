// The superbounce medals (docs/game/superbounce.md, "Medals"): their ids
// and names, a game's bounces from its events and its snapshot, the
// server's bounce medals on the game and in the service record, and the
// medal art. Live works the medals out; the site only shows them.
import { describe, expect, test } from "bun:test";
import { applyEvents, applyGameSnapshot, initialState } from "../src/state/reducer";
import { BOUNCE_MEDALS, MEDALS, MEDAL_BY_ID, gameMedalCounts, isBounceMedal, serviceRecord } from "../src/state/derive";
import { PACK_MEDALS, medalArt } from "../src/lib/art";
import { medalKey } from "../src/lib/gameart";
import { STATS_EVENT_KINDS, isStatsEvent } from "../src/stats/eventKinds";
import { ev, resetPos } from "./helpers";

const A = "0009000000000001", B = "0009000000000002";

describe("the bounce medal list", () => {
  test("ids, names and the medal list", () => {
    expect(BOUNCE_MEDALS).toEqual([
      "superbounce", "double-bounce", "triple-bounce", "quad-bounce", "chain-bounce", "bouncetacular", "bounced",
      "monster-bounce",
    ]);
    expect(BOUNCE_MEDALS.map((id) => MEDAL_BY_ID[id].name)).toEqual([
      "Superbounce", "Double Bounce", "Triple Bounce", "Quad Bounce", "Chain Bounce", "Bouncetacular", "Bounced",
      "Monster Bounce",
    ]);
    expect(MEDALS.filter((m) => isBounceMedal(m.id)).map((m) => m.id)).toEqual(BOUNCE_MEDALS);
    expect(isBounceMedal("double-kill")).toBe(false);
    expect(isBounceMedal("incel")).toBe(false);
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
      ev("Superbounce", "game-g", { tick: 70, player: 0, chain: 2, monster: true }),
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
    // Only the Monster carries the flag.
    expect(g.bounces.map((b) => b.monster === true)).toEqual([false, true, false, false]);
    expect("monster" in g.bounces[0]!).toBe(false);
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
    // Bounces alone award nothing here: the medals are the server's.
    expect(fresh.games.h!.medals).toEqual({});

    // A snapshot's bounce row carries `monster` beside `chain`.
    resetPos();
    const monster = initialState();
    applyGameSnapshot(monster, {
      game_id: "m", map: "lockout", mode: "multiplayer", started_at: "2026-11-15T10:00:00.000Z", players: [],
      bounces: [{ tick: 5, player: 0, chain: 2, monster: true, position: 3, at: "" }],
    });
    expect(monster.games.m!.bounces).toEqual([{ position: 3, at: "", tick: 5, player: 0, chain: 2, monster: true }]);
  });

  /** The server's medals for `world()`'s game, as `GET /v1/games/{id}` lists them. */
  function withServerMedals() {
    const s = world();
    applyGameSnapshot(s, {
      game_id: "g", map: "lockout", mode: "multiplayer", started_at: "2026-11-15T10:00:00.000Z", players: [],
      medals: [
        { player: 0, name: "a", xuid: A, medals: { "superbounce": 1, "double-bounce": 1, "monster-bounce": 1, "incel": 1 } },
        { player: 1, name: "b", xuid: B, medals: { "bounced": 1, "incel": 1 } },
        { player: 2, name: "Clanker 2", medals: { "superbounce": 1, "incel": 1 } },
      ],
    });
    return s;
  }

  test("the game's medals hold every player's bounce medals, a bot's included", () => {
    const g = withServerMedals().games.g!;
    expect(Object.fromEntries(gameMedalCounts(g, [0]))).toEqual({ "superbounce": 1, "double-bounce": 1, "monster-bounce": 1, "incel": 1 });
    expect(Object.fromEntries(gameMedalCounts(g, [1]))).toEqual({ "bounced": 1, "incel": 1 });
    expect(Object.fromEntries(gameMedalCounts(g, [2]))).toEqual({ "superbounce": 1, "incel": 1 });
  });

  test("a career holds the player's own bounce medals", () => {
    const s = withServerMedals();
    expect(serviceRecord(s, A).medals.get("superbounce")).toBe(1);
    expect(serviceRecord(s, A).medals.get("double-bounce")).toBe(1);
    expect(serviceRecord(s, A).medals.get("monster-bounce")).toBe(1);
    expect(serviceRecord(s, B).medals.get("monster-bounce")).toBeUndefined();
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
  test("the pack draws them, named after the ids, and the game has no counterpart", () => {
    for (const id of BOUNCE_MEDALS) {
      expect(PACK_MEDALS.has(id)).toBe(true);
      expect(medalArt(id, 30)).toEqual({ src: `/art/medals/${id}.webp`, srcset: `/art/medals/${id}.webp 1x, /art/medals/${id}@2x.webp 2x` });
      expect(medalKey(id)).toBeNull();
    }
    // The pack draws only our modes' medals: the bounce family and Incel.
    expect([...PACK_MEDALS].sort()).toEqual([...BOUNCE_MEDALS, "incel"].sort());
    expect(MEDALS.filter((m) => PACK_MEDALS.has(m.id)).map((m) => m.id)).toEqual([...BOUNCE_MEDALS, "incel"]);
    for (const gone of ["twofer", "hat-trick", "full-house", "encore", "hot-streak", "wildfire", "untouchable", "opening-act", "last-word", "payback"]) {
      expect(PACK_MEDALS.has(gone)).toBe(false);
      expect(medalArt(gone, 30)).toBeNull();
    }
    // Halo 2's own have no pack art.
    expect(medalArt("double-kill", 30)).toBeNull();
  });
});
