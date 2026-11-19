// Campaign and multiplayer are two halves of the stats: campaign games never
// count towards multiplayer K/D, wins, medals or leaderboards, and have
// their own record.

import { describe, expect, test } from "bun:test";
import { applyEvents, endedGames, gamesFor, initialState, isCampaign, liveGames, modeOf, playerStats } from "../src/state/reducer";
import { campaignHistory, campaignOutcome, campaignRecord, gameHistory, playedWith, serviceRecord } from "../src/state/derive";
import { teamRecord } from "../src/state/social-model";
import { campaignLabel, difficultyName, gameKind, gameTypeLabel, gameTypeLine } from "../src/state/gametype";
import { clanHref, historyMode, historyTab, recordHref, spotlightTab } from "../src/stats/modes";
import { World } from "../mock/world";
import { ev, resetPos } from "./helpers";

const A = "0009000000000001", B = "0009000000000002", C = "0009000000000003";
const NOW = Date.parse("2026-11-15T23:00:00.000Z");

function world() {
  resetPos();
  const s = initialState();
  applyEvents(s, [
    // Multiplayer: A 4 kills 2 deaths, wins.
    ev("GameStarted", "game-m", { game_id: "m", map: "teacup", mode: "multiplayer", team_play: false, players: [
      { index: 0, name: "a", xuid: A, team: 0 }, { index: 1, name: "b", xuid: B, team: 1 },
    ] }, "2026-11-15T10:00:00.000Z"),
    ev("PlayerKilled", "game-m", { tick: 40, victim: 1, killer: 0, weapon: "rifle", kind: "kill" }),
    ev("PlayerKilled", "game-m", { tick: 80, victim: 1, killer: 0, weapon: "rifle", kind: "kill" }),
    ev("GameEnded", "game-m", { tick: 300, reason: "finished", winner: { player: 0 }, players: [
      { index: 0, score: 4, kills: 4, deaths: 2, assists: 1 }, { index: 1, score: 2, kills: 2, deaths: 4, assists: 0 },
    ] }, "2026-11-15T10:10:00.000Z"),
    // Campaign, co-op with B, finished: lots of AI kills and some deaths,
    // which must not touch A's multiplayer K/D. 90 s of game time.
    ev("GameStarted", "game-c1", { game_id: "c1", map: "harbor_dawn", mode: "campaign", team_play: false, tick: 0, players: [
      { index: 0, name: "a", xuid: A, team: 0 }, { index: 1, name: "b", xuid: B, team: 0 },
    ] }, "2026-11-15T12:00:00.000Z"),
    ev("PlayerKilled", "game-c1", { tick: 300, victim: 0, kind: "kill", weapon: "plasma" }),
    ev("PlayerKilled", "game-c1", { tick: 600, victim: 0, kind: "world" }),
    ev("PlayerKilled", "game-c1", { tick: 900, victim: 1, kind: "kill" }),
    ev("ScoreChanged", "game-c1", { tick: 1000, players: [
      { index: 0, score: 0, kills: 30, deaths: 1, assists: 0 }, { index: 1, score: 0, kills: 12, deaths: 1, assists: 0 },
    ] }),
    ev("GameEnded", "game-c1", { tick: 2700, reason: "finished", players: [
      { index: 0, score: 0, kills: 31, deaths: 1, assists: 0 }, { index: 1, score: 0, kills: 12, deaths: 1, assists: 0 },
    ] }, "2026-11-15T12:02:00.000Z"),
    // Campaign, solo, quit: no scoreboard at all.
    ev("GameStarted", "game-c2", { game_id: "c2", map: "harbor_dawn", mode: "campaign", team_play: false, tick: 0, players: [
      { index: 0, name: "a", xuid: A, team: 0 },
    ] }, "2026-11-15T13:00:00.000Z"),
    ev("PlayerKilled", "game-c2", { tick: 100, victim: 0, kind: "kill" }),
    ev("GameEnded", "game-c2", { tick: 1800, reason: "quit" }, "2026-11-15T13:01:00.000Z"),
    // Campaign, still being played, with A, B and C.
    ev("GameStarted", "game-c3", { game_id: "c3", map: "glass_canyon", mode: "campaign", team_play: false, tick: 0, players: [
      { index: 0, name: "a", xuid: A, team: 0 }, { index: 1, name: "b", xuid: B, team: 0 }, { index: 2, name: "c", xuid: C, team: 0 },
    ] }, "2026-11-15T22:59:00.000Z"),
  ]);
  return s;
}

describe("mode", () => {
  test("campaign is only `campaign`; anything else is multiplayer", () => {
    expect(modeOf({ mode: "campaign" })).toBe("campaign");
    expect(modeOf({ mode: "multiplayer" })).toBe("multiplayer");
    expect(modeOf({ mode: "" })).toBe("multiplayer");
    expect(isCampaign({ mode: "campaign" })).toBe(true);
  });

  test("game lists split by mode, and default to both", () => {
    const s = world();
    expect(endedGames(s, 20).map((g) => g.game_id)).toEqual(["c2", "c1", "m"]);
    expect(endedGames(s, 20, "multiplayer").map((g) => g.game_id)).toEqual(["m"]);
    expect(endedGames(s, 20, "campaign").map((g) => g.game_id)).toEqual(["c2", "c1"]);
    expect(endedGames(s, 1, "multiplayer").map((g) => g.game_id)).toEqual(["m"]);
    expect(liveGames(s).map((g) => g.game_id)).toEqual(["c3"]);
    expect(liveGames(s, "multiplayer")).toEqual([]);
    expect(gamesFor(s, A, "campaign").map((g) => g.game_id)).toEqual(["c3", "c2", "c1"]);
  });
});

describe("multiplayer stays multiplayer", () => {
  test("K/D, wins and games ignore campaign games", () => {
    const s = world();
    const rec = serviceRecord(s, A);
    expect(rec).toMatchObject({ games: 1, completed: 1, wins: 1, kills: 4, deaths: 2, assists: 1, exp: 3 });
    expect(rec.kd).toBe(2);
    expect(gameHistory(s, A).map((h) => h.game.game_id)).toEqual(["m"]);
    expect(playerStats(s, A)).toEqual({ games: 1, kills: 4, deaths: 2, wins: 1, live: 0 });
    // Campaign deaths never show up as knockouts against anyone.
    expect(rec.weaponsAgainst).toEqual([]);
  });

  test("an account with only campaign games has an empty multiplayer record", () => {
    const s = world();
    expect(serviceRecord(s, C)).toMatchObject({ games: 0, kills: 0, deaths: 0, kd: null, rank: "Newcomer" });
  });

  test("played with: everyone, multiplayer, or co-op partners", () => {
    const s = world();
    expect(playedWith(s, A).map((f) => [f.xuid, f.games])).toEqual([[B, 3], [C, 1]]);
    expect(playedWith(s, A, "multiplayer").map((f) => [f.xuid, f.games])).toEqual([[B, 1]]);
    expect(playedWith(s, A, "campaign").map((f) => [f.xuid, f.games])).toEqual([[B, 2], [C, 1]]);
  });

  test("team records count multiplayer only and list shared campaign games apart", () => {
    const s = world();
    const r = teamRecord(s, [A, B]);
    expect(r.games.map((g) => g.game.game_id)).toEqual(["m"]);
    expect(r).toMatchObject({ together: 1, wins: 1, kills: 6, deaths: 6 });
    expect(r.campaign.map((g) => g.game_id)).toEqual(["c3", "c1"]);
  });
});

describe("campaign record", () => {
  test("outcome follows the end reason", () => {
    expect(campaignOutcome({ status: "ended", end_reason: "finished" })).toBe("complete");
    expect(campaignOutcome({ status: "ended", end_reason: "quit" })).toBe("quit");
    expect(campaignOutcome({ status: "ended", end_reason: "lost" })).toBe("lost");
    expect(campaignOutcome({ status: "ended", end_reason: null })).toBe("ended");
    expect(campaignOutcome({ status: "live", end_reason: null })).toBe("playing");
  });

  test("history rows: deaths, AI kills when reported, partners and time", () => {
    const s = world();
    const h = campaignHistory(s, A, NOW);
    expect(h.map((r) => r.game.game_id)).toEqual(["c3", "c2", "c1"]);
    const [live, solo, coop] = h;
    expect(coop).toMatchObject({ outcome: "complete", deaths: 2, kills: 31, partners: ["b"], seconds: 90 });
    // No scoreboard: deaths come from the death events, kills are unknown.
    expect(solo).toMatchObject({ outcome: "quit", deaths: 1, kills: null, partners: [], seconds: 60 });
    expect(live).toMatchObject({ outcome: "playing", partners: ["b", "c"] });
  });

  test("totals, levels and co-op partners", () => {
    const s = world();
    const rec = campaignRecord(s, A, NOW);
    expect(rec).toMatchObject({ games: 3, completed: 1, unfinished: 1, live: 1, coop: 2, deaths: 3, kills: 31 });
    expect(rec.seconds).toBe(90 + 60 + 60);
    expect(rec.levels.map((l) => [l.map, l.plays, l.completions, l.fastest])).toEqual([
      ["glass_canyon", 1, 0, null], ["harbor_dawn", 2, 1, 90],
    ]);
    expect(rec.partners.map((p) => p.xuid)).toEqual([B, C]);
    expect(campaignRecord(s, C, NOW)).toMatchObject({ games: 1, completed: 0, kills: null });
  });

  test("a game the server ended as lost before any frame counts as unfinished, with no time", () => {
    const s = world();
    applyEvents(s, [
      ev("GameStarted", "game-c4", { game_id: "c4", map: "harbor_dawn", mode: "campaign", team_play: false, tick: 0, players: [
        { index: 0, name: "a", xuid: A, team: 0 },
      ] }, "2026-11-15T14:00:00.000Z"),
      // The console went quiet; the server's idle sweep gave up 5 minutes on.
      ev("GameEnded", "game-c4", { tick: 0, reason: "lost", winner: null, players: [] }, "2026-11-15T14:05:01.000Z"),
    ]);
    const row = campaignHistory(s, A, NOW).find((r) => r.game.game_id === "c4")!;
    expect(row).toMatchObject({ outcome: "lost", seconds: 0 });
    const rec = campaignRecord(s, A, NOW);
    expect(rec).toMatchObject({ games: 4, completed: 1, unfinished: 2, live: 1 });
    expect(rec.seconds).toBe(90 + 60 + 60);
  });

  test("a player with no campaign games has an empty record", () => {
    resetPos();
    const s = initialState();
    expect(campaignRecord(s, A, NOW)).toMatchObject({ games: 0, kills: null, levels: [], partners: [], firstPlayed: null });
  });
});

describe("game type labels", () => {
  const roster = (n: number) => Object.fromEntries(Array.from({ length: n }, (_, i) => [i, {}])) as never;
  const g = (o: Record<string, unknown>) => ({
    mode: "multiplayer", kind: null, playlist: null, online: null, players: roster(1), variant: null, difficulty: null, ...o,
  }) as Parameters<typeof gameTypeLine>[0];

  test("kind: as sent, else inferred from the mode", () => {
    expect(gameKind(g({}))).toBe("custom");
    expect(gameKind(g({ kind: "matchmaking" }))).toBe("matchmaking");
    expect(gameKind(g({ kind: "clankermatch" }))).toBe("clankermatch");
    expect(gameKind(g({ mode: "campaign" }))).toBe("campaign");
    // A campaign game stays campaign whatever its kind says.
    expect(gameKind(g({ mode: "campaign", kind: "custom" }))).toBe("campaign");
    expect(gameKind(g({ kind: "campaign" }))).toBe("custom");
  });

  test("labels per the contract", () => {
    expect(gameTypeLabel(g({ kind: "custom" }))).toBe("Custom Game");
    expect(gameTypeLabel(g({ kind: "matchmaking", playlist: "Quad Squad" }))).toBe("Matchmaking: Quad Squad");
    expect(gameTypeLabel(g({ kind: "matchmaking" }))).toBe("Matchmaking");
    expect(gameTypeLabel(g({ kind: "clankermatch", playlist: "Clanker Slayer" }))).toBe("Clankermatch: Clanker Slayer");
    expect(gameTypeLabel(g({ kind: "clankermatch" }))).toBe("Clankermatch");
    expect(campaignLabel(g({ mode: "campaign" }))).toBe("Campaign");
    expect(campaignLabel(g({ mode: "campaign", players: roster(2) }))).toBe("Campaign Co-op");
    expect(campaignLabel(g({ mode: "campaign", players: roster(2), online: true }))).toBe("Campaign Online");
  });

  test("one line: variant and type for multiplayer, label and difficulty for campaign", () => {
    expect(gameTypeLine(g({ variant: "Slayer" }))).toBe("Slayer · Custom Game");
    expect(gameTypeLine(g({ variant: "team_slayer", kind: "matchmaking", playlist: "Night Owls" }))).toBe("Team Slayer · Matchmaking: Night Owls");
    expect(gameTypeLine(g({ mode: "campaign", players: roster(3), difficulty: "heroic" }))).toBe("Campaign Co-op · Heroic");
    expect(gameTypeLine(g({ mode: "campaign" }))).toBe("Campaign");
    expect(difficultyName("LEGENDARY")).toBe("Legendary");
    expect(difficultyName(null)).toBeNull();
  });

  test("the reducer keeps the optional fields, softly", () => {
    resetPos();
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-x", { game_id: "x", map: "01a_dock", mode: "campaign", kind: "campaign", difficulty: "Heroic", online: false, team_play: false, players: [] }),
      ev("GameStarted", "game-y", { game_id: "y", map: "teacup", mode: "multiplayer", kind: "matchmaking", playlist: "Quad Squad", playlist_id: "quad", online: "yes", team_play: false, players: [] }),
      ev("GameStarted", "game-z", { game_id: "z", map: "teacup", mode: "multiplayer", team_play: false, players: [] }),
    ]);
    expect(s.games.x).toMatchObject({ kind: "campaign", difficulty: "heroic", online: false, playlist: null });
    expect(s.games.y).toMatchObject({ kind: "matchmaking", playlist: "Quad Squad", playlist_id: "quad", online: null });
    expect(s.games.z).toMatchObject({ kind: null, playlist: null, difficulty: null, online: null });
  });
});

describe("routing", () => {
  test("the online page's spotlight tab", () => {
    expect(spotlightTab(undefined, true)).toBe("live");
    expect(spotlightTab(undefined, false)).toBe("recent");
    expect(spotlightTab("campaign", true)).toBe("campaign");
    expect(spotlightTab("nonsense", false)).toBe("recent");
  });

  test("history tabs default to multiplayer", () => {
    expect(historyTab(undefined)).toBe("all");
    expect(historyTab("campaign")).toBe("campaign");
    expect(historyTab("bogus")).toBe("all");
    expect(historyMode("campaign")).toBe("campaign");
    expect(historyMode("team")).toBe("multiplayer");
  });

  test("clans live under /games/halo2/clans", () => {
    expect(clanHref("abc123")).toBe("/games/halo2/clans/abc123");
  });

  test("service record halves", () => {
    expect(recordHref("Sneaky Pete", "multiplayer")).toBe("/games/halo2/players/Sneaky%20Pete");
    expect(recordHref("Sneaky Pete", "campaign")).toBe("/games/halo2/players/Sneaky%20Pete/campaign");
  });
});

describe("mock campaign games", () => {
  test("occasional campaign games: invented levels, 1 to 4 players, difficulty, no scores or winner", () => {
    const w = new World();
    w.seed(90 * 60_000);
    const all = [...w.games.values()];
    const camp = all.filter((g) => g.mode === "campaign");
    expect(camp.length).toBeGreaterThan(0);
    expect(camp.length).toBeLessThan(all.length / 2);
    for (const g of camp) {
      expect(g.map.name).toMatch(/^\d\d[a-z]_/);
      expect(g.players.length).toBeGreaterThanOrEqual(1);
      expect(g.players.length).toBeLessThanOrEqual(4);
      expect(g.winner).toBeNull();
      const started = w.log.find((e) => e.type === "GameStarted" && e.stream === `game-${g.id}`)!.data as Record<string, unknown>;
      expect(started).toMatchObject({ mode: "campaign", kind: "campaign", team_play: false });
      expect(started.score_limit).toBeUndefined();
      expect(started.variant).toBeUndefined();
      expect(["easy", "normal", "heroic", "legendary"]).toContain(started.difficulty as string);
      expect(g.players.every((p) => p.score.score === 0)).toBe(true);
    }
    const mp = all.filter((g) => g.mode !== "campaign");
    expect(mp.some((g) => g.kind.kind === "matchmaking" && typeof g.kind.playlist === "string")).toBe(true);
    expect(mp.some((g) => g.kind.kind === "custom")).toBe(true);
  }, 30_000);
});
