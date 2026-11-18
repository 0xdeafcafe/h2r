import { describe, expect, test } from "bun:test";
import type { WireEvent } from "../src/api/types";
import { eventWindowStart, readEventWindow } from "../src/lib/eventLog";
import { STATS_EVENT_KINDS, isStatsEvent } from "../src/stats/eventKinds";

const event = (position: number, type: string, stream: string): WireEvent => ({
  position, type, stream, version: 1, at: "", data: {},
});

describe("stats event log", () => {
  test("allows the game record and rank changes", () => {
    const gameKinds = ["GameStarted", "PlayerJoined", "PlayerLeft", "PlayerKilled", "ScoreChanged", "GameEnded"];
    expect(Object.keys(STATS_EVENT_KINDS)).toEqual([...gameKinds, "RankChanged"]);
    for (const kind of gameKinds) expect(isStatsEvent(event(1, kind, "game-x"))).toBe(true);
    expect(isStatsEvent(event(1, "RankChanged", "rank-x"))).toBe(true);
  });

  test("excludes raw telemetry, Live events, and joins or leaves from session streams", () => {
    for (const [type, stream] of [
      ["AccountCreated", "account-x"], ["FriendRequested", "account-x"], ["GameInviteSent", "mailbox-x"],
      ["PlayerReported", "report-x"], ["SignedIn", "presence-x"], ["TeamCreated", "team-x"],
      ["PlayerJoined", "session-x"], ["PlayerLeft", "session-x"], ["GameFrame", "game-x"],
      ["RankChanged", "game-x"], ["FutureEvent", "game-x"], ["constructor", "game-x"],
    ] as const) expect(isStatsEvent(event(1, type, stream))).toBe(false);
  });
});

describe("raw event log windows", () => {
  test("read every kind across the position window, including frames and service events", async () => {
    const events = [
      event(102, "GameFrame", "game-x"), event(225, "GameInviteSent", "mailbox-x"),
      event(300, "PlayerLeft", "session-x"), event(301, "RankChanged", "rank-x"),
    ];
    const rows = await readEventWindow({
      events: async (query) => {
        expect(query).toEqual({ from: 100, limit: 200 });
        return { items: events, next: "301", position: 500 };
      },
    }, 300);
    expect(rows).toEqual(events.slice(0, 3));
    expect(eventWindowStart(300)).toBe(100);
    expect(eventWindowStart(100)).toBe(0);
  });
});
