// `bun run banner`: draws the stats site's banners from the owner's own
// extracted game art (docs/live/live-art.md, "Stats banners").
//
//   bun run banner [--art DIR] [--chrome PATH] [--shots DIR]
//
// --art is the art `h2 live assets extract` wrote (default: game-art/ in the
// server directory). The banners go into it as banner/<id>.jpg and are added
// to its manifest.json as `banner/<id>`, so the server, and a publish of the
// art, carry them like the rest. Run it again after each extraction (that
// starts the directory afresh). --shots writes every shot alone to DIR, for
// checking. A headless Chrome draws them: --chrome, else $CHROME, else the
// usual install. Nothing here reads the disc or writes into the repository.

import { createServer, type Plugin } from "vite";
import solid from "vite-plugin-solid";
import { mkdtempSync, mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync, existsSync, createReadStream } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, normalize, resolve, sep, extname } from "node:path";
import { COMPOSITIONS, SHOTS, check, type Shot } from "./plan";
import { applyBanners, type Manifest } from "./manifest";
import type { BannerRequest, BannerResult } from "./page";

const WEB = resolve(import.meta.dir, "..");

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

function defaultArt(): string {
  const data = process.env.H2_LIVE_DATA ?? (process.platform === "darwin"
    ? join(homedir(), "Library/Application Support/h2recomp/live/server")
    : join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local/share"), "h2recomp/live/server"));
  return join(data, "game-art");
}

function defaultChrome(): string {
  if (process.env.CHROME) return process.env.CHROME;
  const found = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
  ].find((p) => existsSync(p));
  if (!found) throw new Error("no Chrome found: pass --chrome PATH or set $CHROME");
  return found;
}

const TYPES: Record<string, string> = { ".json": "application/json", ".png": "image/png", ".jpg": "image/jpeg", ".glb": "model/gltf-binary" };

/** Serves the art directory at /art/game/, as the server does. */
function serveArt(dir: string): Plugin {
  return {
    name: "banner-art",
    configureServer(server) {
      server.middlewares.use("/art/game", (req, res, next) => {
        const rel = decodeURIComponent((req.url ?? "/").split("?")[0]!);
        const path = normalize(join(dir, rel));
        if (!path.startsWith(dir + sep) || !existsSync(path) || !statSync(path).isFile()) return next();
        res.setHeader("content-type", TYPES[extname(path)] ?? "application/octet-stream");
        createReadStream(path).pipe(res);
      });
    },
  };
}

/** A minimal DevTools protocol client over one WebSocket. */
class Cdp {
  private id = 0;
  private waiting = new Map<number, { ok: (v: any) => void; fail: (e: Error) => void }>();
  private constructor(private ws: WebSocket) {
    ws.onmessage = (e) => {
      const m = JSON.parse(String(e.data));
      // The page's console, for --verbose.
      if (m.method === "Runtime.consoleAPICalled" && process.argv.includes("--verbose")) console.error("page:", ...m.params.args.map((a: any) => a.value ?? a.description));
      const w = m.id !== undefined ? this.waiting.get(m.id) : undefined;
      if (!w) return;
      this.waiting.delete(m.id);
      if (m.error) w.fail(new Error(m.error.message)); else w.ok(m.result);
    };
  }
  static open(url: string): Promise<Cdp> {
    return new Promise((ok, fail) => {
      const ws = new WebSocket(url);
      ws.onopen = () => ok(new Cdp(ws));
      ws.onerror = () => fail(new Error(`could not reach ${url}`));
    });
  }
  send(method: string, params: object = {}, sessionId?: string): Promise<any> {
    const id = ++this.id;
    this.ws.send(JSON.stringify({ id, method, params, sessionId }));
    return new Promise((ok, fail) => this.waiting.set(id, { ok, fail }));
  }
  close() { this.ws.close(); }
}

async function launchChrome(path: string, profile: string): Promise<{ proc: ReturnType<typeof Bun.spawn>; ws: string }> {
  const proc = Bun.spawn([path,
    "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
    "--no-first-run", "--no-default-browser-check", "--ignore-gpu-blocklist", "--enable-webgl",
    "--hide-scrollbars", "--mute-audio", "--window-size=1920,1080", "about:blank"],
  { stdout: "ignore", stderr: "pipe" });
  const reader = proc.stderr.getReader();
  let text = "";
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const { value, done } = await reader.read();
    if (done) break;
    text += new TextDecoder().decode(value);
    const m = /DevTools listening on (ws:\/\/\S+)/.exec(text);
    if (m) { reader.releaseLock(); return { proc, ws: m[1]! }; }
  }
  proc.kill();
  throw new Error(`Chrome did not start:\n${text}`);
}

function dataUrlBytes(url: string): Buffer {
  return Buffer.from(url.slice(url.indexOf(",") + 1), "base64");
}

async function main() {
  const art = resolve(arg("--art") ?? defaultArt());
  const shotsDir = arg("--shots");
  const problems = check(SHOTS, COMPOSITIONS);
  if (problems.length) throw new Error(`the banner plan has problems:\n  ${problems.join("\n  ")}`);
  const manifestPath = join(art, "manifest.json");
  if (!existsSync(manifestPath)) throw new Error(`${art} has no manifest.json: extract the art first (h2 live assets extract)`);

  const server = await createServer({
    configFile: false, root: WEB, logLevel: "warn",
    plugins: [solid(), serveArt(art)],
    server: { host: "127.0.0.1", port: 0, strictPort: false, hmr: false },
  });
  await server.listen();
  const addr = server.httpServer!.address();
  const port = typeof addr === "object" && addr ? addr.port : 0;
  const profile = mkdtempSync(join(tmpdir(), "h2-banner-chrome-"));
  const chrome = await launchChrome(arg("--chrome") ?? defaultChrome(), profile);
  let cdp: Cdp | null = null;
  try {
    cdp = await Cdp.open(chrome.ws);
    const { targetId } = await cdp.send("Target.createTarget", { url: `http://127.0.0.1:${port}/banner/index.html` });
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
    await cdp.send("Runtime.enable", {}, sessionId);
    const evaluate = async (expression: string) => {
      const r = await cdp!.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, sessionId);
      if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
      return r.result.value;
    };
    const ready = Date.now() + 60_000;
    while (!(await evaluate("window.bannerReady === true"))) {
      if (Date.now() > ready) throw new Error("the banner page did not load");
      await Bun.sleep(200);
    }
    const sheetDir = arg("--sheet");
    if (sheetDir) {
      // Candidate shots of the maps named (default: those the plan uses).
      const maps = (arg("--maps") ?? [...new Set(SHOTS.map((s) => s.map))].join(",")).split(",").filter(Boolean);
      // --shot-file: a JSON list of shots to draw instead of the candidates.
      const file = arg("--shot-file");
      const shots: Shot[] = file ? JSON.parse(readFileSync(file, "utf8")) : [];
      // --size WxH: each shot's size (default 640x300).
      const size = (arg("--size") ?? "640x300").split("x").map(Number) as [number, number];
      const sheets = (await evaluate(`window.sheet(${JSON.stringify({ root: "/art/game/", maps, shots, width: size[0], height: size[1], columns: 4 })})`)) as Record<string, string>;
      mkdirSync(sheetDir, { recursive: true });
      for (const [map, url] of Object.entries(sheets)) writeFileSync(join(sheetDir, `${map}.jpg`), dataUrlBytes(url));
      console.log(`contact sheets in ${sheetDir}: ${Object.keys(sheets).join(", ")}`);
      return;
    }
    const req: BannerRequest = { root: "/art/game/", shots: SHOTS, compositions: COMPOSITIONS, quality: 0.86 };
    const t0 = Date.now();
    const result = (await evaluate(`window.banner(${JSON.stringify(req)})`)) as BannerResult;
    const written: Array<{ id: string; file: string; width: number; height: number }> = [];
    mkdirSync(join(art, "banner"), { recursive: true });
    for (const c of COMPOSITIONS) {
      const url = result.pictures[c.id];
      if (!url) continue;
      const file = `banner/${c.id}.jpg`;
      writeFileSync(join(art, file), dataUrlBytes(url));
      written.push({ id: c.id, file, width: c.width, height: c.height });
    }
    const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as Manifest;
    const tmp = manifestPath + ".banner";
    writeFileSync(tmp, JSON.stringify(applyBanners(manifest, written), null, 2) + "\n");
    renameSync(tmp, manifestPath);
    if (shotsDir) {
      mkdirSync(shotsDir, { recursive: true });
      for (const [k, url] of Object.entries(result.shots)) writeFileSync(join(shotsDir, k.replace("/", "-") + ".jpg"), dataUrlBytes(url));
    }
    console.log(`banners in ${join(art, "banner")} (${((Date.now() - t0) / 1000).toFixed(1)} s)`);
    for (const w of written) console.log(`  ${w.id.padEnd(8)} ${w.width} x ${w.height}  ${w.file}`);
    for (const [k, why] of Object.entries(result.skipped)) console.log(`  skipped ${k}: ${why}`);
    if (!written.length) process.exitCode = 1;
  } finally {
    cdp?.close();
    chrome.proc.kill();
    await chrome.proc.exited;
    rmSync(profile, { recursive: true, force: true });
    await server.close();
  }
}

await main();
