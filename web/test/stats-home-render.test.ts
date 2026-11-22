// The stats home rendered to HTML with the mock's numbers: the app's .tsx
// compiled for the server the way vite-plugin-solid compiles it for the
// browser (babel-preset-solid), then rendered with renderToString.

import { beforeAll, describe, expect, test } from "bun:test";
import { plugin } from "bun";
import { STATS_NAME } from "../src/brand";

let html = "";
let unsupported = "";
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
  const { HomeView } = await import("../src/stats/Home");
  const { default: StatsLayout } = await import("../src/stats/Layout");
  const { parseSummary } = await import("../src/lib/statsHome");
  const { StatsFold, archive } = await import("../mock/stats");
  const { createComponent } = await import("solid-js");

  const until = Date.now();
  const accts = ["Sneaky Pete", "Captain Waffles", "Mochi", "Noodle", "Pixel Pop", "Sprinkles"]
    .map((gamertag, i) => ({ xuid: `00090000000000${String(i).padStart(2, "0")}`, gamertag }));
  const fold = new StatsFold();
  for (const e of archive(accts, until - 3_600_000)) fold.apply(e);
  const week = parseSummary(fold.summary("7d", until, 1));

  const page = (sum: ReturnType<typeof parseSummary> | null, status: "ok" | "unsupported") =>
    renderToString(() => createComponent(Router, {
      url: "/games/halo2",
      get children() {
        return createComponent(Route, {
          path: "/games/halo2", component: StatsLayout,
          get children() {
            return createComponent(Route, { path: "/", component: () => createComponent(HomeView, { win: "7d", week: sum, weekStatus: status, sum, status }) });
          },
        });
      },
    }));
  html = page(week, "ok");
  unsupported = page(null, "unsupported");
});

describe("the stats home, rendered", () => {
  test("the nav has the home tab first and the dashboard as a way out, not a tab", () => {
    const nav = html.slice(html.indexOf('class="s-nav"'), html.indexOf("</nav>"));
    expect(nav.indexOf(STATS_NAME)).toBeLessThan(nav.indexOf("My Stats"));
    expect(nav).not.toContain("Dashboard");
    expect(html).toMatch(/<a [^>]*href="\/" class="s-exit[^"]*"[^>]*>‹ Live Dashboard<\/a>/);
  });

  test("the marks name the site h2 Uplink, not h2 live", () => {
    // The header logo and the banner strip's mark are the Uplink wordmark.
    const marks = [...html.matchAll(/<svg [^>]*class="wordmark[^"]*"[^>]*>/g)].map((m) => m[0]);
    expect(marks.length).toBeGreaterThanOrEqual(2);
    for (const m of marks) expect(m).toContain(`aria-label="${STATS_NAME}"`);
    expect(html).not.toMatch(/h2 live Players Online/);
    expect(html).toMatch(/\d+ Players Online \(\d+ Games? Live\)/);
    // Every link back to the dashboard says which one.
    for (const m of html.matchAll(/<a [^>]*href="\/"[^>]*>([^<]*)<\/a>/g)) expect(m[1]).toContain("Live Dashboard");
  });

  test("the crumbs start at Home", () => {
    expect(html).toMatch(/class="s-crumbs-trail"><span>Home<\/span>/);
  });

  test("every section is there", () => {
    for (const h of ["Live Now", "Last 24 Hours", "All Time", "Leaderboards", "Most Kills", "Best K/D Ratio", "Most Wins",
      "Longest Killing Spree", "Legendary Levels", "Most Time Played", "Most Played Maps", "Most Used Weapons", "When People Play",
      "Game Types", "Skulls On", "Recent Games", "Weapon of the Week", "Map of the Week", "Server Report", "Clan Standings"]) {
      expect(html).toContain(h);
    }
  });

  test("the boards name the mock's players and the windows are tabs", () => {
    expect(html).toContain("Sneaky Pete");
    for (const t of ["Today", "This Week", "This Month", "All Time"]) expect(html).toContain(`>${t}</a>`);
    expect(html).toContain('href="/games/halo2?window=30d"');
  });

  test("the heatmap is a table with every hour of every day", () => {
    const grid = html.slice(html.indexOf('class="s-heat-grid"'));
    expect(grid.match(/<td class="lv\d"/g)!.length).toBe(7 * 24);
  });

  test("with nothing live and no games on record, Live Now shows a map montage, not an empty box", () => {
    const panel = html.slice(html.indexOf('id="live"'), html.indexOf('class="s-tallies"'));
    expect(panel).toContain("s-idle-montage");
    expect(panel).toContain("Pick a map, start a game");
    expect(panel).toContain("The next one shows up here as it starts.");
    expect(panel).not.toContain('class="s-empty"');
  });

  test("the open games list shows the filters button before the first session response", () => {
    const start = html.indexOf('aria-labelledby="s-open-h"');
    expect(start).toBeGreaterThanOrEqual(0);
    const panel = html.slice(start, html.indexOf("</section>", start));
    expect(panel).toContain("Open Games");
    expect(panel).toContain('aria-controls="s-open-filters"');
    expect(panel).toContain("Loading open games");
    for (const filter of ["Campaign Co-op", "Clankers", "Game type", "Any variant", "Map category", "Difficulty", "Has space"]) {
      expect(panel).toContain(filter);
    }
  });

  test("the menu column drops zero rows and keeps the ones that always count", () => {
    const report = html.slice(html.indexOf("Server Report"), html.indexOf("Daily Count"));
    expect(report).toContain("Games on Record");
    expect(report).toContain("Accounts");
  });

  test("a server without the summary says so and still shows what is live", () => {
    expect(unsupported).toContain("publish leaderboards yet");
    expect(unsupported).toContain("Live Now");
    expect(unsupported).toContain("Recent Games");
    expect(unsupported).not.toContain("Most Kills");
  });
});
