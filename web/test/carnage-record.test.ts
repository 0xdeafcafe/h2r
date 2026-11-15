// The carnage report's game record: length on the game clock, the signed-in
// player shown by gamertag, and an honest signature summary.
import { beforeEach, describe, expect, test } from "bun:test";
import {
  addGamePositions, applyAccountsSnapshot, applyEvent, applyEvents, applyGameSnapshot, initialState, standings,
} from "../src/state/reducer";
import { gameSeconds, elapsedSeconds } from "../src/state/length";
import { ReplayTimeline } from "../src/state/replay";
import { countResults, summaryLine } from "../src/state/verify";
import type { Frame } from "../src/api/types";
import { ev, resetPos } from "./helpers";

const AFR = "0009000000000042";

beforeEach(resetPos);

describe("game length", () => {
  // The owner's game: 5:20 of play recorded, then the console went quiet and
  // the server ended it as lost after its 5-minute idle wait.
  function lostGame() {
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-g1", { game_id: "g1", map: "m", mode: "multiplayer", team_play: false, tick: 100, players: [{ index: 0, name: "Halo0001", team: 0 }] }, "2026-11-15T20:00:00.000Z"),
      ev("GameEnded", "game-g1", { tick: 100 + 320 * 30, reason: "lost", players: [] }, "2026-11-15T20:10:30.000Z"),
    ]);
    return s.games.g1!;
  }

  test("an ended game's length is game time, the clock the replay uses", () => {
    const g = lostGame();
    expect(elapsedSeconds(g, 0)).toBe(630);
    expect(gameSeconds(g, 0)).toBe(320);
    const frames: Frame[] = [{ game_id: "g1", tick: 101, players: [] }, { game_id: "g1", tick: 100 + 320 * 30, players: [] }];
    const tl = new ReplayTimeline(frames, []);
    expect(Math.abs(tl.durationSeconds - gameSeconds(g, 0))).toBeLessThan(1);
  });

  test("a detail snapshot supplies both ticks", () => {
    const s = initialState();
    applyGameSnapshot(s, {
      game_id: "g2", map: "m", mode: "multiplayer", started_at: "2026-11-15T20:00:00Z", ended_at: "2026-11-15T20:10:30Z",
      players: [], team_play: false, tick: 50, last_tick: 50 + 300, end_reason: "lost",
    } as never);
    expect(gameSeconds(s.games.g2!, 0)).toBe(10);
  });

  test("without ticks, or while live, it is the wall clock", () => {
    const s = initialState();
    applyEvent(s, ev("GameStarted", "game-g3", { game_id: "g3", map: "m", mode: "multiplayer", team_play: false, players: [] }, "2026-11-15T20:00:00.000Z"));
    const g = s.games.g3!;
    expect(gameSeconds(g, Date.parse("2026-11-15T20:01:00.000Z"))).toBe(60);
  });
});

describe("players linked to an account", () => {
  test("show the gamertag and keep the in-game name", () => {
    const s = initialState();
    applyAccountsSnapshot(s, [{ xuid: AFR, gamertag: "afr", created_at: "", online: true }]);
    applyEvent(s, ev("GameStarted", "game-g1", {
      game_id: "g1", map: "m", mode: "multiplayer", team_play: false,
      players: [{ index: 0, name: "Halo0001", xuid: AFR, team: 0, local: true }, { index: 1, name: "Guest", team: 1 }],
    }));
    const [a, b] = standings(s.games.g1!).sort((x, y) => x.index - y.index);
    expect(a!.name).toBe("afr");
    expect(a!.game_name).toBe("Halo0001");
    expect(a!.xuid).toBe(AFR);
    expect(b!.name).toBe("Guest");
    expect(b!.game_name).toBeNull();
  });

  test("an account learned after the game renames its player", () => {
    const s = initialState();
    applyEvents(s, [
      ev("GameStarted", "game-g1", { game_id: "g1", map: "m", mode: "multiplayer", team_play: false, players: [{ index: 0, name: "Halo0001", xuid: AFR, team: 0 }] }),
      ev("AccountCreated", `account-${AFR}`, { xuid: AFR, gamertag: "afr" }),
    ]);
    expect(s.games.g1!.players[0]!.name).toBe("afr");
    expect(s.games.g1!.players[0]!.game_name).toBe("Halo0001");
  });

  test("a matching profile name is not repeated", () => {
    const s = initialState();
    applyAccountsSnapshot(s, [{ xuid: AFR, gamertag: "afr", created_at: "", online: true }]);
    applyEvent(s, ev("GameStarted", "game-g1", { game_id: "g1", map: "m", mode: "multiplayer", team_play: false, players: [{ index: 0, name: "afr", xuid: AFR, team: 0 }] }));
    expect(s.games.g1!.players[0]!.game_name).toBeNull();
  });
});

describe("game record signatures", () => {
  test("a game loaded from a snapshot gains its events' positions from the replay read", () => {
    const s = initialState();
    applyGameSnapshot(s, { game_id: "g1", map: "m", mode: "multiplayer", started_at: "x", ended_at: "y", players: [], team_play: false } as never);
    expect(s.games.g1!.positions).toEqual([]);
    const evs = [
      ev("GameStarted", "game-g1", {}), ev("GameFrame", "game-g1", {}), ev("GameEnded", "game-g1", {}),
      ev("SignedIn", `presence-${AFR}`, {}),
    ];
    expect(addGamePositions(s, "g1", evs)).toEqual([1, 2, 3]);
    expect(addGamePositions(s, "g1", evs)).toEqual([]);
    expect(s.games.g1!.positions).toEqual([1, 2, 3]);
  });

  test("the summary never reports a bare zero", () => {
    expect(summaryLine(countResults([], {}))).toBe("No events loaded to check yet");
    expect(summaryLine(countResults([1, 2], {}))).toBe("Checking 2 signed events…");
    const r = { 1: { position: 1, status: "verified" as const }, 2: { position: 2, status: "verified" as const } };
    expect(summaryLine(countResults([1, 2, 3], r))).toBe("2 signed events verified · 1 checking");
    expect(summaryLine(countResults([1], { 1: { position: 1, status: "bad" } }))).toBe("1 event failed signature checks");
  });
});
