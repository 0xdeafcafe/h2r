// `bun run brand`: draws the web app's icons, its manifest and the default
// link-preview card into web/public from the project's own app icon art
// (plan.ts says what and from where). Run it again after the icon art or
// a name in src/brand.ts changes, and commit what it writes: the art is
// ours (art/app-icon/README.md), nothing here comes from the game.
//
//   bun run brand [--chrome PATH] [--out DIR] [--preview DIR]
//
// --out writes somewhere other than web/public; --preview also writes each
// icon at 8x (nearest neighbour, to see the pixels) and a sheet of them.
// A headless Chrome draws: --chrome, else $CHROME, else the usual install.

import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { SITE_NAME, STATS_NAME } from "../src/brand";
import { Cdp, defaultChrome, launchChrome } from "../banner/chrome";
import { CARD, ICONS, ICO_PARTS, SOURCES, faviconSvg, ico, outputs, webManifest, type Source } from "./plan";

const WEB = resolve(import.meta.dir, "..");
const ROOT = resolve(WEB, "..");

function arg(name: string): string | undefined {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

const TYPES: Record<string, string> = { ".png": "image/png", ".svg": "image/svg+xml", ".ttf": "font/ttf" };

/** The drawing page and the sources, by name only: nothing else is served. */
function serve() {
  return Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch(req) {
      const path = new URL(req.url).pathname;
      if (path === "/") return new Response(Bun.file(join(import.meta.dir, "page.html")), { headers: { "content-type": "text/html" } });
      const name = path.startsWith("/src/") ? (path.slice(5) as Source) : null;
      if (name && name in SOURCES) {
        const file = join(ROOT, SOURCES[name]);
        return new Response(Bun.file(file), { headers: { "content-type": TYPES[file.slice(file.lastIndexOf("."))] ?? "application/octet-stream" } });
      }
      return new Response("not found", { status: 404 });
    },
  });
}

const bytes = (dataUrl: string) => new Uint8Array(Buffer.from(dataUrl.slice(dataUrl.indexOf(",") + 1), "base64"));

function write(dir: string, file: string, data: Uint8Array | string) {
  mkdirSync(dirname(join(dir, file)), { recursive: true });
  writeFileSync(join(dir, file), data);
}

async function main() {
  const out = resolve(arg("--out") ?? join(WEB, "public"));
  const preview = arg("--preview");
  const server = serve();
  const base = `http://127.0.0.1:${server.port}/`;
  const profile = mkdtempSync(join(tmpdir(), "h2-brand-chrome-"));
  const chrome = await launchChrome(arg("--chrome") ?? defaultChrome(), profile);
  let cdp: Cdp | null = null;
  try {
    cdp = await Cdp.open(chrome.ws);
    const open = async (url: string, width: number, height: number) => {
      const { targetId } = await cdp!.send("Target.createTarget", { url: "about:blank" });
      const { sessionId } = await cdp!.send("Target.attachToTarget", { targetId, flatten: true });
      await cdp!.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false }, sessionId);
      await cdp!.send("Emulation.setDefaultBackgroundColorOverride", { color: { r: 0, g: 0, b: 0, a: 0 } }, sessionId);
      await cdp!.send("Page.enable", {}, sessionId);
      await cdp!.send("Page.navigate", { url }, sessionId);
      const evaluate = async (expression: string) => {
        const r = await cdp!.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, sessionId);
        if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
        return r.result.value;
      };
      const deadline = Date.now() + 30_000;
      while (!(await evaluate("window.brandReady === true").catch(() => false))) {
        if (Date.now() > deadline) throw new Error("the drawing page did not load");
        await Bun.sleep(100);
      }
      return { sessionId, evaluate };
    };

    // The icons.
    const page = await open(base, 600, 600);
    const drawn = new Map<string, Uint8Array>();
    for (const icon of ICONS) {
      const spec = { src: `/src/${icon.source}`, size: icon.size, svg: SOURCES[icon.source].endsWith(".svg"), scale: icon.scale ?? 1 };
      drawn.set(icon.file, bytes(await page.evaluate(`window.icon(${JSON.stringify(spec)})`)));
    }
    write(out, "favicon.ico", ico(ICO_PARTS.map((i) => ({ size: i.size, png: drawn.get(i.file)! }))));
    write(out, "favicon.svg", faviconSvg(readFileSync(join(ROOT, SOURCES.micro), "utf8")));
    for (const icon of ICONS) if (!ICO_PARTS.includes(icon)) write(out, icon.file, drawn.get(icon.file)!);
    write(out, "manifest.webmanifest", JSON.stringify(webManifest(), null, 2) + "\n");

    // The card: the page laid out at its size, photographed.
    const q = new URLSearchParams({ card: "1", site: SITE_NAME, stats: STATS_NAME });
    const card = await open(`${base}?${q}`, CARD.width, CARD.height);
    await card.evaluate("window.cardReady");
    const shot = await cdp.send("Page.captureScreenshot", { format: "jpeg", quality: CARD.quality, clip: { x: 0, y: 0, width: CARD.width, height: CARD.height, scale: 1 } }, card.sessionId);
    write(out, CARD.file, Buffer.from(shot.data, "base64"));

    if (preview) {
      for (const [file, png] of drawn) write(preview, file, png);
      write(preview, CARD.file.replace("/", "-"), Buffer.from(shot.data, "base64"));
    }
    console.log(`brand: wrote ${outputs().length} files into ${out}`);
    for (const f of outputs()) console.log(`  ${f}`);
  } finally {
    cdp?.close();
    chrome.proc.kill();
    await chrome.proc.exited;
    rmSync(profile, { recursive: true, force: true });
    server.stop(true);
  }
}

await main();
