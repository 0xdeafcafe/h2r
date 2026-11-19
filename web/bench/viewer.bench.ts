// The 3D viewer's perf harness. Run: `bun run bench:viewer` (cd web), after
// `bun run build`. Not a test (bunfig.toml roots `bun test` at ./test).
//
//   bun run bench:viewer [--games ID,ID] [--server URL] [--runs N]
//                        [--throttle MBIT] [--seconds S] [--chrome PATH] [--out FILE]
//
// Serves the built site (dist/) on a loopback port, forwarding the API
// (/v1) and the art manifest (/art) to --server (default
// https://live.forbes.red; the art itself then comes from wherever the
// manifest redirects, as in production), and opens each game's page with
// `?view=3d&perf` in a headless Chrome over DevTools (banner/chrome.ts).
// `?perf` puts the scene on `window.h2Viewer` (Viewer3D.tsx); the scene
// leaves User Timing marks (scene.ts): `h2-viewer:map-loaded`,
// `h2-viewer:first-map-frame` and `h2-viewer:textured`.
//
// Reported per game (median of --runs cold loads, cache off):
//   load    requests and bytes on the wire (DevTools' encoded lengths) until
//           textured, split by kind; ms from navigation to the first frame
//           with the level and to fully textured
//   warm    the same page loaded again with the cache on (a second game on
//           the same map)
//   frame   with the replay playing for --seconds: draw calls, triangles,
//           programs, geometries and textures (renderer.info, every pass of
//           a frame summed), and CPU ms per frame in update() (everything
//           but drawing) and in draw() (three.js's submission; not GPU time),
//           median and 95th percentile; and the JS heap. With AO on (the
//           default) and off.
// --throttle limits the network (Mbit/s, 20 ms latency) for the load runs.
// Nothing here writes into the repository unless --out names a file there.

import { existsSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { extname, join, normalize, resolve, sep } from "node:path";
import { Cdp, defaultChrome, launchChrome } from "../banner/chrome";

const WEB = resolve(import.meta.dir, "..");
const DIST = join(WEB, "dist");

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

// cyclotron: a small multiplayer map, four players; h1_bloodgulch: a big
// open converted Halo 1 map with vehicles; 03a_oldmombasa: a campaign level
// with AI, several zones and many placed objects.
const DEFAULT_GAMES = ["17a34bbc25d71adff1f31fccd6276c67", "962db8ad2f6000eeddec921e81f0a7c3", "99b3844ce2ae173e94b8e50709776f21"];

const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp", ".woff2": "font/woff2", ".ico": "image/x-icon" };

/** dist/ with the SPA fallback, and /v1, /art, /.well-known forwarded. */
function serve(upstream: string) {
  return Bun.serve({
    hostname: "127.0.0.1", port: 0,
    async fetch(req) {
      const u = new URL(req.url);
      if (/^\/(v1|art|\.well-known)\//.test(u.pathname)) {
        const r = await fetch(upstream + u.pathname + u.search, { headers: { accept: req.headers.get("accept") ?? "*/*" }, redirect: "manual" });
        const h = new Headers(r.headers);
        // fetch has already undone any compression.
        h.delete("content-encoding"); h.delete("content-length");
        return new Response(r.status === 304 || r.status === 204 ? null : await r.arrayBuffer(), { status: r.status, headers: h });
      }
      let path = normalize(join(DIST, decodeURIComponent(u.pathname)));
      if (!path.startsWith(DIST + sep) || !existsSync(path) || !statSync(path).isFile()) path = join(DIST, "index.html");
      return new Response(Bun.file(path), { headers: { "content-type": TYPES[extname(path)] ?? "application/octet-stream" } });
    },
  });
}

interface Load { firstFrame: number; textured: number; requests: number; bytes: number; byKind: Record<string, { n: number; bytes: number }> }
interface FrameStats { census?: Record<string, { meshes: number; groups: number; tris: number }>; frames: number; calls: number; triangles: number; programs: number; geometries: number; textures: number; updateMs: [number, number]; drawMs: [number, number]; heapMB: number }

const median = (xs: number[]) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.floor(s.length / 2)]! : NaN; };
const p95 = (xs: number[]) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(s.length * 0.95))]! : NaN; };

function kindOf(url: string, mime: string): string {
  const p = new URL(url).pathname;
  if (p.endsWith(".glb")) return p.endsWith("map.glb") ? "map.glb" : p.endsWith("sky.glb") ? "sky.glb" : "model glb";
  if (/\/tex\//.test(p)) return "tex";
  if (p.startsWith("/v1/")) return "api";
  if (p.endsWith(".js")) return "js";
  if (p.endsWith("manifest.json")) return "manifest";
  if (mime.startsWith("image/")) return "image";
  return "other";
}

async function main() {
  const upstream = (arg("--server") ?? "https://live.forbes.red").replace(/\/+$/, "");
  const games = (arg("--games") ?? DEFAULT_GAMES.join(",")).split(",").filter(Boolean);
  const runs = Number(arg("--runs") ?? 3);
  const seconds = Number(arg("--seconds") ?? 8);
  const mbit = arg("--throttle") ? Number(arg("--throttle")) : null;
  if (!existsSync(join(DIST, "index.html"))) throw new Error("no dist/: run `bun run build` first");
  const server = serve(upstream);
  const origin = `http://127.0.0.1:${server.port}`;
  const profile = mkdtempSync(join(tmpdir(), "h2-viewer-bench-"));
  const chrome = await launchChrome(arg("--chrome") ?? defaultChrome(), profile);
  const cdp = await Cdp.open(chrome.ws);
  const out: Record<string, unknown> = {};
  try {
    for (const id of games) {
      const detail = await (await fetch(`${upstream}/v1/games/${id}`)).json() as { map?: string };
      const page = `${origin}/games/halo2/games/${id}?view=3d&perf`;
      const cold: Load[] = [];
      let warm: Load | null = null;
      let frame: { ao: FrameStats; noAo: FrameStats } | null = null;
      for (let r = 0; r <= runs; r++) {
        // Runs 0..runs-1 are cold; the last is warm (cache on, after a cold one).
        const isWarm = r === runs;
        const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
        const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
        const send = (m: string, p: object = {}) => cdp.send(m, p, sessionId);
        await send("Network.enable");
        await send("Runtime.enable");
        await send("Page.enable");
        await send("Network.setCacheDisabled", { cacheDisabled: !isWarm });
        if (mbit) await send("Network.emulateNetworkConditions", { offline: false, latency: 20, downloadThroughput: mbit * 125_000, uploadThroughput: mbit * 125_000 });
        const reqs = new Map<string, { url: string; mime: string; bytes: number; done: boolean; t0: number; t1: number }>();
        const sent = new Map<string, number>();
        const onEvent = (m: any) => {
          if (m.sessionId !== sessionId) return;
          if (m.method === "Network.requestWillBeSent") sent.set(m.params.requestId, m.params.timestamp);
          if (m.method === "Network.responseReceived") reqs.set(m.params.requestId, { url: m.params.response.url, mime: m.params.response.mimeType, bytes: 0, done: false, t0: sent.get(m.params.requestId) ?? m.params.timestamp, t1: 0 });
          if (m.method === "Network.loadingFinished") { const q = reqs.get(m.params.requestId); if (q) { q.bytes = m.params.encodedDataLength; q.done = true; q.t1 = m.params.timestamp; } }
        };
        cdp.listen(onEvent);
        if (isWarm) { await send("Page.navigate", { url: page }); await waitFor(send, "performance.getEntriesByName('h2-viewer:textured').length > 0", 120_000); reqs.clear(); }
        await send("Page.navigate", { url: page });
        const ok = await waitFor(send, "performance.getEntriesByName('h2-viewer:textured').length > 0", 120_000);
        if (!ok) throw new Error(`${id}: the level never finished texturing`);
        await Bun.sleep(300);
        const marks = await evaluate(send, "({ first: performance.getEntriesByName('h2-viewer:first-map-frame')[0]?.startTime ?? -1, textured: performance.getEntriesByName('h2-viewer:textured')[0].startTime })");
        const byKind: Load["byKind"] = {};
        let bytes = 0;
        for (const q of reqs.values()) {
          if (q.url.startsWith("data:")) continue;
          const k = kindOf(q.url, q.mime);
          (byKind[k] ??= { n: 0, bytes: 0 }).n++;
          byKind[k].bytes += q.bytes;
          bytes += q.bytes;
        }
        const load: Load = { firstFrame: Math.round(marks.first), textured: Math.round(marks.textured), requests: [...reqs.values()].filter((q) => !q.url.startsWith("data:")).length, bytes, byKind };
        if (isWarm) warm = load; else cold.push(load);
        if (r === 0 && process.argv.includes("--timeline")) {
          const all = [...reqs.values()].filter((q) => q.done && !q.url.startsWith("data:"));
          const t = Math.min(...all.map((q) => q.t0));
          for (const q of all.sort((a, b) => a.t0 - b.t0)) console.log(`    ${String(Math.round((q.t0 - t) * 1000)).padStart(6)} ${String(Math.round((q.t1 - t) * 1000)).padStart(6)} ${String(Math.round(q.bytes / 1024)).padStart(6)} KB  ${q.url.replace(/\?.*/, "").slice(-70)}`);
        }
        cdp.unlisten(onEvent);
        if (r === 0) {
          await send("Network.emulateNetworkConditions", { offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1 });
          const profile = process.argv.includes("--profile");
          if (profile) { await send("Profiler.enable"); await send("Profiler.setSamplingInterval", { interval: 200 }); await send("Profiler.start"); }
          const ao = await measureFrames(send, seconds, true);
          if (profile) printProfile((await send("Profiler.stop")).profile);
          frame = { ao, noAo: await measureFrames(send, seconds, false) };
        }
        await cdp.send("Target.closeTarget", { targetId });
      }
      const pick = (f: (l: Load) => number) => median(cold.map(f));
      const result = {
        map: detail.map,
        load: { firstFrameMs: pick((l) => l.firstFrame), texturedMs: pick((l) => l.textured), requests: pick((l) => l.requests), bytes: pick((l) => l.bytes), byKind: cold[0]!.byKind },
        warm: warm && { firstFrameMs: warm.firstFrame, texturedMs: warm.textured, requests: warm.requests, bytes: warm.bytes },
        frame,
      };
      out[id] = result;
      report(id, result);
    }
    const file = arg("--out");
    if (file) writeFileSync(file, JSON.stringify(out, null, 2) + "\n");
  } finally {
    cdp.close();
    chrome.proc.kill();
    await chrome.proc.exited;
    rmSync(profile, { recursive: true, force: true });
    server.stop(true);
  }
}

async function evaluate(send: (m: string, p?: object) => Promise<any>, expression: string): Promise<any> {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  return r.result.value;
}

async function waitFor(send: (m: string, p?: object) => Promise<any>, expression: string, ms: number): Promise<boolean> {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try { if (await evaluate(send, expression)) return true; } catch { /* navigating */ }
    await Bun.sleep(50);
  }
  return false;
}

/** Plays the replay and samples the scene's counts and CPU time per frame. */
async function measureFrames(send: (m: string, p?: object) => Promise<any>, seconds: number, ao: boolean): Promise<FrameStats> {
  const script = `(async () => {
    const s = window.h2Viewer;
    if (!s) throw new Error("no window.h2Viewer: is this build older than the perf hook?");
    s.setAo(${ao});
    const info = s.renderer.info;
    info.autoReset = false;
    const up = [], dr = [], counts = [];
    const update = s.update, draw = s.draw;
    let inDraw = 0;
    s.draw = function (...a) { info.reset(); const t = performance.now(); draw.apply(this, a); inDraw = performance.now() - t; counts.push([info.render.calls, info.render.triangles]); };
    s.update = function (...a) { inDraw = -1; const t = performance.now(); update.apply(this, a); const all = performance.now() - t; if (inDraw >= 0) { dr.push(inDraw); up.push(all - inDraw); } };
    // Play from the same place each time: 30% in.
    const pos = document.querySelector('input[aria-label="Replay position"]');
    if (pos) { pos.value = String(Math.round(+pos.min + (+pos.max - +pos.min) * 0.3)); pos.dispatchEvent(new Event('input', { bubbles: true })); }
    await new Promise(r => setTimeout(r, 1500));
    const play = document.querySelector('.replay-bar button.play');
    if (play && play.getAttribute('aria-label') === 'Play') play.click();
    const end = performance.now() + ${seconds * 1000};
    while (performance.now() < end) { s.invalidate(); await new Promise(r => requestAnimationFrame(r)); }
    if (play && play.getAttribute('aria-label') === 'Pause') play.click();
    s.update = update; s.draw = draw; info.autoReset = true;
    // Where the draw calls go: visible meshes by the scene child they are under.
    const census = {};
    const scene = s.scene;
    for (const top of scene.children) {
      let key = (top.name || top.type) ;
      top.traverseVisible((o) => {
        if (!o.isMesh && !o.isPoints && !o.isLine && !o.isSprite) return;
        const k = key + (o.isInstancedMesh ? ' (instanced)' : o.isSkinnedMesh ? ' (skinned)' : '');
        const c = census[k] ??= { meshes: 0, groups: 0, tris: 0 };
        c.meshes++;
        const g = o.geometry, n = Array.isArray(o.material) ? Math.max(1, g.groups.length) : 1;
        c.groups += n;
        const idx = g.index ? g.index.count : (g.attributes.position?.count ?? 0);
        c.tris += Math.round(idx / 3) * (o.isInstancedMesh ? o.count : 1);
      });
    }
    const mem = info.memory, heap = performance.memory ? performance.memory.usedJSHeapSize : 0;
    return { up, dr, counts, programs: info.programs.length, geometries: mem.geometries, textures: mem.textures, heap, census };
  })()`;
  const r = await evaluate(send, script);
  const calls = r.counts.map((c: number[]) => c[0]), tris = r.counts.map((c: number[]) => c[1]);
  const round = (x: number) => Math.round(x * 100) / 100;
  return {
    frames: r.up.length, calls: median(calls), triangles: median(tris), programs: r.programs, geometries: r.geometries, textures: r.textures,
    updateMs: [round(median(r.up)), round(p95(r.up))], drawMs: [round(median(r.dr)), round(p95(r.dr))], heapMB: round(r.heap / 1048576), census: r.census,
  };
}

/** --profile: the functions the page spent most of its own time in while playing (AO on). */
function printProfile(p: { nodes: Array<{ id: number; callFrame: { functionName: string; url: string; lineNumber: number }; hitCount?: number }>; samples: number[]; timeDeltas: number[] }) {
  const self = new Map<string, number>();
  const byId = new Map(p.nodes.map((n) => [n.id, n]));
  let total = 0;
  for (let i = 0; i < p.samples.length; i++) {
    const n = byId.get(p.samples[i]!)!;
    const dt = p.timeDeltas[i] ?? 0;
    total += dt;
    const f = n.callFrame;
    const key = `${f.functionName || "(anonymous)"} ${f.url.replace(/^.*\//, "")}:${f.lineNumber}`;
    self.set(key, (self.get(key) ?? 0) + dt);
  }
  console.log(`  profile: ${(total / 1000).toFixed(0)} ms sampled; top self time:`);
  for (const [k, us] of [...self].sort((a, b) => b[1] - a[1]).slice(0, 30)) console.log(`    ${(us / 1000).toFixed(0).padStart(7)} ms ${(100 * us / total).toFixed(1).padStart(5)}%  ${k}`);
}

function report(id: string, r: any) {
  const kb = (b: number) => `${(b / 1024).toFixed(0)} KB`;
  console.log(`\n${r.map} (${id})`);
  console.log(`  load  first frame ${r.load.firstFrameMs} ms, textured ${r.load.texturedMs} ms, ${r.load.requests} requests, ${kb(r.load.bytes)}`);
  for (const [k, v] of Object.entries(r.load.byKind as Record<string, { n: number; bytes: number }>).sort((a, b) => b[1].bytes - a[1].bytes)) console.log(`        ${k.padEnd(10)} ${String(v.n).padStart(4)}  ${kb(v.bytes)}`);
  if (r.warm) console.log(`  warm  first frame ${r.warm.firstFrameMs} ms, textured ${r.warm.texturedMs} ms, ${r.warm.requests} requests, ${kb(r.warm.bytes)}`);
  for (const [name, f] of [["AO on", r.frame?.ao], ["AO off", r.frame?.noAo]] as Array<[string, FrameStats | undefined]>) {
    if (!f) continue;
    if (process.argv.includes("--census") && f.census) for (const [k, c] of Object.entries(f.census).sort((a, b) => b[1].groups - a[1].groups)) console.log(`        ${k.slice(0, 40).padEnd(40)} ${String(c.meshes).padStart(5)} meshes ${String(c.groups).padStart(5)} draws ${String(c.tris).padStart(8)} tris`);
    console.log(`  ${name.padEnd(6)} ${f.frames} frames: ${f.calls} draw calls, ${f.triangles} triangles, ${f.programs} programs, ${f.geometries} geometries, ${f.textures} textures; update ${f.updateMs[0]} ms (p95 ${f.updateMs[1]}), draw ${f.drawMs[0]} ms (p95 ${f.drawMs[1]}); heap ${f.heapMB} MB`);
  }
}

await main();
