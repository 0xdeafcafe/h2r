// The stats home's numbers (src/lib/statsHome.ts), the nav and crumbs that
// lead to it (src/stats/modes.ts), and the mock's summary (mock/stats.ts),
// which follows the server's counting rules.

import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import {
  DAYS, KD_MIN_GAMES, clanStandings, emptyTotals, heatmap, hourName, hoursOf, kd, leaderboards, level, ofTheWeek, parseSummary,
  peak, shares, statsWindow, utcSlot, type PlayerRow,
} from "../src/lib/statsHome";
import { DASHBOARD_EXIT, HOME_CRUMB, STATS_HOME, statsNav } from "../src/stats/modes";
import { StatsFold, archive } from "../mock/stats";
import { STATS_NAME } from "../src/brand";
import { STATS_ROOT } from "../src/routes";

const P = (xuid: string, o: Partial<PlayerRow> = {}): PlayerRow => ({
  xuid, name: xuid.toUpperCase(), games: 0, multiplayer: 0, campaign: 0, wins: 0, kills: 0, deaths: 0, betrayals: 0,
  best_spree: 0, seconds: 0, legendary: 0, ...o,
});

describe("nav and crumbs", () => {
  test("the home tab comes first and the dashboard is not a tab", () => {
    const nav = statsNav(null, false);
    expect(nav[0]).toEqual({ label: STATS_NAME, href: STATS_HOME, end: true });
    expect(nav.some((n) => n.href === "/" || /dashboard/i.test(n.label))).toBe(false);
    expect(nav.map((n) => n.label)).toEqual([STATS_NAME, "My Stats", "Games", "Players", "Clans", "Mods", "Event Log", "Server"]);
  });

  test("my stats follows the signed-in account, and admins get their tab", () => {
    const nav = statsNav({ gamertag: "Sneaky Pete" }, true);
    expect(nav.find((n) => n.label === "My Stats")!.href).toBe("/games/halo2/players/Sneaky%20Pete");
    expect(statsNav(null, false).find((n) => n.label === "My Stats")!.href).toBe("/dash/signup");
    expect(nav.find((n) => n.label === "Halo Admin")!.href).toBe("/games/halo2/admin");
  });

  test("the way out goes to the dashboard and says so", () => {
    expect(DASHBOARD_EXIT.href).toBe("/");
    expect(DASHBOARD_EXIT.label).toBe("‹ Live Dashboard");
    expect(HOME_CRUMB).toEqual(["Home", "/games/halo2"]);
  });

  test("every stats page's crumbs start at the stats home", () => {
    const dirs = ["src/stats", "src/admin"];
    let seen = 0;
    for (const d of dirs) {
      for (const f of readdirSync(`${import.meta.dir}/../${d}`).filter((f) => f.endsWith(".tsx"))) {
        const src = readFileSync(`${import.meta.dir}/../${d}/${f}`, "utf8");
        for (const m of src.matchAll(/<Crumbs trail=\{\[\[([^\]]*)\]/g)) {
          seen++;
          expect([`"Home", STATS_ROOT`, `"Home"`]).toContain(m[1]!);
        }
      }
    }
    expect(seen).toBeGreaterThan(8);
  });

  test("the routes put the home at /games/halo2 and the games spotlight at /games/halo2/games", () => {
    const src = readFileSync(`${import.meta.dir}/../src/index.tsx`, "utf8");
    expect(src).toContain(`<Route path="/" component={Home} />`);
    expect(src).toContain(`<Route path="/games" component={Games} />`);
    expect(src).toContain(`<Route path={STATS_ROOT} component={StatsLayout}>`);
    expect(STATS_ROOT).toBe("/games/halo2");
  });
});

describe("reading a summary", () => {
  test("missing and malformed fields read as nothing", () => {
    const s = parseSummary({ window: "nope", totals: { games: 3, kills: -1, deaths: "x" }, players: [{ name: "no xuid" }, { xuid: "a", kills: 4 }], activity: [{ hour: "bad" }, { hour: "2026-11-16T10:00:00Z", games: 2 }] });
    expect(s.window).toBe("7d");
    expect(s.totals).toEqual({ ...emptyTotals(), games: 3 });
    expect(s.players).toEqual([P("a", { name: "", kills: 4 })]);
    expect(s.activity).toEqual([{ hour: "2026-11-16T10:00:00Z", games: 2, kills: 0 }]);
    expect(parseSummary(null).maps).toEqual([]);
  });

  test("windows", () => {
    expect(["today", "7d", "30d", "all"].map(statsWindow)).toEqual(["today", "7d", "30d", "all"]);
    expect(statsWindow(undefined)).toBe("7d");
    expect(statsWindow("1y")).toBe("7d");
  });
});

describe("boards", () => {
  const players = [
    P("a", { multiplayer: 10, games: 10, kills: 100, deaths: 50, wins: 6, best_spree: 9, seconds: 7200 }),
    P("b", { multiplayer: 2, games: 2, kills: 30, deaths: 3, wins: 2, best_spree: 12 }),
    P("c", { multiplayer: 5, games: 6, campaign: 1, kills: 60, deaths: 20, wins: 6, legendary: 2, seconds: 1800 }),
    P("d", { campaign: 3, games: 3 }),
  ];
  const by = (id: string) => leaderboards(players).find((b) => b.id === id)!;

  test("most kills, and nobody with none", () => {
    expect(by("kills").rows.map((r) => [r.xuid, r.value])).toEqual([["a", 100], ["c", 60], ["b", 30]]);
  });

  test("best K/D needs the minimum multiplayer games", () => {
    expect(KD_MIN_GAMES).toBe(3);
    expect(by("kd").rows.map((r) => [r.xuid, r.text])).toEqual([["c", "3.00"], ["a", "2.00"]]);
    expect(leaderboards(players, 1).find((b) => b.id === "kd")!.rows[0]!.xuid).toBe("b");
  });

  test("ties go to more games, then the name", () => {
    expect(by("wins").rows.map((r) => r.xuid)).toEqual(["a", "c", "b"]);
  });

  test("sprees, legendary and time", () => {
    expect(by("spree").rows[0]).toMatchObject({ xuid: "b", value: 12 });
    expect(by("legendary").rows).toEqual([{ xuid: "c", name: "C", value: 2, text: "2" }]);
    expect(by("time").rows.map((r) => r.text)).toEqual(["2 h", "0.5 h"]);
  });

  test("kd and hours", () => {
    expect(kd(10, 0)).toBe(10);
    expect(kd(10, 4)).toBe(2.5);
    expect(hoursOf(0)).toBe(0);
    expect(hoursOf(5400)).toBe(1);
    expect(hoursOf(900)).toBe(0.3);
  });
});

describe("of the week", () => {
  test("the top weapon and the top multiplayer map", () => {
    const w = ofTheWeek({
      weapons: [{ weapon: "rifle", kills: 9 }, { weapon: "smg", kills: 3 }],
      maps: [{ map: "outskirts", campaign: true, games: 9, kills: 0 }, { map: "lockout", campaign: false, games: 4, kills: 80 }],
    });
    expect(w.weapon!.weapon).toBe("rifle");
    expect(w.map!.map).toBe("lockout");
  });

  test("a campaign-only week still has a map; an empty one has neither", () => {
    expect(ofTheWeek({ weapons: [], maps: [{ map: "outskirts", campaign: true, games: 2, kills: 0 }] }).map!.map).toBe("outskirts");
    expect(ofTheWeek({ weapons: [], maps: [] })).toEqual({ weapon: null, map: null });
  });

  test("shares are whole percents of the total", () => {
    expect(shares([3, 1], (x) => x).map((s) => s.pct)).toEqual([75, 25]);
    expect(shares([], (x: number) => x)).toEqual([]);
  });
});

describe("heatmap", () => {
  // 2026-11-16 is a Monday.
  const act = [
    { hour: "2026-11-16T21:00:00.000Z", games: 3, kills: 50 },
    { hour: "2026-11-23T21:00:00.000Z", games: 2, kills: 10 },
    { hour: "2026-11-15T09:00:00.000Z", games: 1, kills: 0 },
  ];

  test("games fold into day of week and hour", () => {
    const h = heatmap(act, utcSlot);
    expect(h.grid).toHaveLength(7);
    expect(h.grid[1]![21]).toBe(5);
    expect(h.grid[0]![9]).toBe(1);
    expect(h.max).toBe(5);
    expect(h.total).toBe(6);
    expect(peak(h.grid)).toEqual({ day: 1, hour: 21, value: 5 });
    expect(DAYS[1]).toBe("Mon");
    expect(heatmap(act, utcSlot, (a) => a.kills).grid[1]![21]).toBe(60);
  });

  test("an empty map has no peak", () => {
    expect(peak(heatmap([], utcSlot).grid)).toBeNull();
  });

  test("levels and hour names", () => {
    expect([0, 1, 2, 3, 5].map((v) => level(v, 5))).toEqual([0, 1, 2, 3, 4]);
    expect(level(3, 0)).toBe(0);
    expect([0, 9, 12, 21].map(hourName)).toEqual(["12 am", "9 am", "12 pm", "9 pm"]);
  });
});

describe("clan standings", () => {
  test("members' multiplayer records, summed; idle clans left out", () => {
    const players = [P("a", { multiplayer: 3, kills: 30, deaths: 10, wins: 2 }), P("b", { multiplayer: 1, kills: 5, deaths: 5 }), P("c", { campaign: 2 })];
    const s = clanStandings([
      { id: "t1", name: "Breakfast Club", members: ["b", "z"] },
      { id: "t2", name: "Sneaky Squad", members: ["a", "b"] },
      { id: "t3", name: "Idle", members: ["c"] },
    ], players);
    expect(s).toEqual([
      { id: "t2", name: "Sneaky Squad", members: 2, active: 2, games: 4, kills: 35, deaths: 15, wins: 2 },
      { id: "t1", name: "Breakfast Club", members: 2, active: 1, games: 1, kills: 5, deaths: 5, wins: 0 },
    ]);
  });
});

describe("the mock's summary", () => {
  const T0 = Date.parse("2025-11-17T12:00:00.000Z");
  const A = "0009000000000001", B = "0009000000000002";
  let pos = 0;
  const ev = (game: string, type: string, at: number, data: unknown) => ({ stream: `game-${game}`, type, at: new Date(at).toISOString(), data, position: ++pos });

  test("counts like the server (crates/live/live-server/src/stats.rs, the first test)", () => {
    const f = new StatsFold();
    const k = (victim: number, killer: number, kind: string, weapon: string, s: number) =>
      f.apply(ev("g1", "PlayerKilled", T0 + s * 1000, { tick: 1, victim, killer, kind, weapon }));
    f.apply(ev("g1", "GameStarted", T0, {
      game_id: "g1", map: "lockout", mode: "multiplayer", kind: "matchmaking", playlist: "Team Slayer", skulls: [4], team_play: true,
      players: [{ index: 0, name: "Ace", xuid: A, team: 0 }, { index: 1, name: "Bee", xuid: B, team: 1 }, { index: 2, name: "Guest", team: 1 }],
    }));
    k(1, 0, "kill", "battle_rifle", 10); k(2, 0, "kill", "battle_rifle", 20); k(1, 0, "kill", "smg", 30); k(0, 1, "kill", "smg", 40);
    k(0, 0, "suicide", "frag_grenade", 50); k(1, 2, "betrayal", "smg", 55); k(1, 0, "kill", "sniper_rifle", 60);
    f.apply(ev("g1", "GameEnded", T0 + 600_000, { tick: 9, reason: "finished", winner: { team: 0 } }));
    const s = f.summary("7d", T0 + 700_000, pos)!;
    expect(s.totals).toMatchObject({ games: 1, multiplayer: 1, matchmaking: 1, kills: 5, deaths: 7, suicides: 1, betrayals: 1, player_seconds: 1800 });
    const a = s.players.find((p) => p.xuid === A)!;
    expect([a.games, a.kills, a.deaths, a.wins, a.best_spree, a.seconds]).toEqual([1, 4, 2, 1, 3, 600]);
    expect(s.players).toHaveLength(2);
    expect(s.weapons[0]).toEqual({ weapon: "battle_rifle", kills: 2 });
    expect(s.playlists).toEqual([{ playlist: "Team Slayer", games: 1 }]);
    expect(s.skulls).toEqual([{ skull: 4, games: 1 }]);
    expect(s.last_24h).toEqual(s.totals);
    expect(f.summary("1y", T0, pos)).toBeNull();
  });

  test("an environmental kill earns credit but does not rank as a weapon", () => {
    const f = new StatsFold();
    f.apply(ev("g", "GameStarted", T0, { mode: "multiplayer", map: "arena", players: [] }));
    for (const weapon of ["fall", "collision", "splatter", "guardians", "teleporter", "explosion", "frag_grenade"]) {
      f.apply(ev("g", "PlayerKilled", T0 + 1000, { victim: 1, killer: 0, kind: "kill", weapon }));
    }
    const s = f.summary("7d", T0 + 2000, pos)!;
    expect(s.totals.kills).toBe(7);
    expect(s.weapons).toEqual([{ weapon: "frag_grenade", kills: 1 }]);
  });

  test("the archive is a month of games, the same every time", () => {
    const accts = Array.from({ length: 8 }, (_, i) => ({ xuid: `00090000000000${String(i).padStart(2, "0")}`, gamertag: `P${i}` }));
    const a = archive(accts, T0), b = archive(accts, T0);
    expect(a).toEqual(b);
    expect(Date.parse(a[0]!.at)).toBeGreaterThanOrEqual(T0 - 30 * 86_400_000);
    expect(Date.parse(a[a.length - 1]!.at)).toBeLessThan(T0);
    const f = new StatsFold();
    for (const e of a) f.apply(e);
    const s = f.summary("30d", T0, 0)!;
    expect(s.totals.games).toBeGreaterThan(100);
    expect(s.totals.campaign).toBeGreaterThan(0);
    expect(s.activity.length).toBeGreaterThan(50);
    expect(leaderboards(parseSummary(s).players).every((bd) => bd.id === "legendary" || bd.rows.length > 0)).toBe(true);
  });
});
