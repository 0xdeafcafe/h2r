// Clanker bots in a game (docs/game/bots.md §6): players in the game record
// and the live view, with their own names and a BOT mark, a bot with a server account (an xuid) links to its page, a host's own
// Clanker does not, and game-only: a kill of a bot or a death to one stays out of
// the human's career. The scoreboard and carnage report are compiled for the
// server as in stats-home-render.test.ts and rendered to HTML.

import { beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { plugin } from "bun";
import { createHash } from "node:crypto";
import clankerKernel from "../src/lib/clanker-kernel.json";
import appearanceCorpus from "../../crates/live-proto/tests/fixtures/clanker-appearance.json";
import { applyAccountsSnapshot, applyEvents, initialState, standings, type AccountState, type GameState, type LiveState } from "../src/state/reducer";
import { gameHistory, playedWith, serviceRecord, type ServiceRecord } from "../src/state/derive";
import { clankerAppearance } from "../src/lib/gameart";
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

test("the browser kernel is generated from its current C source", async () => {
  const source = await Bun.file(new URL("../src/lib/clanker_appearance.c", import.meta.url)).arrayBuffer();
  expect(createHash("sha256").update(new Uint8Array(source)).digest("hex")).toBe(clankerKernel.sourceSha256);
});

test("fresh processes give equivalent gamertags the same profile", () => {
  const module = new URL("../src/lib/gameart.ts", import.meta.url).href;
  const script = `import { clankerAppearance } from ${JSON.stringify(module)};
    console.log(JSON.stringify(clankerAppearance(process.argv[1])));`;
  const lookup = (name: string) => {
    const child = Bun.spawnSync([process.execPath, "--eval", script, name]);
    expect(child.exitCode).toBe(0);
    return JSON.parse(child.stdout.toString());
  };
  expect(lookup("  RUSTY\tlugnut ")).toEqual(lookup("Rusty Lugnut"));
});

test("the web mock and native roster share the 8,192-name appearance vector", () => {
  expect(clankerAppearance("  RUSTY\tlugnut  ")).toEqual(clankerAppearance("Rusty Lugnut"));
  expect(clankerAppearance("ÅNGSTRÖM")).toEqual(clankerAppearance("ångström"));
  let hash = 2166136261;
  for (let i = 0; i < appearanceCorpus.count; i++) {
    const { emblem, colours } = clankerAppearance(`${appearanceCorpus.prefix}${i}`);
    for (const index of [emblem!.fg!, emblem!.bg!, colours!.primary!, colours!.secondary!, colours!.tertiary!, colours!.quaternary!]) {
      hash = Math.imul(hash ^ index, 16777619);
    }
  }
  expect(hash >>> 0).toBe(appearanceCorpus.fingerprint);
});

describe("bots in the game record", () => {
  test("bots are players with their own names, marked as bots", () => {
    const g = botGame().games.g1!;
    const rows = standings(g);
    expect(rows.map((r) => [r.name, r.bot])).toEqual([["Halo0001", false], ["Rusty Lugnut", true], ["Clanker 1", true]]);
    const clanker = rows.find((r) => r.index === 1)!;
    expect(clanker.xuid).toBeNull();
    expect(rows.find((r) => r.xuid === ROSTERED)?.appearance).toEqual({
      emblem: { fg: 1, bg: 30 }, colours: { primary: 10, secondary: 11, tertiary: 13, quaternary: 16 },
    });
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
let strip: (a: AccountState, rec: ServiceRecord) => string;
let playersPage: () => string;

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
  const { PlayerStrip } = await import("../src/stats/Player");
  const { default: Players } = await import("../src/stats/Players");
  strip = (a, rec) => inRouter(() => createComponent(PlayerStrip, { account: a, rec }));
  playersPage = () => inRouter(() => createComponent(Players, {}));
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
  test("the live scoreboard marks bots and links the account-bot, not the account-less Clanker", () => {
    const html = scoreboard(botGame().games.g1!);
    const c = cells(html, "score-name");
    expect(c.size).toBe(3);
    expect(c.get("Halo0001")).toContain('href="/games/halo2/players/Halo0001"');
    expect(c.get("Halo0001")).not.toContain("bot-tag");
    for (const bot of ["Clanker 1", "Rusty Lugnut"]) {
      expect(c.get(bot)).toContain('class="bot-tag"');
      expect(c.get(bot)).toContain(">BOT<");
    }
    // A bot with an xuid is a server bot account and has a page; a host's Clanker has none.
    expect(c.get("Rusty Lugnut")).toContain('href="/games/halo2/players/Rusty%20Lugnut"');
    expect(c.get("Clanker 1")).not.toContain("href=");
  });

  test("the carnage report marks bots and links the account-bot, not the account-less Clanker", () => {
    const html = carnage(botGame().games.g1!);
    const c = cells(html, "who");
    expect(c.get("Halo0001")).toContain("/players/Halo0001");
    for (const bot of ["Clanker 1", "Rusty Lugnut"]) expect(c.get(bot)).toContain('class="bot-tag"');
    expect(c.get("Rusty Lugnut")).toContain("/players/Rusty%20Lugnut");
    expect(c.get("Clanker 1")).not.toContain("/players/");
  });
});

/** botGame() plus the server's bot account for the rostered bot. */
function withBotAccount(): LiveState {
  const s = botGame();
  applyEvents(s, [ev("BotIssued", `bot-${ROSTERED}`, {
    xuid: ROSTERED, gamertag: "Rusty Lugnut", persistent: true, emblem: null, colors: null, personality: null,
  })]);
  return s;
}

describe("bot accounts", () => {
  test("a BotIssued event makes a bot account; a human account is never overwritten", () => {
    const s = withBotAccount();
    expect(s.accounts[ROSTERED]).toMatchObject({ gamertag: "Rusty Lugnut", bot: true, online: false });
    expect(s.accounts[HUMAN]!.bot).toBe(false);
    applyEvents(s, [ev("BotIssued", `bot-${HUMAN}`, { xuid: HUMAN, gamertag: "Impostor" })]);
    expect(s.accounts[HUMAN]).toMatchObject({ gamertag: "Halo0001", bot: false });
  });

  test("an accounts snapshot carries bot and keeps a known bot a bot", () => {
    const s = withBotAccount();
    applyAccountsSnapshot(s, [{ xuid: ROSTERED, gamertag: "Rusty Lugnut" }]);
    expect(s.accounts[ROSTERED]!.bot).toBe(true);
    applyAccountsSnapshot(s, [{ xuid: "000b000000000009", gamertag: "Other", bot: true }]);
    expect(s.accounts["000b000000000009"]!.bot).toBe(true);
  });

  test("a bot with a known bot account keeps its name", () => {
    expect(withBotAccount().games.g1!.players[2]!.name).toBe("Rusty Lugnut");
  });

  test("the bot's career counts its own kills and deaths; the human's still leaves them out", () => {
    const s = withBotAccount();
    const bot = serviceRecord(s, ROSTERED);
    expect(bot.games).toBe(1);
    expect(bot.wins).toBe(0);
    expect(bot.kills).toBe(1);
    expect(bot.deaths).toBe(1);
    expect(bot.killsSeen).toBe(1);
    const human = serviceRecord(s, HUMAN);
    expect([human.kills, human.deaths, human.killsSeen]).toEqual([0, 0, 0]);
    expect(playedWith(s, HUMAN)).toEqual([]);
  });

  test("the players list leaves bots out", () => {
    // The page reads the live store, which is empty here: the filter is on accounts.
    expect(playersPage()).not.toContain("Rusty Lugnut");
    const s = withBotAccount();
    expect(Object.values(s.accounts).filter((a) => !a.bot).map((a) => a.gamertag)).toEqual(["Halo0001"]);
  });

  test("a bot's identity strip has the BOT tag", () => {
    const s = withBotAccount();
    const html = strip(s.accounts[ROSTERED]!, serviceRecord(s, ROSTERED));
    expect(html).toContain('class="bot-tag"');
    expect(html).toContain("Rusty Lugnut");
    expect(strip(s.accounts[HUMAN]!, serviceRecord(s, HUMAN))).not.toContain("bot-tag");
  });
});
