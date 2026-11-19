import { describe, expect, test } from "bun:test";
import { DEFAULT_PLAYLISTS, changeText, parseBoard, parseHistory, parseRanks, parseStanding, playlistName, progress, threshold, winRate, type Standing } from "../src/lib/ranks";

const st = (over: Partial<Standing> = {}): Standing => ({ level: 1, experience: 0, highest: 1, games: 0, wins: 0, ...over });

describe("rank reads", () => {
  test("standings parse and anything off the contract is dropped", () => {
    expect(parseStanding({ level: 12, experience: 4000, highest: 14, games: 30, wins: 17 })).toEqual(st({ level: 12, experience: 4000, highest: 14, games: 30, wins: 17 }));
    for (const bad of [null, 3, {}, { ...st(), level: 0 }, { ...st(), level: 51 }, { ...st(), experience: -1 }, { ...st(), games: 1.5 }, { ...st(), wins: "2" }]) {
      expect(parseStanding(bad)).toBeNull();
    }
  });

  test("a player's ranks come best level first", () => {
    const body = { xuid: "0009000000000001", ranks: { "1": st({ level: 3, experience: 300 }), "4": st({ level: 9, experience: 2000 }), "6": { level: "x" } } };
    expect(parseRanks(body).map(([id]) => id)).toEqual(["4", "1"]);
    expect(parseRanks(null)).toEqual([]);
  });

  test("history rows keep their game and both sides of the change", () => {
    const body = {
      standing: st({ level: 2, experience: 189 }),
      history: [
        { game_id: "g2", at: "2026-11-18T10:00:00Z", position: 9, before: st({ level: 2, experience: 100 }), after: st({ level: 2, experience: 189 }) },
        { game_id: "g1", at: "2026-11-18T09:00:00Z", position: 5, before: st(), after: st({ level: 2, experience: 100, highest: 2 }) },
        { game_id: 7, at: "x", before: st(), after: st() },
      ],
    };
    const h = parseHistory(body);
    expect(h.standing?.experience).toBe(189);
    expect(h.history.map((e) => e.gameId)).toEqual(["g2", "g1"]);
    expect(changeText(h.history[0]!)).toBe("+89 XP, level 2");
    expect(changeText(h.history[1]!)).toBe("+100 XP, level 1 → 2");
    expect(changeText({ gameId: "g", at: "", before: st({ level: 10, experience: 2400 }), after: st({ level: 9, experience: 2200 }) })).toBe("−200 XP, level 10 → 9");
  });

  test("the leaderboard keeps the server's order and needs a gamertag", () => {
    const rows = parseBoard({ items: [{ xuid: "a", gamertag: "Host", ...st({ level: 2 }) }, { xuid: "b", ...st() }, { xuid: "c", gamertag: "Guest", ...st() }] });
    expect(rows.map((r) => r.gamertag)).toEqual(["Host", "Guest"]);
  });
});

describe("rank sums", () => {
  test("thresholds match the server's table", () => {
    expect(threshold(1)).toBe(0);
    expect(threshold(2)).toBe(100);
    expect(threshold(10)).toBe(2700);
    expect(threshold(50)).toBe(63700);
  });

  test("progress runs from one threshold to the next, full at the top", () => {
    expect(progress(st({ level: 2, experience: 100 }))).toBe(0);
    expect(progress(st({ level: 2, experience: 175 }))).toBe(0.5);
    expect(progress(st({ level: 50, experience: 63700 }))).toBe(1);
    // A player held up by the drop rule sits below their threshold: 0, not negative.
    expect(progress(st({ level: 10, experience: 2600 }))).toBe(0);
  });

  test("playlist names and win rates", () => {
    expect(playlistName("1")).toBe("Team Slayer");
    expect(playlistName("12")).toBe("Playlist 12");
    expect(playlistName("3", { "3": "Snipers Only" })).toBe("Snipers Only");
    expect(Object.keys(DEFAULT_PLAYLISTS)).toHaveLength(11);
    expect(playlistName("8")).toBe("Clanker Slayer");
    expect(winRate(st({ games: 4, wins: 3 }))).toBe("75%");
    expect(winRate(st())).toBe("–");
  });
});
