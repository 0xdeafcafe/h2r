// Clanker bots in a game (docs/game/bots.md §6): players in the game record
// and the live view, with their own names and a BOT mark, never linked to a
// player page, and game-only: a kill of a bot or a death to one stays out of
// the human's career. The scoreboard and carnage report are compiled for the
// server as in stats-home-render.test.ts and rendered to HTML.

import { beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { plugin } from "bun";
import { applyAccountsSnapshot, applyEvents, initialState, standings, type GameState, type LiveState } from "../src/state/reducer";
import { gameHistory, playedWith, serviceRecord } from "../src/state/derive";
import { rosterFor } from "../src/lib/game";
import { rosterLabel } from "../src/map/render";
import { StatsFold } from "../mock/stats";
import { ev, resetPos } from "./helpers";

const ROUTER = `${import.meta.dir}/../node_modules/@solidjs/router/dist/index.jsx`;
const HUMAN = "0009000000000042";
// A server-rostered bot's xuid (the 0x000B range).
const ROSTERED = "000b000000000007";

beforeEach(resetPos);

/** Halo0001 (an account) with two Clankers: one of the host's own (no xuid)
 *  and one from a server roster (an xuid, still a bot). The human kills the
 *  first Clanker twice, kills the rostered one once, and dies to it once. */
function botGame(): LiveState {
  const s = initialState();
  applyAccountsSnapshot(s, [{ xuid: HUMAN, gamertag: "Halo0001" }]);
  applyEvents(s, [
    ev("GameStarted", "game-g1", {
      game_id: "g1", map: "lockout", mode: "multiplayer", kind: "custom", variant: "Slayer", team_play: false, tick: 0,
      players: [
        { index: 0, name: "Halo0001", xuid: HUMAN, team: 0, local: true },
        { index: 1, name: "Clanker 1", team: 1, local: true, bot: true },
      ],
    }),
    ev("PlayerJoined", "game-g1", { player: { index: 2, name: "Rusty Lugnut", xuid: ROSTERED, team: 2, bot: true } }),
    ev("PlayerKilled", "game-g1", { tick: 30, victim: 1, killer: 0, kind: "kill", weapon: "battle_rifle" }),
    ev("PlayerKilled", "game-g1", { tick: 60, victim: 1, killer: 0, kind: "kill", weapon: "battle_rifle" }),
    ev("PlayerKilled", "game-g1", { tick: 90, victim: 2, killer: 0, kind: "kill", weapon: "smg" }),
    ev("PlayerKilled", "game-g1", { tick: 120, victim: 0, killer: 2, kind: "kill", weapon: "smg" }),
    ev("GameEnded", "game-g1", {
      tick: 900, reason: "finished", winner: { player: 0 },
      players: [
        { index: 0, score: 3, kills: 3, deaths: 1, assists: 0 },
        { index: 1, score: 0, kills: 0, deaths: 2, assists: 0 },
        { index: 2, score: 1, kills: 1, deaths: 1, assists: 0 },
      ],
    }),
  ]);
  return s;
}

describe("bots in the game record", () => {
  test("bots are players with their own names, marked as bots", () => {
    const g = botGame().games.g1!;
    const rows = standings(g);
    expect(rows.map((r) => [r.name, r.bot])).toEqual([["Halo0001", false], ["Rusty Lugnut", true], ["Clanker 1", true]]);
    const clanker = rows.find((r) => r.index === 1)!;
    expect(clanker.xuid).toBeNull();
    expect(clanker.score).toMatchObject({ kills: 0, deaths: 2 });
    expect(rows.find((r) => r.index === 2)!.score).toMatchObject({ kills: 1, deaths: 1 });
  });

  test("a bot is never shown by an account's gamertag", () => {
    const s = botGame();
    applyAccountsSnapshot(s, [{ xuid: HUMAN, gamertag: "Halo0001" }, { xuid: ROSTERED, gamertag: "Someone" }]);
    expect(s.games.g1!.players[2]!.name).toBe("Rusty Lugnut");
  });

  test("the map and viewer roster label bots", () => {
    const r = rosterFor(botGame().games.g1!);
    expect(r.get(1)).toMatchObject({ name: "Clanker 1", bot: true });
    expect(r.get(0)!.bot).toBeUndefined();
    expect(rosterLabel(r.get(1)!)).toBe("Clanker 1 · BOT");
    expect(rosterLabel(r.get(0)!)).toBe("Halo0001");
  });
});

describe("bots are game-only in a career", () => {
  test("the game history keeps the scoreboard; the service record leaves the bots out", () => {
    const s = botGame();
    const [row] = gameHistory(s, HUMAN);
    expect(row).toMatchObject({ kills: 3, deaths: 1, won: true });
    const rec = serviceRecord(s, HUMAN);
    expect(rec.games).toBe(1);
    expect(rec.wins).toBe(1);
    expect(rec.kills).toBe(0);
    expect(rec.deaths).toBe(0);
    expect(rec.bestSpree).toBe(0);
    expect(rec.killsSeen).toBe(0);
    expect(rec.weapons).toEqual([]);
  });

  test("a bot is nobody's 'played with'", () => {
    expect(playedWith(botGame(), HUMAN)).toEqual([]);
  });

  test("the mock's stats summary counts bots in the totals only, as the server does", () => {
    const fold = new StatsFold();
    for (const e of [
      ev("GameStarted", "game-g2", {
        game_id: "g2", map: "lockout", mode: "multiplayer", kind: "custom", team_play: false, tick: 0,
        players: [{ index: 0, name: "Halo0001", xuid: HUMAN, team: 0 }, { index: 1, name: "Clanker 1", team: 1, bot: true }],
      }, new Date().toISOString()),
      ev("PlayerKilled", "game-g2", { tick: 30, victim: 1, killer: 0, kind: "kill", weapon: "battle_rifle" }, new Date().toISOString()),
      ev("PlayerKilled", "game-g2", { tick: 60, victim: 0, killer: 1, kind: "kill", weapon: "smg" }, new Date().toISOString()),
    ]) fold.apply(e);
    const sum = fold.summary("all", Date.now(), 1) as { totals: { kills: number }; players: Array<{ xuid: string; kills: number; deaths: number }> };
    expect(sum.totals.kills).toBe(2);
    expect(sum.players.map((p) => [p.xuid, p.kills, p.deaths])).toEqual([[HUMAN, 0, 0]]);
  });
});

let scoreboard: (g: GameState) => string;
let carnage: (g: GameState) => string;

beforeAll(async () => {
  const babel = await import("@babel/core");
  const solid = (await import("babel-preset-solid")).default;
  plugin({
    name: "solid-ssr",
    setup(build) {
      build.onLoad({ filter: /(\/src\/.*\.tsx|\/@solidjs\/router\/dist\/.*\.jsx)$/ }, async ({ path }) => {
        const src = await Bun.file(path).text();
        const out = await babel.transformAsync(src, {
          filename: path, babelrc: false, configFile: false,
          parserOpts: { plugins: ["jsx", "typescript"] },
          presets: [[solid, { generate: "ssr", hydratable: false }]],
        });
        return { contents: out!.code!.replaceAll('"@solidjs/router"', JSON.stringify(ROUTER)), loader: "ts" };
      });
    },
  });
  const { renderToString } = await import("solid-js/web");
  const { createComponent } = await import("solid-js");
  const { Router, Route } = await import(ROUTER);
  const { default: Scoreboard } = await import("../src/components/Scoreboard");
  const { default: Carnage } = await import("../src/stats/Carnage");
  const inRouter = (view: () => unknown) =>
    renderToString(() => createComponent(Router, {
      url: "/",
      get children() { return createComponent(Route, { path: "/", component: view }); },
    }));
  scoreboard = (g) => inRouter(() => createComponent(Scoreboard, { game: g }));
  carnage = (g) => inRouter(() => createComponent(Carnage, { game: g }));
});

/** Each name cell (from `class="<cls>"` to the end of its table cell), by
 *  the name shown. */
function cells(html: string, cls: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const part of html.split(`class="${cls}"`).slice(1)) {
    const cell = part.slice(0, part.indexOf("</td>"));
    const name = /(Halo0001|Clanker 1|Rusty Lugnut)/.exec(cell)?.[1];
    if (name) out.set(name, cell);
  }
  return out;
}

describe("the BOT mark and no player link", () => {
  test("the live scoreboard marks bots and links only the human", () => {
    const html = scoreboard(botGame().games.g1!);
    const c = cells(html, "score-name");
    expect(c.size).toBe(3);
    expect(c.get("Halo0001")).toContain('href="/games/halo2/players/Halo0001"');
    expect(c.get("Halo0001")).not.toContain("bot-tag");
    for (const bot of ["Clanker 1", "Rusty Lugnut"]) {
      expect(c.get(bot)).toContain('class="bot-tag"');
      expect(c.get(bot)).toContain(">BOT<");
      expect(c.get(bot)).not.toContain("href=");
    }
  });

  test("the carnage report marks bots and links only the human", () => {
    const html = carnage(botGame().games.g1!);
    const c = cells(html, "who");
    expect(c.get("Halo0001")).toContain("/players/Halo0001");
    for (const bot of ["Clanker 1", "Rusty Lugnut"]) {
      expect(c.get(bot)).toContain('class="bot-tag"');
      expect(c.get(bot)).not.toContain("/players/");
    }
  });
});
