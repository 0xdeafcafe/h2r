import { describe, expect, test } from "bun:test";
import {
  clampPan, defaultView, depthOf, easePanZoom, fitPicture, gameToGl, glToGame, mapStem, parseMapFiles, parseViews,
  viewAxis, viewImageFile, viewLabel, viewScale, worldToPixel, yawOnView, yawToGl, zoomAbout, type Mat2x4,
} from "../src/map/views";
import { parseManifest } from "../src/lib/gameart";
import { buildArena, encodeGlb, mockMapGeometry, triangulate, viewsJson, VIEW_IDS } from "../mock/arena";

// A top view: 10 px per unit, world (-50, 50) at the top-left pixel, north up.
const TOP: Mat2x4 = [[10, 0, 0, 500], [0, -10, 0, 500]];

describe("world_to_pixel", () => {
  test("a top view puts +x right and +y (north) up, and ignores height", () => {
    expect(worldToPixel(TOP, 0, 0, 0)).toEqual([500, 500]);
    expect(worldToPixel(TOP, 10, 0, 0)).toEqual([600, 500]);
    expect(worldToPixel(TOP, 0, 10, 0)).toEqual([500, 400]);
    expect(worldToPixel(TOP, -50, 50, 7)).toEqual([0, 0]);
  });

  test("an iso view moves a point up the picture as it rises", () => {
    const iso: Mat2x4 = [[7, -7, 0, 512], [3.5, 3.5, -8.6, 400]];
    const [x0, y0] = worldToPixel(iso, 4, 4, 0), [x1, y1] = worldToPixel(iso, 4, 4, 5);
    expect(x1).toBe(x0);
    expect(y1).toBeCloseTo(y0 - 43, 6);
  });

  test("the view axis points at the viewer: up for a top view", () => {
    const a = viewAxis(TOP);
    expect(a[0]).toBeCloseTo(0, 9);
    expect(a[1]).toBeCloseTo(0, 9);
    expect(a[2]).toBeCloseTo(1, 9);
    expect(depthOf(a, 3, 4, 9)).toBeGreaterThan(depthOf(a, 3, 4, 1));
    expect(viewScale(TOP)).toBe(10);
  });

  test("yaw 0 points right on a top view and a quarter turn points up", () => {
    expect(yawOnView(TOP, 0)).toBeCloseTo(0, 9);
    expect(yawOnView(TOP, Math.PI / 2)).toBeCloseTo(-Math.PI / 2, 9);
  });

  test("the mock's views plot its own bounds inside each picture, and agree with its drawing", () => {
    const arena = buildArena("teacup")!;
    const views = parseViews(viewsJson(arena))!;
    expect(views.views.map((v) => v.id)).toEqual(VIEW_IDS);
    expect(defaultView(views).id).toBe("top");
    const { min, max } = views.bounds;
    for (const v of views.views) {
      for (const x of [min[0], max[0]]) for (const y of [min[1], max[1]]) for (const z of [min[2], max[2]]) {
        const [px, py] = worldToPixel(v.world_to_pixel, x, y, z);
        expect(px).toBeGreaterThanOrEqual(0); expect(px).toBeLessThanOrEqual(v.width);
        expect(py).toBeGreaterThanOrEqual(0); expect(py).toBeLessThanOrEqual(v.height);
      }
      // Every iso view looks down from 30° up; the top view straight down.
      expect(viewAxis(v.world_to_pixel)[2]).toBeCloseTo(v.id === "top" ? 1 : 0.5, 3);
    }
    // iso-ne is seen from the north-east corner.
    const ne = viewAxis(views.views.find((v) => v.id === "iso-ne")!.world_to_pixel);
    expect(ne[0]).toBeGreaterThan(0); expect(ne[1]).toBeGreaterThan(0);
  });
});

describe("views.json", () => {
  const good = { geometry_version: 1, map: "teacup", bounds: { min: [0, 0, 0], max: [1, 1, 1] }, views: [{ id: "top", kind: "ortho", width: 8, height: 8, world_to_pixel: TOP, up: "north" }] };

  test("reads a good file", () => {
    const v = parseViews(good)!;
    expect(v.views[0]!.world_to_pixel).toEqual(TOP);
    expect(v.views[0]!.up).toBe("north");
  });

  test("refuses another version, missing bounds, or no usable view", () => {
    expect(parseViews({ ...good, geometry_version: 2 })).toBeNull();
    expect(parseViews({ ...good, bounds: undefined })).toBeNull();
    expect(parseViews({ ...good, views: [] })).toBeNull();
    expect(parseViews(null)).toBeNull();
  });

  test("drops a view with a bad matrix, size or id but keeps the rest", () => {
    const v = parseViews({ ...good, views: [
      { id: "iso-ne", width: 8, height: 8, world_to_pixel: [[1, 2, 3], [4, 5, 6, 7]] },
      { id: "../x", width: 8, height: 8, world_to_pixel: TOP },
      { id: "iso-nw", width: 0, height: 8, world_to_pixel: TOP },
      { id: "iso-sw", width: 8, height: 8, world_to_pixel: [[1, 2, 3, NaN], [4, 5, 6, 7]] },
      good.views[0],
    ] })!;
    expect(v.views.map((x) => x.id)).toEqual(["top"]);
  });

  test("names and files", () => {
    expect(viewImageFile("map/teacup/views.json", { id: "iso-ne" })).toBe("map/teacup/views/iso-ne.png");
    expect(viewImageFile("map/teacup/views.json", { id: "top", file: "views/top-v2.png" })).toBe("map/teacup/views/top-v2.png");
    // A view's own file name is kept only when it stays beside views.json.
    const v = parseViews({ ...good, views: [{ ...good.views[0], file: "views/top.png" }, { ...good.views[0], id: "iso-ne", file: "../../x.png" }] })!;
    expect(v.views.map((x) => x.file)).toEqual(["views/top.png", undefined]);
    expect(viewLabel("top")).toBe("Top");
    expect(viewLabel("iso-sw")).toBe("SW");
    expect(mapStem("maps\\Tea Cup.map")).toBe("tea cup");
  });
});

describe("axes", () => {
  test("game (x, y, z) is glTF (x, z, -y), and back", () => {
    expect(gameToGl(1, 2, 3)).toEqual([1, 3, -2]);
    expect(glToGame(...gameToGl(1, 2, 3))).toEqual([1, 2, 3]);
    // Up stays up; north (+y) goes into the screen (-z).
    expect(gameToGl(0, 0, 1)).toEqual([0, 1, -0]);
    expect(gameToGl(0, 1, 0)).toEqual([0, 0, -1]);
  });

  test("the conversion keeps handedness: x cross y is still z", () => {
    const x = gameToGl(1, 0, 0), y = gameToGl(0, 1, 0), z = gameToGl(0, 0, 1);
    const c = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    expect(c.map((v) => v + 0)).toEqual(z.map((v) => v + 0));
  });

  test("a yaw about game z is the same angle about glTF y", () => {
    for (const yaw of [0, 0.7, Math.PI / 2, -2.1]) {
      // A model facing +x, turned by the angle about y…
      const a = yawToGl(yaw);
      const turned = [Math.cos(a), 0, -Math.sin(a)];
      // …faces where the game's yaw faces.
      const want = gameToGl(Math.cos(yaw), Math.sin(yaw), 0);
      turned.forEach((v, i) => expect(v).toBeCloseTo(want[i]!, 9));
    }
  });
});

describe("manifest", () => {
  test("reads per-map files from a maps table", () => {
    const m = parseMapFiles({ maps: { Teacup: { geometry: "map/teacup/map.glb", views: "map/teacup/views.json" } } });
    expect(m.teacup).toEqual({ geometry: "map/teacup/map.glb", views: "map/teacup/views.json" });
  });

  test("reads them off the map's preview image entry too", () => {
    const m = parseMapFiles({ images: { "map/orchard/preview": { file: "map/orchard.png", width: 1, height: 1, geometry: "map/orchard/map.glb", views: "map/orchard/views.json" } } });
    expect(m.orchard).toEqual({ geometry: "map/orchard/map.glb", views: "map/orchard/views.json" });
  });

  test("refuses files outside the art directory", () => {
    const m = parseMapFiles({ maps: { a: { geometry: "../x.glb", views: "/etc/views.json" }, b: { geometry: "https://x/y.glb" }, c: { views: "map/c/views.json" } } });
    expect(m.a).toBeUndefined();
    expect(m.b).toBeUndefined();
    expect(m.c).toEqual({ views: "map/c/views.json" });
  });

  test("the art manifest keeps them", () => {
    const m = parseManifest({ version: 1, images: {}, maps: { teacup: { geometry: "map/teacup/map.glb" } } })!;
    expect(m.maps?.teacup?.geometry).toBe("map/teacup/map.glb");
    expect(parseManifest({ version: 1, images: {} })!.maps).toEqual({});
  });
});

describe("pan and zoom", () => {
  test("fits a picture in a box, centred", () => {
    expect(fitPicture(400, 300, 1000, 1000)).toEqual({ x: 50, y: 0, k: 0.3 });
    expect(fitPicture(400, 300, 1000, 1000, 10).k).toBeCloseTo(0.28, 9);
  });

  test("zooming keeps the point under the pointer where it is", () => {
    const p = { x: 50, y: 0, k: 0.3 };
    const z = zoomAbout(p, 120, 200, 2, 0.3, 2.4);
    expect(z.k).toBeCloseTo(0.6, 9);
    // The picture pixel under (120, 200) before and after.
    expect((120 - z.x) / z.k).toBeCloseTo((120 - p.x) / p.k, 9);
    expect((200 - z.y) / z.k).toBeCloseTo((200 - p.y) / p.k, 9);
  });

  test("zoom stays inside its limits", () => {
    const p = { x: 0, y: 0, k: 1 };
    expect(zoomAbout(p, 0, 0, 100, 0.5, 4).k).toBe(4);
    expect(zoomAbout(p, 0, 0, 0.001, 0.5, 4).k).toBe(0.5);
  });

  test("a small picture stays centred; a big one can't be dragged off", () => {
    expect(clampPan({ x: -500, y: 900, k: 0.1 }, 400, 300, 1000, 1000)).toEqual({ x: 150, y: 100, k: 0.1 });
    expect(clampPan({ x: 80, y: -5000, k: 1 }, 400, 300, 1000, 1000)).toEqual({ x: 0, y: -700, k: 1 });
  });

  test("easing closes in without overshooting, then lands exactly", () => {
    const target = { x: 100, y: -40, k: 2 };
    let cur = { x: 0, y: 0, k: 1 };
    const one = easePanZoom(cur, target, 16);
    expect(one.x).toBeGreaterThan(0); expect(one.x).toBeLessThan(100);
    expect(one.k).toBeGreaterThan(1); expect(one.k).toBeLessThan(2);
    for (let i = 0; i < 120; i++) cur = easePanZoom(cur, target, 16);
    expect(cur).toEqual(target);
    // The same total time in bigger steps ends in the same place.
    expect(easePanZoom({ x: 0, y: 0, k: 1 }, target, 0)).toEqual({ x: 0, y: 0, k: 1 });
  });
});

describe("mock arena", () => {
  test("triangulates a concave polygon into n - 2 triangles covering its area", () => {
    const l: [number, number][] = [[0, 0], [4, 0], [4, 1], [1, 1], [1, 4], [0, 4]];
    const tris = triangulate(l);
    expect(tris.length).toBe(4);
    const area = tris.reduce((s, [a, b, c]) => s + Math.abs((l[b]![0] - l[a]![0]) * (l[c]![1] - l[a]![1]) - (l[b]![1] - l[a]![1]) * (l[c]![0] - l[a]![0])) / 2, 0);
    expect(area).toBeCloseTo(7, 9);
  });

  test("the glb is well formed and already in glTF axes", () => {
    const arena = buildArena("teacup")!;
    const glb = encodeGlb(arena);
    const dv = new DataView(glb.buffer, glb.byteOffset, glb.byteLength);
    expect(dv.getUint32(0, true)).toBe(0x46546c67);
    expect(dv.getUint32(8, true)).toBe(glb.length);
    const jsonLen = dv.getUint32(12, true);
    expect(jsonLen % 4).toBe(0);
    const json = JSON.parse(new TextDecoder().decode(glb.subarray(20, 20 + jsonLen)));
    expect(json.nodes[0].name).toBe("structure");
    expect(json.asset.extras.geometry_version).toBe(1);
    expect(dv.getUint32(20 + jsonLen, true)).toBe(json.buffers[0].byteLength);
    // POSITION bounds across primitives are the game bounds turned to glTF: y is game z, z is -game y.
    const pos = json.meshes[0].primitives.map((p: { attributes: { POSITION: number } }) => json.accessors[p.attributes.POSITION]);
    const lo = (i: number) => Math.min(...pos.map((a: { min: number[] }) => a.min[i]!)), hi = (i: number) => Math.max(...pos.map((a: { max: number[] }) => a.max[i]!));
    const b = arena.bounds;
    expect(lo(0)).toBeCloseTo(b.min[0], 3); expect(hi(0)).toBeCloseTo(b.max[0], 3);
    expect(lo(1)).toBeCloseTo(b.min[2], 3); expect(hi(1)).toBeCloseTo(b.max[2], 3);
    expect(lo(2)).toBeCloseTo(-b.max[1], 3); expect(hi(2)).toBeCloseTo(-b.min[1], 3);
    for (const m of json.materials) expect(["floor", "wall", "metal", "foliage", "default"]).toContain(m.name);
  });

  test("serves a glb, views.json and a picture per view for maps with an arena, and nothing else", () => {
    const g = mockMapGeometry();
    expect(Object.keys(g.maps).sort()).toEqual(["orchard", "teacup"]);
    expect(g.file("map/teacup/map.glb")!.type).toBe("model/gltf-binary");
    expect(parseViews(JSON.parse(new TextDecoder().decode(g.file("map/orchard/views.json")!.bytes)))!.map).toBe("orchard");
    const png = g.file("map/teacup/views/top.png")!;
    expect(png.type).toBe("image/png");
    expect([...png.bytes.subarray(1, 4)]).toEqual([0x50, 0x4e, 0x47]);
    expect(g.file("map/lighthouse/map.glb")).toBeNull();
    expect(g.file("map/teacup/views/side.png")).toBeNull();
    expect(g.file("map/teacup/../x")).toBeNull();
  });
});
