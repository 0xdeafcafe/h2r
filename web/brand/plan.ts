// What `bun run brand` makes (run.ts draws it): the web app's icons, its
// manifest and the default link-preview card, all from the project's own
// app icon art (art/app-icon/README.md: hand-drawn, NASA Blue Marble
// albedo, nothing from the game). Everything here is pure, for tests.

import { BACKGROUND, DASH_NAME, STATS_NAME, THEME } from "../src/brand";
import { STATS_ROOT, statsPath } from "../src/routes";

/** Paths from the repository root. */
export const SOURCES = {
  /** The 16-unit Mac icon redrawn for tiny sizes: true vector, opaque. */
  micro: "art/app-icon/sources/macos-optical-micro.svg",
  /** The iOS icon: the full scene, square and opaque. */
  scene: "platform/ios/H2iOS/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png",
  /** The Mac icon: the scene on a rounded plate, transparent around it. */
  plated: "crates/app-icon/assets/AppIcon-512.png",
  /** The Apple TV top shelf: the scene, wide. */
  shelf: "platform/tvos/H2TV/Assets.xcassets/App Icon & Top Shelf Image.brandassets/Top Shelf Image.imageset/TopShelf@2x.png",
  /** The fonts the card's words are set in (OFL, in the repository). */
  orbitron: "web/src/dash/fonts/Orbitron-wght.ttf",
  barlow: "web/src/dash/fonts/BarlowCondensed-SemiBold.ttf",
} as const;

export type Source = keyof typeof SOURCES;

/** One square icon: which source, drawn how big, and how much of the
 *  square it fills (1 unless set; below 1 the rest is the source's own
 *  edges, blurred out). The maskable icon is the full scene: its numeral
 *  sits inside the middle 80% that every mask keeps. */
export interface Icon {
  file: string;
  size: number;
  source: Source;
  /** The share of the square the source covers, centred (1: all of it). */
  scale?: number;
}

/** The icons, into web/public. The ICO's sizes come from the `ico-*` ones. */
export const ICONS: Icon[] = [
  { file: "ico-16.png", size: 16, source: "micro" },
  { file: "ico-32.png", size: 32, source: "micro" },
  { file: "ico-48.png", size: 48, source: "micro" },
  { file: "apple-touch-icon.png", size: 180, source: "scene" },
  { file: "icon-192.png", size: 192, source: "plated" },
  { file: "icon-512.png", size: 512, source: "plated" },
  { file: "icon-maskable-512.png", size: 512, source: "scene" },
];

/** Icons only the ICO holds: not written on their own. */
export const ICO_PARTS = ICONS.filter((i) => i.file.startsWith("ico-"));

/** The link-preview card: Open Graph's and X's large image. */
export const CARD = { file: "og/default.jpg", width: 1200, height: 630, quality: 88 } as const;

/** `favicon.svg` is the micro source as it is, minus its fixed size, so
 *  it scales to the tab. */
export function faviconSvg(micro: string): string {
  return micro.trim().replace(/^<svg\b[^>]*>/, (root) => root.replace(/\s(width|height)="\d+"/g, "")) + "\n";
}

/** A Windows icon holding PNGs (Vista and later read PNG entries; every
 *  browser does). Each entry: its size (1..256) and PNG bytes. */
export function ico(images: Array<{ size: number; png: Uint8Array }>): Uint8Array {
  const header = 6, entry = 16;
  let offset = header + entry * images.length;
  const total = offset + images.reduce((n, i) => n + i.png.length, 0);
  const out = new Uint8Array(total);
  const view = new DataView(out.buffer);
  view.setUint16(0, 0, true);
  view.setUint16(2, 1, true);
  view.setUint16(4, images.length, true);
  images.forEach((img, i) => {
    if (img.size < 1 || img.size > 256) throw new Error(`an icon is 1 to 256 pixels, not ${img.size}`);
    const at = header + entry * i;
    view.setUint8(at, img.size === 256 ? 0 : img.size);
    view.setUint8(at + 1, img.size === 256 ? 0 : img.size);
    view.setUint8(at + 2, 0); // no palette
    view.setUint8(at + 3, 0);
    view.setUint16(at + 4, 1, true); // planes
    view.setUint16(at + 6, 32, true); // bits per pixel
    view.setUint32(at + 8, img.png.length, true);
    view.setUint32(at + 12, offset, true);
    out.set(img.png, offset);
    offset += img.png.length;
  });
  return out;
}

/** The web app manifest (`manifest.webmanifest`). Installed, it opens on
 *  the dashboard, in the dashboard's colours. */
export function webManifest() {
  return {
    name: DASH_NAME,
    short_name: DASH_NAME,
    description: `The dashboard and ${STATS_NAME}: who is online, live games, service records and clans for h2recomp.`,
    start_url: "/",
    scope: "/",
    display: "standalone",
    orientation: "any",
    theme_color: THEME.dash,
    background_color: BACKGROUND,
    icons: [
      { src: "/icon-192.png", sizes: "192x192", type: "image/png", purpose: "any" },
      { src: "/icon-512.png", sizes: "512x512", type: "image/png", purpose: "any" },
      { src: "/icon-maskable-512.png", sizes: "512x512", type: "image/png", purpose: "maskable" },
      { src: "/favicon.svg", sizes: "any", type: "image/svg+xml", purpose: "any" },
    ],
    shortcuts: [
      { name: STATS_NAME, short_name: STATS_NAME, url: STATS_ROOT },
      { name: "Games", url: statsPath("/games") },
      { name: "Players", url: statsPath("/players") },
    ],
  };
}

/** Every file `bun run brand` writes into web/public. */
export function outputs(): string[] {
  return [
    "favicon.ico",
    "favicon.svg",
    ...ICONS.filter((i) => !ICO_PARTS.includes(i)).map((i) => i.file),
    "manifest.webmanifest",
    CARD.file,
  ];
}
