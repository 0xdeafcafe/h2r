// Both sides carry the Game Content Usage Rules notice, worded as the rules
// ask, with the rules linked: the stats footer (rendered) and the dashboard
// menu (its component rendered; Dashboard.tsx is checked for using it).

import { beforeAll, describe, expect, test } from "bun:test";
import { plugin } from "bun";
import { DASH_NAME, STATS_NAME } from "../src/brand";
import { RULES_URL, noticeText } from "../src/legal";

const ROUTER = `${import.meta.dir}/../node_modules/@solidjs/router/dist/index.jsx`;
let stats = "";
let dash = "";

const strip = (h: string) => h.replace(/<!--.*?-->/g, "").replace(/<[^>]+>/g, "").replace(/&#39;|&#x27;/g, "'").replace(/&quot;/g, '"').replace(/&amp;/g, "&");

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
  const { createComponent } = await import("solid-js");
  const { default: StatsLayout } = await import("../src/stats/Layout");
  const { default: Legal } = await import("../src/components/Legal");
  stats = renderToString(() => createComponent(Router, {
    url: "/stats",
    get children() { return createComponent(Route, { path: "/stats", component: StatsLayout, get children() { return createComponent(Route, { path: "/", component: () => "" }); } }); },
  }));
  dash = renderToString(() => createComponent(Legal, { name: DASH_NAME, class: "dx-legal" }));
});

describe("the Game Content Usage Rules notice", () => {
  test("the stats footer carries it, with its own name, and the old line is gone", () => {
    const foot = stats.slice(stats.indexOf('class="s-foot"'));
    expect(strip(foot)).toContain(noticeText(STATS_NAME));
    expect(foot).not.toContain("Not affiliated with any game publisher");
  });

  test("the dashboard's component carries it, with its own name", () => {
    expect(strip(dash)).toContain(noticeText(DASH_NAME));
  });

  test("the dashboard menu renders it under its own name", async () => {
    const src = await Bun.file(`${import.meta.dir}/../src/dash/Dashboard.tsx`).text();
    expect(src).toContain("<Legal name={DASH_NAME}");
  });

  test("the rules are linked, and it is not called fair use", () => {
    for (const h of [stats, dash]) expect(h).toContain(`href="${RULES_URL}"`);
    expect(RULES_URL).toBe("https://www.xbox.com/en-US/developers/rules");
    expect(noticeText("x").toLowerCase()).not.toContain("fair use");
  });

  test("the notice is the rules' wording", () => {
    expect(noticeText("Foo")).toContain(`Foo was created under Microsoft's "Game Content Usage Rules" using assets from Halo 2 and Halo: Combat Evolved, and it is not endorsed by or affiliated with Microsoft.`);
    expect(noticeText("Foo")).toContain("Halo © Microsoft Corporation.");
    expect(noticeText("Foo")).toContain("free, non-commercial fan project");
  });
});
