// The Ranked page's pieces rendered to HTML with a populated board: the
// page's .tsx compiled for the server the way vite-plugin-solid compiles it
// for the browser (babel-preset-solid), then rendered with renderToString —
// the data shaping itself is tested in ranks.test.ts.

import { beforeAll, describe, expect, test } from "bun:test";
import { plugin } from "bun";
import type { BoardRow, Standing } from "../src/lib/ranks";

let pieces = "";

// The router's JSX source (its "solid" export), so it renders on the server too.
const ROUTER = `${import.meta.dir}/../node_modules/@solidjs/router/dist/index.jsx`;

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
  const { Router, Route } = await import(ROUTER);
  const { createComponent, For } = await import("solid-js");
  const ranks = await import("../src/lib/ranks");
  const { EmptyBoard, PlaylistHead, PlaylistMenu, Row, Xp } = await import("../src/stats/Ranks");

  const st = (over: Partial<Standing> = {}): Standing => ({ level: 1, experience: 0, highest: 1, games: 0, wins: 0, ...over });
  const board: BoardRow[] = [
    { xuid: "0009000000000001", gamertag: "Sneaky Pete", ...st({ level: 34, experience: 29700, highest: 34, games: 210, wins: 130 }) },
    { xuid: "0009000000000002", gamertag: "Clanker Bob", bot: true, ...st({ level: 12, experience: 3850, highest: 14, games: 30, wins: 17 }) },
    { xuid: "0009000000000003", gamertag: "Mochi", ...st({ level: 50, experience: 63700, highest: 50, games: 900, wins: 500 }) },
  ];
  const menu = ranks.playlistMenu(null, null);
  const counts = Object.fromEntries(menu.map((p) => [p.id, p.id === "1" ? board.length : 0]));
  const games = [
    { game_id: "g1", playlist_id: "4", playlist: "Big Team Battle", map: "coagulation", started_at: "2026-11-18T10:00:00Z" },
    { game_id: "g2", playlist_id: "4", playlist: "Big Team Battle", map: "headlong", started_at: "2026-11-18T12:00:00Z" },
  ] as never[];

  pieces = renderToString(() =>
    createComponent(Router, {
      url: "/games/halo2/ranks/1",
      get children() {
        return createComponent(Route, {
          path: "/games/halo2/ranks/:playlist?",
          component: () => createComponent(For, {
            each: [
              createComponent(PlaylistMenu, { playlists: menu, current: "1", counts }),
              createComponent(PlaylistHead, { info: menu[1], id: "1", rows: board, capped: false }),
              createComponent(Row, { r: board[0]!, place: 1, you: false }),
              createComponent(Row, { r: board[1]!, place: 2, you: false }),
              createComponent(Row, { r: board[2]!, place: 3, you: true }),
              createComponent(Xp, { s: st({ level: 12, experience: 4200 }) }),
              createComponent(EmptyBoard, { info: menu[4], id: "4", games: [] }),
              createComponent(EmptyBoard, { info: menu[4], id: "4", games }),
              createComponent(EmptyBoard, { info: menu[6], id: "6", games: [] }),
            ],
            children: (piece: ReturnType<typeof createComponent>) => piece,
          }),
        });
      },
    })
  );
});

describe("the Ranked page, rendered", () => {
  test("the side menu names its three groups, lights the current pill and counts each board", () => {
    for (const h of ['<h3 class="s-rank-group">Ranked</h3>', '<h3 class="s-rank-group">Unranked</h3>', '<h3 class="s-rank-group">Clankermatch</h3>']) {
      expect(pieces).toContain(h);
    }
    const pill = pieces.slice(pieces.indexOf('href="/games/halo2/ranks/1"') - 40, pieces.indexOf('href="/games/halo2/ranks/1"') + 60);
    expect(pill).toContain('class=" active"');
    expect(pill).toContain('aria-current="page"');
    expect(pieces).toContain('<span class="s-rank-count">3</span>');
    expect(pieces).toContain('<span class="s-rank-count">0</span>');
  });

  test("the header panel says what the playlist is and what its board adds up to", () => {
    expect(pieces).toContain("<h3>Team Slayer</h3>");
    expect(pieces).toContain("Four against four Slayer on the small and medium maps.");
    expect(pieces).toContain('<span class="s-rank-kind s-rank-kind--ranked">Ranked</span>');
    expect(pieces).toContain("<span>2 teams of 3–4</span><span>Team Slayer</span>");
    expect(pieces).toContain("<dt>Ranked Players</dt><dd>3</dd>");
    expect(pieces).toContain("<dd>1,140</dd>"); // every ranked player's games added up
    expect(pieces).toContain('title="Every ranked player');
  });

  test("a row is place, player, badge, XP, games, wins, win rate — bots and you marked", () => {
    expect(pieces).toContain('<td class="num">1</td>');
    expect(pieces).toContain(">Sneaky Pete</a>");
    expect(pieces).toContain('href="/games/halo2/players/Clanker%20Bob"');
    expect(pieces).toContain('<span class="bot-tag"');
    expect(pieces).toMatch(/<span class="s-lvl t3"[^>]*>34<\/span>/);
    expect(pieces).toMatch(/<span class="s-lvl t1"[^>]*>12<\/span>/);
    expect(pieces).toMatch(/<span class="s-lvl t5"[^>]*>50<\/span>/);
    expect(pieces).toContain('title="29,700 XP, 1,750 to level 35"');
    expect(pieces).toContain('title="63,700 XP — level 50, the top of the ladder"');
    expect(pieces).toContain('style="width:100%"');
    expect(pieces).toContain('<td class="num">62%</td>');
    expect(pieces).toContain('<tr class="sel"');
    expect(pieces).toContain('<span class="s-rank-youtag">you</span>');
  });

  test("the empty board explains how to get ranked, and links the playlist's games by their maps", () => {
    expect(pieces).toContain("Nobody has played Big Team Battle ranked yet.");
    expect(pieces).toContain("Xbox Live › OptiMatch › Big Team Battle");
    expect(pieces).toContain("No games of this playlist on record yet either");
    expect(pieces).toContain('href="/games/halo2/games/g1"');
    expect(pieces).toContain(">Coagulation</a>");
    expect(pieces).toContain(">Headlong</a>");
    expect(pieces).toContain(">every game on record</a>");
    // An unranked playlist never fills: it says so instead.
    expect(pieces).toContain("Free for All keeps no ranks.");
  });
});
