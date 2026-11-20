// Render the same table used by Uplink with synthetic member packages.
import { beforeAll, expect, test } from "bun:test";
import { plugin } from "bun";
import { parseContent } from "../src/admin/content";

const ROUTER = `${import.meta.dir}/../node_modules/@solidjs/router/dist/index.jsx`;
const uploader = { xuid: "0009000000000030", gamertag: "Archivist" };
const map = (name: string, stem: string, hash: string) => ({
  map_id: hash.repeat(64), name, author: uploader, base: { map: stem, derived: false },
  size: 2048, layers: [], state: "ok", reports: 0,
});
const maps = [map("Night Circuit: High Bridge Remix", "high_bridge", "a"), map("Estuary Remix", "estuary", "b")];
const pack = {
  id: "night-circuit", name: "Night Circuit", authors: ["Bridge Crew"], version: "1.2", year: 2008, advisory: "not_for_online", uploader,
  source_url: "https://mods.example/night", images: [{ sha256: "c".repeat(64) }], members: maps,
  map_count: 2, size: 3072, state: "hidden", reports: 7,
};
const content = parseContent({ packs: [pack], maps });
let render: (expanded: string | null) => string;

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
  const { createComponent } = await import("solid-js");
  const { renderToString } = await import("solid-js/web");
  const { default: Table } = await import("../src/admin/ModPackTable");
  render = (expanded) => renderToString(() => createComponent(Table, {
    content, expanded, api: "/v1", busy: false, onExpand() {}, onAction() {},
  }));
});

test("collapsed packs show one moderation row with credit, counts, cover and pack actions", () => {
  const html = render(null);
  expect(html.match(/aria-expanded="false"/g)).toHaveLength(1);
  expect(html).toContain("Night Circuit");
  expect(html).toContain("Bridge Crew");
  expect(html).toContain('/v1/mod-packs/night-circuit/images/0');
  expect(html).toContain("<td>2</td>");
  expect(html).toContain("<td>3 KB</td>"); // Shared layers count once in the pack total.
  expect(html).toContain("<td>7</td>");
  expect(html).toContain(">unhide</button>");
  expect(html).not.toContain("High Bridge Remix");
  expect(html).not.toContain("Estuary Remix");
});

test("expanding a pack reveals both maps and distinguishes the uploader from the makers", () => {
  const html = render(pack.id);
  expect(html).toContain('aria-expanded="true"');
  expect(html).toContain("High Bridge Remix");
  expect(html).toContain("Estuary Remix");
  expect(html).not.toContain("Night Circuit: High Bridge Remix");
  expect(html).toContain("Archivist");
  expect(html).toContain("Version 1.2");
  expect(html).toContain("Released 2008");
  expect(html).toContain("Not for online play");
  expect(html).toContain('href="https://mods.example/night"');
  expect(html.match(/>hide<\/button>/g)).toHaveLength(2);
});
