import { describe, expect, test } from "bun:test";
import {
  DEFAULT_PLAYLISTS, boardStats, changeText, defaultPlaylist, levelTier, parseBoard, parseClankerMarks, parseHistory, parseRanks,
  parseStanding, playlistMenu, playlistName, progress, threshold, toNext, winRate, yourRank, type Standing,
} from "../src/lib/ranks";

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

  test("a row the server marks bot keeps the mark, others have none", () => {
    const rows = parseBoard({ items: [{ xuid: "a", gamertag: "Person", ...st() }, { xuid: "b", gamertag: "Clanker", bot: true, ...st() }, { xuid: "c", gamertag: "Odd", bot: "yes", ...st() }] });
    expect(rows.map((r) => r.bot)).toEqual([undefined, true, undefined]);
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

describe("the playlist menu", () => {
  const hopper = (over: Record<string, unknown> = {}) => ({
    index: 1, type: 4, unused: false, status: 1, record_count: 1, name: "p1",
    title: "Team Slayer", description: "Four against four.", ranges: [2, 3, 4],
    records: [{ id: 1, string: "", flags: 0, dwords: [10, 0], variant: { name: "Team Slayer", engine: 2, team: true } }],
    ...over,
  });

  test("without a published file it is the built-in lineup, three groups in hopper order", () => {
    const menu = playlistMenu(null, null);
    expect(menu.map((p) => p.name)).toEqual(Object.values(DEFAULT_PLAYLISTS));
    expect(menu.filter((p) => p.group === "ranked").map((p) => p.name))
      .toEqual(["Rumble Pit", "Team Slayer", "Team Skirmish", "Team Snipers", "Big Team Battle", "Team Objective"]);
    expect(menu.filter((p) => p.group === "unranked").map((p) => p.name)).toEqual(["Free for All", "Unranked Teams"]);
    expect(menu.filter((p) => p.group === "clanker").map((p) => p.name)).toEqual(["Clanker Slayer", "Clanker Teams", "Clanker Doubles"]);
    expect(menu[1]).toMatchObject({ id: "1", size: "2 teams of 3–4", games: ["Team Slayer"] });
    expect(menu[0]).toMatchObject({ id: "0", size: "4–8 players, free for all", games: ["Rumble Slayer", "Oddball", "King of the Hill"] });
    expect(menu[2]).toMatchObject({ size: "2 vs 2" });
  });

  test("a published file's hoppers replace the lineup; unused slots and junk drop out", () => {
    const doc = { hoppers: [hopper({ index: 3, type: 2, title: "Lone Wolves", ranges: [4, 8, 0] }), hopper({ index: 4, type: 0, unused: true }), { junk: true }, hopper({ index: 5, records: "x" })] };
    const menu = playlistMenu(doc, null);
    expect(menu).toHaveLength(2);
    expect(menu[0]).toMatchObject({ id: "3", name: "Lone Wolves", group: "ranked", size: "4–8 players, free for all", games: ["Team Slayer"] });
    expect(menu[1]).toMatchObject({ id: "5", games: [] });
  });

  test("a hopper with no title is named by its index, and its entries' game types come once each in order", () => {
    const doc = {
      hoppers: [hopper({
        index: 2, title: "", records: [
          { variant: { name: "Team Slayer" } }, { variant: { name: "Classic CTF" } }, { variant: { name: "Team Slayer" } }, { variant: null }, {},
        ],
      })],
    };
    const menu = playlistMenu(doc, null);
    expect(menu[0]!.name).toBe("Playlist 2");
    expect(menu[0]!.games).toEqual(["Team Slayer", "Classic CTF"]);
  });

  test("clanker marks move a hopper to the Clankermatch group whatever its type", () => {
    const body = { hoppers: [{ hopper_id: 1, playlist_id: 2, players: 8, teams: 2 }, { hopper_id: "x" }, 3] };
    const marks = parseClankerMarks(body);
    expect(marks.has(1)).toBe(true);
    expect(marks.has(9)).toBe(false);
    const menu = playlistMenu(null, body);
    expect(menu.find((p) => p.id === "1")!.group).toBe("clanker");
    expect(menu.filter((p) => p.group === "clanker").map((p) => p.id)).toEqual(["1", "8", "9", "10"]);
    expect(playlistMenu(null, { hoppers: "junk" }).filter((p) => p.group === "clanker")).toHaveLength(3);
  });

  test("the type params that don't read leave the size off", () => {
    expect(playlistMenu({ hoppers: [hopper({ ranges: [0, 0, 0] })] }, null)[0]!.size).toBeNull();
    expect(playlistMenu({ hoppers: [hopper({ type: 1, ranges: [8, 2, 0] })] }, null)[0]!.size).toBeNull();
    expect(playlistMenu({ hoppers: [hopper({ type: 1, ranges: [8, 8, 0] })] }, null)[0]!.size).toBe("8 players, free for all");
  });

  test("bare /ranks opens the ranked board with the most players, the lowest hopper on a tie", () => {
    const menu = playlistMenu(null, null);
    expect(defaultPlaylist(menu, {})).toBe("0");
    expect(defaultPlaylist(null, { "1": 5 })).toBe("1"); // no menu read yet: the guard pick
    expect(defaultPlaylist(menu, { "4": 9, "1": 12, "8": 99 })).toBe("1");
    expect(defaultPlaylist(menu, { "4": 9, "6": 99 })).toBe("4");
    expect(defaultPlaylist(menu, { "9": 99 })).toBe("0"); // clanker boards never win the landing
  });
});

describe("the board's sums", () => {
  const row = (xuid: string, over: Partial<Standing> = {}) => ({ xuid, gamertag: xuid, ...st(over) });

  test("players, player games and the highest level anyone reached", () => {
    const stats = boardStats([
      row("a", { level: 12, experience: 1500, highest: 14, games: 30, wins: 17 }),
      row("b", { level: 9, experience: 900, highest: 9, games: 10, wins: 2 }),
    ]);
    expect(stats).toEqual({ players: 2, games: 40, highest: 14 });
    expect(boardStats([])).toEqual({ players: 0, games: 0, highest: 1 });
  });

  test("your rank is your row's place, 1-based, and null off the board or signed out", () => {
    const rows = [row("a"), row("0009000000000001"), row("c")];
    expect(yourRank(rows, "0009000000000001")).toBe(2);
    expect(yourRank(rows, "c")).toBe(3);
    expect(yourRank(rows, "zz")).toBeNull();
    expect(yourRank(rows, null)).toBeNull();
    expect(yourRank(rows, undefined)).toBeNull();
  });

  test("XP to the next level, never negative, null at the top", () => {
    expect(toNext(st({ level: 2, experience: 100 }))).toBe(150); // threshold(3) − 100
    expect(toNext(st({ level: 10, experience: 2600 }))).toBe(650); // held below by the drop rule
    expect(toNext(st({ level: 2, experience: 175 }))).toBe(75);
    expect(toNext(st({ level: 50, experience: 63700 }))).toBeNull();
  });

  test("the badge's colour grades step every ten levels, 50 apart", () => {
    expect([1, 9].map(levelTier)).toEqual([0, 0]);
    expect([10, 19].map(levelTier)).toEqual([1, 1]);
    expect([20, 29].map(levelTier)).toEqual([2, 2]);
    expect([30, 39].map(levelTier)).toEqual([3, 3]);
    expect([40, 49].map(levelTier)).toEqual([4, 4]);
    expect(levelTier(50)).toBe(5);
  });
});
