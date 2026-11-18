// The banner's drawing page, run in a headless browser by banner/run.ts:
// each shot drawn by the 3D viewer from the owner's extracted maps, then
// the shots laid side by side (plan.ts) and graded into one picture.
// `window.banner(...)` does it all and answers each picture as a JPEG data
// URL.

import { ViewerScene } from "../src/viewer/scene";
import type { MapFrame } from "../src/components/MapCanvas";
import { parseManifest } from "../src/lib/gameart";
import type { RosterEntry } from "../src/map/render";
import type { SampledPlayer } from "../src/state/frames";
import type { PlayerModels } from "../src/viewer/models";
import { camera, candidates, isSpotShot, layout, panelWeight, placeCast, type Composition, type Panel, type Shot } from "./plan";

export interface BannerRequest {
  /** Where the art is served, with a trailing slash. */
  root: string;
  shots: Shot[];
  compositions: Composition[];
  /** JPEG quality, 0 to 1. */
  quality: number;
}

export interface BannerResult {
  pictures: Record<string, string>;
  /** Shots that could not be drawn, and why. */
  skipped: Record<string, string>;
  /** Each shot alone, by composition and shot, for checking (data URLs). */
  shots: Record<string, string>;
}

const EMPTY: MapFrame = { players: [], markers: [], seen: null };

/** One map loaded into a viewer, drawn from shot after shot. */
class Stage {
  // Shots so far: each shot's cast gets indexes of its own, so none is
  // taken for one from an earlier shot (its model, colour, a body left).
  private shots = 0;

  private constructor(private scene: ViewerScene, private host: HTMLElement) {}

  static async open(host: HTMLElement, glb: string, models: { root: string; table: PlayerModels | null | undefined }): Promise<Stage> {
    const scene = new ViewerScene(host, { reducedMotion: true, bare: true });
    try {
      scene.setModels(models.root, models.table);
      scene.resize(960, 540);
      scene.setAo(true);
      // Teleporter and lift markers are the viewer's own drawing, not the level's.
      scene.setFeatures(false);
      if (!(await scene.loadMap(glb))) throw new Error(`could not load ${glb}`);
      await scene.texturesReady;
      await scene.skyReady();
      if (!scene.levelBounds()) throw new Error("the level has no bounds");
      return new Stage(scene, host);
    } catch (e) {
      scene.dispose();
      throw e;
    }
  }

  async draw(shot: Shot, w: number, h: number): Promise<HTMLCanvasElement> {
    const scene = this.scene;
    this.host.style.width = `${w}px`;
    this.host.style.height = `${h}px`;
    scene.resize(w, h);
    const cam = camera(shot, scene.levelBounds()!, scene.spots());
    if (!cam) throw new Error(`the level has no ${"spot" in shot ? shot.spot : "spot"}`);
    scene.aim(cam.eye, cam.at, shot.fov);
    // The cast stands on the spot's floor.
    const floor = isSpotShot(shot) ? scene.spots().get(shot.spot)?.at[2] : undefined;
    const cast = isSpotShot(shot) && shot.cast && floor !== undefined ? placeCast(shot.cast, cam.eye, cam.at, floor) : [];
    const roster = new Map<number, RosterEntry>();
    const base = 16 * this.shots++;
    const players: SampledPlayer[] = cast.map((c, k) => {
      const i = base + k;
      roster.set(i, { name: "", colour: c.colour });
      // Each on the floor under them (the ground rises and falls), else the spot's.
      const z = scene.floorBelow(c.x, c.y, c.z + 3) ?? c.z;
      return { index: i, alive: true, x: c.x, y: c.y, z, yaw: c.yaw, vehicle: null, species: c.species, weapon: c.weapon ?? null, stance: c.crouch ? "crouch" : "stand", speed: 0 };
    });
    const frame: MapFrame = { ...EMPTY, players };
    // Frames until whatever settles over frames has (the sky's fit, the
    // fog, the zone in play), and the cast's models have loaded.
    let now = 1000;
    const frames = players.length ? 40 : 4;
    for (let i = 0; i < frames; i++) {
      scene.invalidate();
      scene.update((now += 16), frame, roster, null, true);
      await new Promise((r) => setTimeout(r, players.length ? 60 : 0));
      await new Promise((r) => requestAnimationFrame(r));
    }
    scene.invalidate();
    scene.update((now += 16), frame, roster, null, true);
    // Read back in the same task as the draw (the buffer isn't kept).
    const out = document.createElement("canvas");
    out.width = w; out.height = h;
    out.getContext("2d")!.drawImage(scene.renderer.domElement, 0, 0, w, h);
    return out;
  }

  spots() { return this.scene.spots(); }

  close() { this.scene.dispose(); }
}

/** The panel's shot, weighted by its seams, added into `into`. */
function addPanel(into: CanvasRenderingContext2D, c: Composition, p: Panel, shot: HTMLCanvasElement) {
  const { x, width: w, height: h } = p.box;
  const tmp = document.createElement("canvas");
  tmp.width = w; tmp.height = h;
  const t = tmp.getContext("2d", { willReadFrequently: true })!;
  t.drawImage(shot, 0, 0, w, h);
  const img = t.getImageData(0, 0, w, h);
  const d = img.data;
  for (let yy = 0; yy < h; yy++) {
    for (let xx = 0; xx < w; xx++) {
      const k = (yy * w + xx) * 4 + 3;
      d[k] = Math.round(d[k]! * panelWeight(p, x + xx, yy, c.height));
    }
  }
  t.putImageData(img, 0, 0);
  into.globalCompositeOperation = "lighter";
  into.drawImage(tmp, x, 0);
  into.globalCompositeOperation = "source-over";
}

/** One look over the whole picture, so shots of different maps read as one:
 *  a cool tint, a thin bright rule along each seam, darker edges, and a
 *  fade at the bottom into the page's navy (the columns ride up over it). */
function grade(g: CanvasRenderingContext2D, c: Composition, panels: Panel[]) {
  const { width: w, height: h } = c;
  g.globalCompositeOperation = "soft-light";
  g.fillStyle = "#2a4d78";
  g.fillRect(0, 0, w, h);
  g.globalCompositeOperation = "source-over";
  // Seam rules.
  g.lineWidth = Math.max(1, h / 180);
  for (const p of panels) {
    if (!p.right) continue;
    const s = p.right;
    const line = g.createLinearGradient(0, 0, 0, h);
    line.addColorStop(0, "rgba(190, 225, 255, 0)");
    line.addColorStop(0.35, "rgba(190, 225, 255, .28)");
    line.addColorStop(1, "rgba(190, 225, 255, 0)");
    g.strokeStyle = line;
    g.beginPath(); g.moveTo(s.top, 0); g.lineTo(s.bottom, h); g.stroke();
  }
  // Edges: dark at the far left and right, the right more under the mark.
  const side = g.createLinearGradient(0, 0, w, 0);
  side.addColorStop(0, "rgba(5, 12, 22, .75)");
  side.addColorStop(0.12, "rgba(5, 12, 22, 0)");
  side.addColorStop(c.mark === "right" ? 0.72 : 0.88, "rgba(5, 12, 22, 0)");
  side.addColorStop(1, `rgba(5, 12, 22, ${c.mark === "right" ? 0.82 : 0.75})`);
  g.fillStyle = side;
  g.fillRect(0, 0, w, h);
  const top = g.createLinearGradient(0, 0, 0, h);
  top.addColorStop(0, "rgba(5, 12, 22, .35)");
  top.addColorStop(0.25, "rgba(5, 12, 22, 0)");
  top.addColorStop(0.55, "rgba(5, 12, 22, 0)");
  top.addColorStop(1, "rgba(5, 12, 22, .92)");
  g.fillStyle = top;
  g.fillRect(0, 0, w, h);
  // Fine scan lines, as the reference's banners have.
  g.fillStyle = "rgba(0, 0, 0, .12)";
  const pitch = Math.max(2, Math.round(h / 120));
  for (let y = 0; y < h; y += pitch * 2) g.fillRect(0, y, w, pitch);
}

async function readManifest(root: string) {
  const manifest = parseManifest(await (await fetch(root + "manifest.json")).json());
  if (!manifest) throw new Error("the art's manifest.json is not readable");
  return manifest;
}

/** Draws every listed (shot, size), a map at a time; failures by key. */
async function drawAll(root: string, jobs: Array<{ key: string; shot: Shot; width: number; height: number }>) {
  const manifest = await readManifest(root);
  const host = document.getElementById("host")!;
  const drawn = new Map<string, HTMLCanvasElement>();
  const failed = new Map<string, string>();
  const maps = [...new Set(jobs.map((j) => j.shot.map))];
  for (const map of maps) {
    const mine = jobs.filter((j) => j.shot.map === map);
    const files = manifest.maps?.[map];
    if (!files?.geometry) { for (const j of mine) failed.set(j.key, `no map/${map} in the art`); continue; }
    let stage: Stage | null = null;
    try {
      stage = await Stage.open(host, root + files.geometry, { root, table: manifest.models });
      for (const j of mine) drawn.set(j.key, await stage.draw(j.shot, j.width, j.height));
    } catch (e) {
      for (const j of mine) if (!drawn.has(j.key)) failed.set(j.key, e instanceof Error ? e.message : String(e));
    } finally {
      stage?.close();
    }
  }
  return { drawn, failed };
}

async function banner(req: BannerRequest): Promise<BannerResult> {
  const byId = new Map(req.shots.map((s) => [s.id, s]));
  const plans = req.compositions.map((c) => ({ c, panels: layout(c) }));
  const jobs = plans.flatMap(({ c, panels }) => panels.map((p) => ({ key: `${c.id}/${p.shot}`, shot: byId.get(p.shot)!, width: p.box.width, height: p.box.height })));
  const { drawn, failed } = await drawAll(req.root, jobs);
  const result: BannerResult = { pictures: {}, skipped: Object.fromEntries(failed), shots: {} };
  for (const [k, pic] of drawn) result.shots[k] = pic.toDataURL("image/jpeg", 0.85);
  for (const { c, panels } of plans) {
    // A picture with gaps would look broken: the page falls back instead.
    if (panels.some((p) => !drawn.has(`${c.id}/${p.shot}`))) continue;
    const out = document.createElement("canvas");
    out.width = c.width; out.height = c.height;
    const g = out.getContext("2d")!;
    g.fillStyle = "#000";
    g.fillRect(0, 0, c.width, c.height);
    for (const p of panels) addPanel(g, c, p, drawn.get(`${c.id}/${p.shot}`)!);
    grade(g, c, panels);
    result.pictures[c.id] = out.toDataURL("image/jpeg", req.quality);
  }
  return result;
}

/** A contact sheet per map of candidate shots (plan.ts `candidates`, or
 *  the shots given), labelled, for choosing new shots (`bun run banner
 *  --sheet DIR`). */
async function sheet(req: { root: string; maps: string[]; shots: Shot[]; width: number; height: number; columns: number }): Promise<Record<string, string>> {
  const manifest = await readManifest(req.root);
  const host = document.getElementById("host")!;
  const out: Record<string, string> = {};
  for (const map of req.maps) {
    const files = manifest.maps?.[map];
    if (!files?.geometry) continue;
    let stage: Stage | null = null;
    try {
      stage = await Stage.open(host, req.root + files.geometry, { root: req.root, table: manifest.models });
      const given = req.shots.filter((s) => s.map === map);
      const mine = given.length ? given : candidates(map, stage.spots());
      const rows = Math.max(1, Math.ceil(mine.length / req.columns));
      const c = document.createElement("canvas");
      c.width = req.width * req.columns; c.height = req.height * rows;
      const g = c.getContext("2d")!;
      g.fillStyle = "#111"; g.fillRect(0, 0, c.width, c.height);
      g.font = "bold 18px sans-serif";
      for (const [i, s] of mine.entries()) {
        const x = (i % req.columns) * req.width, y = Math.floor(i / req.columns) * req.height;
        let label = s.id;
        try { g.drawImage(await stage.draw(s, req.width, req.height), x, y); } catch (e) { label += `: ${e instanceof Error ? e.message : e}`; }
        g.fillStyle = "rgba(0,0,0,.6)"; g.fillRect(x, y, req.width, 26);
        g.fillStyle = "#fff"; g.fillText(label, x + 6, y + 19);
        g.strokeStyle = "#000"; g.strokeRect(x, y, req.width, req.height);
      }
      out[map] = c.toDataURL("image/jpeg", 0.85);
    } catch (e) {
      console.error(map, e);
    } finally {
      stage?.close();
    }
  }
  return out;
}

Object.assign(window, { banner, sheet, bannerReady: true });
