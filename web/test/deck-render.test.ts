// The dense dashboard's style page rendered to HTML with its fixed data
// (compiled for the server the way stats-home-render.test.ts does): every
// panel is there, tiles carry proportional figures, charts draw their marks
// with the validated slots, and tables are sortable with tabular figures.

import { beforeAll, describe, expect, spyOn, test } from "bun:test";
import { plugin } from "bun";
import { SERIES } from "../src/deck/palette";
import { sideOf } from "../src/dash/cross";

let html = "";
let usage = "";
let hosts = "";
let events = "";
let hub = "";
let signedOutHub = "";
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
  const { createComponent } = await import("solid-js");
  const { default: StylePage } = await import("../src/deck/StylePage");
  const { default: UsagePage } = await import("../src/deck/UsagePage");
  const { default: HostsPage } = await import("../src/deck/HostsPage");
  const { default: EventLog } = await import("../src/dash/EventLog");
  const { default: AdminPage } = await import("../src/deck/AdminPage");
  const { adminState } = await import("../src/admin/state");
  const page = (url: string, component: typeof AdminPage) => renderToString(() => createComponent(Router, {
    url,
    get children() { return createComponent(Route, { path: url, component }); },
  }));
  html = page("/dash/admin/style", StylePage);
  usage = page("/dash/admin/usage", UsagePage);
  hosts = page("/dash/admin/hosts", HostsPage);
  events = page("/dash/admin/events", EventLog);
  signedOutHub = page("/dash/admin", AdminPage);
  const admin = spyOn(adminState, "isMeAdmin").mockReturnValue(true);
  try { hub = page("/dash/admin", AdminPage); } finally { admin.mockRestore(); }
});

describe("the Live admin hub", () => {
  test("links to the four tools when admin access is confirmed", () => {
    expect(hub).toContain("<h1>live admin</h1>");
    expect(hub).toMatch(/href="\/dash\/settings" class="dk-back[ "]/);
    expect(hub).toMatch(/href="\/dash\/admin\/usage"[^>]*>usage<\/a>/);
    expect(hub).toMatch(/href="\/dash\/admin\/hosts"[^>]*>hosts<\/a>/);
    expect(hub).toMatch(/href="\/dash\/admin\/events"[^>]*>event log<\/a>/);
    expect(hub).toMatch(/href="\/dash\/admin\/style"[^>]*>style<\/a>/);
  });

  test("asks for sign-in when signed out", () => {
    expect(signedOutHub).toContain('href="/dash/signin"');
    expect(signedOutHub).not.toContain('aria-label="Server tools"');
  });

  test("each tool links back to the hub in its trail", () => {
    for (const tool of [html, usage, hosts, events]) {
      expect(tool).toMatch(/href="\/dash\/admin"[^>]*>live admin<\/a>/);
    }
  });

  test("all deck screens use the dashboard's Back prompt", () => {
    for (const screen of [hub, html, usage, hosts, events]) {
      expect(screen).toContain("dk-back dx-prompt dx-prompt--b");
      expect(screen).toMatch(/class="dx-btn dx-btn--b dx-btn--keyboard"[^>]*><b>esc<\/b>/);
      expect(screen).toContain('<span class="dx-prompt-label">back</span>');
    }
  });
});

describe("the dense dashboard style page", () => {
  test("is on the dashboard side, so leaving it for the stats plays the veil", () => {
    expect(sideOf("/dash/admin/style")).toBe("dash");
  });

  test("has the trail, the title, the way back and one filter row", () => {
    expect(html).toContain('class="dk-trail"');
    expect(html).toMatch(/<h1[^>]*>usage<\/h1>/);
    expect(html).toMatch(/href="\/dash\/server" class="dk-back[ "]/);
    expect(html.match(/class="dk-filters dk-toolbar"/g)).toHaveLength(1);
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

  test("the top accounts link to dashboard players", () => {
    const players = [...html.matchAll(/<a\b[^>]*\bhref="\/dash\/players\/([^"]+)"[^>]*>([^<]+)<\/a>/g)];
    expect(players).toHaveLength(12);
    for (const [, tag, label] of players) expect(decodeURIComponent(tag!)).toBe(label!);
    expect(html).not.toContain('href="/games/halo2/players/');
  });

  test("the sample data says it is a sample", () => {
    expect(html).toContain("fixed sample data");
  });
});

describe("the Live admin usage page", () => {
  test("is on the dashboard side, under the server in the trail", () => {
    expect(sideOf("/dash/admin/usage")).toBe("dash");
    expect(usage).toMatch(/href="\/dash\/server" class="dk-back[ "]/);
  });

  test("signed out it reads nothing and asks for an admin's sign-in, with no filters or numbers", () => {
    expect(usage).toContain("sign in first");
    expect(usage).toMatch(/href="\/dash\/signin"/);
    expect(usage).not.toContain("dk-tile");
    expect(usage).not.toContain("top accounts by");
  });
});
