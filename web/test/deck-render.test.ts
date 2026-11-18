// The dense dashboard's style page rendered to HTML with its fixed data
// (compiled for the server the way stats-home-render.test.ts does): every
// panel is there, tiles carry proportional figures, charts draw their marks
// with the validated slots, and tables are sortable with tabular figures.

import { beforeAll, describe, expect, test } from "bun:test";
import { plugin } from "bun";
import { SERIES } from "../src/deck/palette";
import { sideOf } from "../src/dash/cross";

let html = "";
let usage = "";
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
  const { default: StylePage } = await import("../src/deck/StylePage");
  const { createComponent } = await import("solid-js");
  html = renderToString(() => createComponent(Router, {
    url: "/dash/admin/style",
    get children() { return createComponent(Route, { path: "/dash/admin/style", component: StylePage }); },
  }));
  const { default: UsagePage } = await import("../src/deck/UsagePage");
  usage = renderToString(() => createComponent(Router, {
    url: "/dash/admin/usage",
    get children() { return createComponent(Route, { path: "/dash/admin/usage", component: UsagePage }); },
  }));
});

describe("the dense dashboard style page", () => {
  test("is on the dashboard side, so leaving it for the stats plays the veil", () => {
    expect(sideOf("/dash/admin/style")).toBe("dash");
  });

  test("has the trail, the title, the way back and one filter row", () => {
    expect(html).toContain('class="dk-trail"');
    expect(html).toMatch(/<h1[^>]*>usage<\/h1>/);
    expect(html).toMatch(/href="\/dash\/server" class="dk-back[ "]/);
    expect(html.match(/class="dk-filters"/g)).toHaveLength(1);
    expect(html).toMatch(/role="radio"[^>]*aria-checked="true"[^>]*>30 days/);
  });

  test("six tiles, then every panel", () => {
    expect(html.match(/class="dk-panel dk-tile/g)).toHaveLength(6);
    for (const t of ["data uploaded", "stored", "events written", "messages sent", "players", "events by type", "top accounts"]) {
      expect(html).toContain(`<h2>${t}</h2>`);
    }
  });

  test("the stacked chart uses the slots in order, and only the validated ones", () => {
    const fills = [...html.matchAll(/fill="(#[0-9a-f]{6})"/g)].map((m) => m[1]!);
    for (const c of SERIES.slice(0, 4)) expect(fills).toContain(c);
    const strokes = new Set([...html.matchAll(/stroke="(#[0-9a-f]{6})"/g)].map((m) => m[1]!));
    // Lines are drawn in slots, the single-series accent, the quiet sparkline hue,
    // the grid, the baseline, or the well (the 2px ring) — nothing else.
    for (const s of strokes) expect(["#9be34e", "#46703a", "#163018", "#2e5a26", "#08180a", ...SERIES]).toContain(s);
  });

  test("charts are reachable without a pointer and say how", () => {
    expect(html).toMatch(/<svg[^>]*role="img"[^>]*tabindex="0"[^>]*aria-label="Data uploaded per day by kind: 30 days/);
    expect(html.match(/aria-pressed="false"[^>]*>table</g)!.length).toBeGreaterThanOrEqual(4);
  });

  test("the accounts table sorts by data uploaded, largest first", () => {
    expect(html).toMatch(/aria-sort="descending"[^>]*><button[^>]*>uploaded/);
    const rows = [...html.matchAll(/<tr data-key="([^"]+)"/g)];
    expect(rows).toHaveLength(12);
  });

  test("the sample data says it is a sample", () => {
    expect(html).toContain("fixed sample data");
  });
});

describe("the Live admin usage page", () => {
  test("is on the dashboard side, under the server in the trail", () => {
    expect(sideOf("/dash/admin/usage")).toBe("dash");
    expect(usage).toContain(">live admin<");
    expect(usage).toMatch(/href="\/dash\/server" class="dk-back[ "]/);
  });

  test("signed out it reads nothing and asks for an admin's sign-in, with no filters or numbers", () => {
    expect(usage).toContain("sign in first");
    expect(usage).toMatch(/href="\/dash\/signin"/);
    expect(usage).not.toContain("dk-tile");
    expect(usage).not.toContain("top accounts by");
  });
});
