import { describe, expect, test } from "bun:test";
import { COMPOSITIONS, SHOTS, camera, candidates, check, layout, mapsOf, panelWeight, placeCast, type Composition, type Shot, type Spot } from "../banner/plan";
import { applyBanners, writeManifest } from "../banner/manifest";
import { mkdtempSync, readFileSync, rmSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { brotliDecompressSync, gunzipSync } from "node:zlib";
import { bannerKey, gameArt, parseManifest } from "../src/lib/gameart";

const COMP: Composition = { id: "t", width: 1000, height: 200, shots: ["a", "b", "c", "d"], slant: 0.2, feather: 0.25, mark: "right" };
const close = (a: number, b: number, eps = 1e-6) => expect(Math.abs(a - b)).toBeLessThan(eps);

describe("banner layout", () => {
  test("panels cover the width left to right, seams lean and overlap", () => {
    const panels = layout(COMP);
    expect(panels.map((p) => p.shot)).toEqual(["a", "b", "c", "d"]);
    expect(panels[0]!.left).toBeNull();
    expect(panels[3]!.right).toBeNull();
    expect(panels[0]!.box.x).toBe(0);
    const last = panels[3]!.box;
    expect(last.x + last.width).toBe(1000);
    for (let i = 1; i < panels.length; i++) {
      const s = panels[i]!.left!;
      // The same seam on both sides; it leans right at the top.
      expect(panels[i - 1]!.right).toBe(s);
      expect(s.top - s.bottom).toBeCloseTo(0.2 * 200);
      expect((s.top + s.bottom) / 2).toBeCloseTo(i * 250);
      // Each box reaches past its seams by the lean and half the blend.
      expect(panels[i]!.box.x).toBeLessThanOrEqual(s.bottom - s.feather / 2);
      const prev = panels[i - 1]!.box;
      expect(prev.x + prev.width).toBeGreaterThanOrEqual(s.top + s.feather / 2);
    }
  });

  test("neighbours' weights sum to one everywhere", () => {
    const panels = layout(COMP);
    for (let x = 0; x < 1000; x += 7) {
      for (const y of [0, 50, 100, 199]) {
        const sum = panels.reduce((a, p) => a + panelWeight(p, x, y, COMP.height), 0);
        close(sum, 1);
      }
    }
  });

  test("a panel is whole between its seams and gone past them", () => {
    const [a, b] = layout(COMP);
    expect(panelWeight(a!, 10, 100, 200)).toBe(1);
    expect(panelWeight(a!, 600, 100, 200)).toBe(0);
    expect(panelWeight(b!, 375, 100, 200)).toBe(1);
  });

  test("one shot is the whole picture; none is nothing", () => {
    const [only] = layout({ ...COMP, shots: ["a"] });
    expect(only!.box).toEqual({ x: 0, y: 0, width: 1000, height: 200 });
    expect(panelWeight(only!, 999, 0, 200)).toBe(1);
    expect(layout({ ...COMP, shots: [] })).toEqual([]);
  });
});

describe("banner cameras", () => {
  const box = { min: [0, 0, 0] as [number, number, number], max: [100, 40, 10] as [number, number, number] };

  test("a bounds shot stands at its angle and distance from its point", () => {
    const shot: Shot = { id: "s", map: "m", at: [0.5, 0.5, 0], azimuth: 90, elevation: 0, distance: 0.5, fov: 40 };
    const cam = camera(shot, box)!;
    expect(cam.at).toEqual([50, 20, 0]);
    const r = Math.hypot(100, 40, 10) / 2 * 0.5;
    close(cam.eye[0], 50, 1e-9);
    close(cam.eye[1], 20 + r, 1e-9);
    close(cam.eye[2], 0, 1e-9);
  });

  test("a spot shot stands behind and above its spot, looking at the other", () => {
    const spots = new Map<string, { at: [number, number, number] }>([["netgame:1", { at: [10, 10, 2] }], ["netgame:2", { at: [30, 10, 2] }]]);
    const shot: Shot = { id: "s", map: "m", spot: "netgame:1", toward: "netgame:2", back: 3, lift: 1, fov: 50 };
    const cam = camera(shot, box, spots)!;
    expect(cam.eye).toEqual([7, 10, 3]);
    close(cam.at[0], 30); close(cam.at[1], 10); close(cam.at[2], 2);
    // Turned left (towards +y when looking along +x), and aimed lower.
    const turned = camera({ ...shot, turn: 90, drop: 1 }, box, spots)!;
    close(turned.at[0], 7); expect(turned.at[1]).toBeGreaterThan(10); close(turned.at[2], 1);
  });

  test("a spot shot without `toward` looks at the level's middle; a missing spot is null", () => {
    const spots = new Map([["vehi:0", { at: [10, 20, 0] as [number, number, number] }]]);
    const cam = camera({ id: "s", map: "m", spot: "vehi:0", back: 0, lift: 0, fov: 50, level: 0.5 }, box, spots)!;
    expect(cam.at).toEqual([50, 20, 5]);
    expect(camera({ id: "s", map: "m", spot: "vehi:9", back: 0, lift: 0, fov: 50 }, box, spots)).toBeNull();
    expect(camera({ id: "s", map: "m", spot: "vehi:0", toward: "vehi:9", back: 0, lift: 0, fov: 50 }, box, spots)).toBeNull();
  });

  test("the cast stands ahead of and beside the camera, facing as asked", () => {
    const [front, side] = placeCast([
      { species: "spartan", colour: "#f00", ahead: 2, left: 0, face: 180 },
      { species: "elite", colour: "#00f", ahead: 0, left: 1, face: 0 },
    ], [0, 0, 5], [10, 0, 5], -1);
    expect([front!.x, front!.y, front!.z]).toEqual([2, 0, -1]);
    close(front!.yaw, Math.PI);
    close(side!.x, 0); close(side!.y, 1); close(side!.yaw, 0);
  });
});

describe("banner plan", () => {
  test("the committed plan is sound", () => {
    expect(check(SHOTS, COMPOSITIONS)).toEqual([]);
    expect(COMPOSITIONS.map((c) => c.id)).toEqual(["home", "strip"]);
    expect(mapsOf(SHOTS, COMPOSITIONS).length).toBeGreaterThan(3);
  });

  test("check names what is wrong", () => {
    const bad: Shot[] = [
      { id: "a", map: "Bad Map", at: [2, 0, 0], azimuth: 0, elevation: 0, distance: 0, fov: 1 },
      { id: "a", map: "ok", spot: "netgame", back: 1, lift: 1, fov: 50 },
    ];
    const problems = check(bad, [{ ...COMP, id: "x y", shots: ["a", "zz"] }, { ...COMP, id: "x y", shots: [] }]);
    for (const p of ["listed twice", "not a stem", "fov", "outside the bounds", "distance", "<kind>:<index>", "no shot zz", "not a file name", "no shots"]) {
      expect(problems.some((x) => x.includes(p))).toBe(true);
    }
  });

  test("candidates put vehicles first, then spread the rest", () => {
    const spots = new Map<string, Spot>([
      ["netgame:0", { kind: "netgame", item: "weap", at: [0, 0, 0] }],
      ["netgame:1", { kind: "netgame", item: "weap", at: [1, 0, 0] }],
      ["netgame:2", { kind: "netgame", item: "weap", at: [50, 0, 0] }],
      ["netgame:3", { kind: "netgame", item: "vehi", at: [25, 0, 0] }],
      ["scen:0", { kind: "scen", item: null, at: [99, 0, 0] }],
    ]);
    const picks = candidates("m", spots, 3);
    expect(picks.map((s) => s.id)).toEqual(["m-netgame-3", "m-netgame-0", "m-netgame-2"]);
    expect(check(picks, [])).toEqual([]);
  });
});

describe("banner manifest", () => {
  test("banners replace earlier ones and keep everything else", () => {
    const before = { version: 1, source: "abc", images: { "rank/1": { file: "rank/1.png", width: 1, height: 1 }, "banner/old": { file: "banner/old.jpg", width: 2, height: 2 } }, maps: { m: {} } };
    const after = applyBanners(before, [{ id: "home", file: "banner/home.jpg", width: 1804, height: 360 }]);
    expect(after.images).toEqual({ "rank/1": { file: "rank/1.png", width: 1, height: 1 }, "banner/home": { file: "banner/home.jpg", width: 1804, height: 360 } });
    expect(after.maps).toEqual({ m: {} });
    expect(after.source).toBe("abc");
    // And the web app finds them under its keys.
    const art = gameArt("/art/game/", parseManifest(after)!);
    expect(art.get(bannerKey("home"))).toEqual({ url: "/art/game/banner/home.jpg", width: 1804, height: 360 });
    expect(art.get(bannerKey("players"))).toBeNull();
  });

  test("the manifest's compressed copies are written again with it", () => {
    // The extraction's copies hold the manifest without the banners; the
    // server and the publish send the Brotli copy in place of the file.
    const dir = mkdtempSync(join(tmpdir(), "h2-banner-manifest-"));
    try {
      const path = join(dir, "manifest.json");
      const old = JSON.stringify({ version: 1, images: {} });
      writeFileSync(path, old);
      writeFileSync(path + ".br", "stale brotli");
      writeFileSync(path + ".gz", "stale gzip");
      const past = new Date(Date.now() - 3_600_000);
      for (const p of [path, path + ".br", path + ".gz"]) utimesSync(p, past, past);

      const m = applyBanners(JSON.parse(old), [{ id: "home", file: "banner/home.jpg", width: 1804, height: 360 }]);
      writeManifest(path, m);
      const plain = readFileSync(path);
      expect(JSON.parse(plain.toString()).images["banner/home"]).toEqual({ file: "banner/home.jpg", width: 1804, height: 360 });
      expect(brotliDecompressSync(readFileSync(path + ".br")).equals(plain)).toBe(true);
      expect(gunzipSync(readFileSync(path + ".gz")).equals(plain)).toBe(true);
      // Never older than the file, so the server does not take them as stale.
      for (const ext of [".br", ".gz"]) expect(statSync(path + ext).mtimeMs).toBeGreaterThanOrEqual(statSync(path).mtimeMs);
      expect(() => statSync(path + ".banner")).toThrow();
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("the strip's key", () => {
    expect(bannerKey("home")).toBe("banner/home");
    expect(bannerKey("players")).toBe("banner/strip");
  });
});
