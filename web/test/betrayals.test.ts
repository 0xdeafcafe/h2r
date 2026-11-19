// A betrayal is a teammate killed in a game with teams. Hosts before the fix
// sent every free-for-all kill as one ("afr betrayed Clanker 3"): the Field
// Report and the carnage report read them as kills.
import { beforeEach, describe, expect, test } from "bun:test";
import { applyEvents, applyGameSnapshot, initialState, isBetrayal } from "../src/state/reducer";
import { ev, resetPos } from "./helpers";

beforeEach(resetPos);

const players = [
  { index: 0, name: "afr", team: 0, local: true },
  { index: 1, name: "Clanker 1", team: 0, local: true, bot: true },
  { index: 2, name: "Clanker 2", team: 1, local: true, bot: true },
];

function game(id: string, mode: string, teamPlay: boolean, kills: Array<[number, number, string]>) {
  const s = initialState();
  applyEvents(s, [
    ev("GameStarted", `game-${id}`, { game_id: id, map: "lockout", mode, team_play: teamPlay, players }),
    ...kills.map(([victim, killer, kind]) => ev("PlayerKilled", `game-${id}`, { tick: 30, victim, killer, kind })),
  ]);
  return s.games[id]!;
}

describe("betrayals", () => {
  test("free-for-all kills recorded as betrayals read as kills", () => {
    const g = game("ffa", "multiplayer", false, [[1, 0, "betrayal"], [2, 0, "betrayal"], [0, 1, "betrayal"], [0, 0, "suicide"]]);
    expect(g.kills.map((k) => k.kind)).toEqual(["kill", "kill", "kill", "suicide"]);
  });

  test("in a team game a teammate killed is a betrayal and an enemy a kill", () => {
    const g = game("team", "multiplayer", true, [[1, 0, "kill"], [2, 0, "betrayal"], [0, 0, "suicide"]]);
    expect(g.kills.map((k) => k.kind)).toEqual(["betrayal", "kill", "suicide"]);
  });

  test("a campaign's players are all on one side", () => {
    const s = initialState();
    applyEvents(s, [ev("GameStarted", "game-c", { game_id: "c", map: "03a_oldmombasa", mode: "campaign", team_play: false, players })]);
    expect(isBetrayal(s.games.c!, 0, 1)).toBe(true);
    expect(isBetrayal(s.games.c!, 0, 0)).toBe(false);
    expect(isBetrayal(s.games.c!, null, 1)).toBe(false);
  });

  test("a game detail's kills are settled too", () => {
    const s = initialState();
    applyGameSnapshot(s, {
      game_id: "d", map: "lockout", mode: "multiplayer", started_at: "2026-11-19T20:00:00Z",
      team_play: false, players,
      kills: [{ tick: 30, victim: 2, killer: 1, kind: "betrayal", position: 7, at: "2026-11-19T20:00:01Z" }],
    } as never);
    expect(s.games.d!.kills.map((k) => k.kind)).toEqual(["kill"]);
  });
});
