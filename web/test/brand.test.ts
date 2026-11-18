import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { HTML_NAMES, SITE_NAME, STATS_NAME, THEME, fillNames } from "../src/brand";
import { CARD, ICONS, ICO_PARTS, SOURCES, faviconSvg, ico, outputs, webManifest } from "../brand/plan";

const WEB = resolve(import.meta.dir, "..");
const ROOT = resolve(WEB, "..");
const PUBLIC = join(WEB, "public");

/** A PNG's width and height, from its IHDR. */
function pngSize(b: Uint8Array): [number, number] {
  const v = new DataView(b.buffer, b.byteOffset, b.byteLength);
  expect([...b.slice(1, 4)]).toEqual([0x50, 0x4e, 0x47]);
  return [v.getUint32(16), v.getUint32(20)];
}

/** A baseline or progressive JPEG's width and height, from its SOF. */
function jpegSize(b: Uint8Array): [number, number] {
  let i = 2;
  while (i < b.length) {
    const marker = b[i + 1]!;
    const len = (b[i + 2]! << 8) | b[i + 3]!;
    if (marker >= 0xc0 && marker <= 0xc2) return [(b[i + 7]! << 8) | b[i + 8]!, (b[i + 5]! << 8) | b[i + 6]!];
    i += 2 + len;
  }
  throw new Error("no SOF");
}

describe("brand names", () => {
  test("index.html's placeholders are filled and escaped", () => {
    const html = fillNames(`<title>%H2_SITE_NAME%</title><meta name="h2-stats-name" content="%H2_STATS_NAME%" />`);
    expect(html).toBe(`<title>${SITE_NAME}</title><meta name="h2-stats-name" content="${STATS_NAME}" />`);
    expect(fillNames("%H2_SITE_NAME%".repeat(2))).toBe(SITE_NAME.repeat(2));
    expect(Object.keys(HTML_NAMES)).toEqual(["%H2_SITE_NAME%", "%H2_STATS_NAME%"]);
  });

  test("index.html has the block the server replaces and the names it reads", () => {
    const html = readFileSync(join(WEB, "index.html"), "utf8");
    const begin = html.indexOf("<!-- page-meta -->"), end = html.indexOf("<!-- /page-meta -->");
    expect(begin).toBeGreaterThan(0);
    expect(end).toBeGreaterThan(begin);
    // Everything that differs per page is inside the block.
    const block = html.slice(begin, end), rest = html.slice(0, begin) + html.slice(end);
    for (const tag of ["<title>", 'name="description"', 'name="theme-color"', 'property="og:', 'name="twitter:']) {
      expect(block).toContain(tag);
      expect(rest).not.toContain(tag);
    }
    expect(rest).toContain('<meta name="application-name" content="%H2_SITE_NAME%" />');
    expect(rest).toContain('<meta name="h2-stats-name" content="%H2_STATS_NAME%" />');
    for (const href of ["/favicon.ico", "/favicon.svg", "/apple-touch-icon.png", "/manifest.webmanifest"]) {
      expect(rest).toContain(`href="${href}"`);
      expect(existsSync(join(PUBLIC, href))).toBe(true);
    }
    expect(block).toContain(`content="/${CARD.file}"`);
  });

  test("the server's colours are the app's", () => {
    const rust = readFileSync(join(ROOT, "crates/live-server/src/page_meta.rs"), "utf8");
    expect(rust).toContain(`DASH_THEME: &str = "${THEME.dash}"`);
    expect(rust).toContain(`STATS_THEME: &str = "${THEME.stats}"`);
    expect(rust).toContain(`DEFAULT_IMAGE: &str = "/${CARD.file}"`);
  });
});

describe("icons", () => {
  test("an ICO holds its PNGs with a directory that points at them", () => {
    const a = new Uint8Array([1, 2, 3]), b = new Uint8Array([4, 5]);
    const out = ico([{ size: 16, png: a }, { size: 256, png: b }]);
    const v = new DataView(out.buffer);
    expect([v.getUint16(0, true), v.getUint16(2, true), v.getUint16(4, true)]).toEqual([0, 1, 2]);
    expect([out[6], out[7], v.getUint16(12, true), v.getUint32(14, true), v.getUint32(18, true)]).toEqual([16, 16, 32, 3, 38]);
    expect([out[22], out[23], v.getUint32(30, true), v.getUint32(34, true)]).toEqual([0, 0, 2, 41]);
    expect([...out.slice(38)]).toEqual([1, 2, 3, 4, 5]);
    expect(() => ico([{ size: 300, png: a }])).toThrow();
  });

  test("favicon.svg is the micro icon without a fixed size", () => {
    const svg = faviconSvg(`<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 16 16"><rect width="16" height="16"/></svg>`);
    expect(svg).toBe(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16"/></svg>\n`);
  });

  test("every source is the project's own committed art", () => {
    for (const path of Object.values(SOURCES)) {
      expect(existsSync(join(ROOT, path))).toBe(true);
      expect(path).toMatch(/^(art\/app-icon|platform\/(ios|tvos)\/.*Assets\.xcassets|crates\/app-icon\/assets|web\/src\/dash\/fonts)\//);
    }
  });

  test("the committed outputs are what the plan says, at their sizes", () => {
    for (const f of outputs()) expect(existsSync(join(PUBLIC, f))).toBe(true);
    for (const i of ICONS.filter((i) => !ICO_PARTS.includes(i))) {
      expect(pngSize(readFileSync(join(PUBLIC, i.file)))).toEqual([i.size, i.size]);
    }
    const icoFile = new Uint8Array(readFileSync(join(PUBLIC, "favicon.ico")));
    const v = new DataView(icoFile.buffer, icoFile.byteOffset);
    expect(v.getUint16(4, true)).toBe(ICO_PARTS.length);
    ICO_PARTS.forEach((p, n) => {
      const at = 6 + 16 * n;
      expect(icoFile[at]).toBe(p.size);
      const png = icoFile.slice(v.getUint32(at + 12, true), v.getUint32(at + 12, true) + v.getUint32(at + 8, true));
      expect(pngSize(png)).toEqual([p.size, p.size]);
    });
    expect(jpegSize(new Uint8Array(readFileSync(join(PUBLIC, CARD.file))))).toEqual([CARD.width, CARD.height]);
    expect(JSON.parse(readFileSync(join(PUBLIC, "manifest.webmanifest"), "utf8"))).toEqual(webManifest());
  });

  test("the manifest names the dashboard, its colours and a maskable icon", () => {
    const m = webManifest();
    expect(m.name).toBe(SITE_NAME);
    expect(m.theme_color).toBe(THEME.dash);
    expect(m.start_url).toBe("/");
    expect(m.icons.some((i) => i.purpose === "maskable" && i.sizes === "512x512")).toBe(true);
    expect(m.icons.filter((i) => i.type === "image/png").map((i) => i.sizes)).toEqual(["192x192", "512x512", "512x512"]);
    expect(m.shortcuts[0]).toEqual({ name: STATS_NAME, short_name: STATS_NAME, url: "/stats" });
  });
});
