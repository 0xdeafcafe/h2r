// Pack screens use invented maps, credits and images.
import { beforeAll, expect, test } from "bun:test";
import { plugin } from "bun";
import { filterPacks, packCoverSources, parseModPack, type ModPack } from "../src/lib/modPacks";

const ROUTER = `${import.meta.dir}/../node_modules/@solidjs/router/dist/index.jsx`;
const uploader = { xuid: "0009000000002040", gamertag: "Publisher" };
const a = "a".repeat(64), b = "b".repeat(64), c = "c".repeat(64);
const member = (id: string, base: string) => ({
  map_id: id, name: `High Tide: ${base}`, base: { map: base }, author: uploader,
  category: "visuals", size: 1024, images: [{ sha256: b }], downloads: 1,
});
const release = {
  id: "high-tide", name: "High Tide", authors: ["Nell", "North Shore"], year: 2007,
  version: "2.6", advisory: "not_for_online", source_url: "https://mods.example/tide",
  members: [member(a, "beacon"), member(b, "orchard")], images: [{ sha256: c }], size: 2048, uploader,
};
const pack = parseModPack(release)!;

let render: (p: ModPack, detail?: boolean) => string;
beforeAll(async () => {
  const babel = await import("@babel/core");
  const solid = (await import("babel-preset-solid")).default;
  plugin({
    name: "solid-ssr",
    setup(build) {
      build.onLoad({ filter: /(\/src\/.*\.tsx|\/@solidjs\/router\/dist\/.*\.jsx)$/ }, async ({ path }) => {
        const src = await Bun.file(path).text();
        const out = await babel.transformAsync(src, {
          filename: path, babelrc: false, configFile: false, parserOpts: { plugins: ["jsx", "typescript"] },
          presets: [[solid, { generate: "ssr", hydratable: false }]],
        });
        return { contents: out!.code!.replaceAll('"@solidjs/router"', JSON.stringify(ROUTER)), loader: "ts" };
      });
    },
  });
  const { renderToString } = await import("solid-js/web");
  const { Router, Route } = await import(ROUTER);
  const { createComponent } = await import("solid-js");
  const { PackDetail, PackRow } = await import("../src/stats/Mods");
  render = (p, detail = false) => renderToString(() => createComponent(Router, {
    url: "/mods",
    get children() {
      return createComponent(Route, { path: "/mods", component: () => detail ?
        createComponent(PackDetail, { pack: p, changed() {} }) : createComponent(PackRow, { pack: p }) });
    },
  }));
});

test("the browser row credits the makers and links the maps inside one release", () => {
  const html = render(pack);
  expect(html.match(/class="s-mod-name"/g)).toHaveLength(1);
  expect(html).toContain("View maps");
  expect(html).toContain("Made by");
  expect(html).toContain("Nell, North Shore");
  expect(html).toContain("Uploaded by");
  expect(html).toContain("Publisher");
  expect(html).toContain("2 maps");
  expect(html).toContain("2007");
  expect(html).toContain("Not for online play");
  expect(html).toContain('href="https://mods.example/tide"');
  expect(html).toContain(`/mod-packs/high-tide/images/0?v=${c}`);
  for (const id of [a, b]) expect(html).toContain(`href="/games/halo2/mods/${id}"`);
});

test("the pack page shows pack screenshots, source and pack-level report; missing pack artwork uses member shots", () => {
  const full = render(pack, true);
  expect(full).toContain("Pack screenshots");
  expect(full).toContain("Included maps");
  expect(full).toContain("report this pack");
  expect(full).not.toContain("Edit pack");
  const memberArt = render({ ...pack, images: [] }, true);
  expect(memberArt).toContain("Map screenshots");
  expect(memberArt).toContain(`/maps/${a}/images/0?v=${b}`);
  expect(memberArt).not.toContain("Stock map picture");
});

test("stock art gets a visible label only when there is no pack or member artwork", async () => {
  const { gameArt, parseManifest } = await import("../src/lib/gameart");
  const { setGameArtForTest } = await import("../src/state/gameart");
  const manifest = parseManifest({ version: 1, images: {
    "map/beacon/preview": { file: "map/beacon/preview.png", width: 4, height: 3 },
  } });
  setGameArtForTest(gameArt("https://art.example/", manifest!));
  try {
    const noShots = { ...pack, images: [], members: pack.members.map((m) => ({ ...m, image: undefined, images: [] })) };
    expect(render(noShots)).toContain(">Stock map picture</span>");
    expect(render(pack)).not.toContain("Stock map picture");
  } finally {
    setGameArtForTest(null);
  }
});

test("matching a member keeps its siblings and a pack's authors are searchable", () => {
  expect(filterPacks([pack], { sort: "new", base: "beacon" })[0]?.members).toHaveLength(2);
  expect(filterPacks([pack], { sort: "new", q: "north shore" })).toEqual([pack]);
  expect(filterPacks([pack], { sort: "new", author: "Nell" })).toEqual([]);
  expect(packCoverSources("/v1", pack)[0]).toBe(`/v1/mod-packs/high-tide/images/0?v=${c}`);
  const unsafe = parseModPack({ ...release, source_url: "javascript:x", advisory: "other", year: "2000" });
  expect(unsafe?.source_url).toBeUndefined();
  expect(unsafe?.advisory).toBeUndefined();
  expect(unsafe?.year).toBeUndefined();
});
