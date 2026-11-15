// Optional game art: a server may serve pictures extracted from its owner's
// own disc (docs/live-api.md "Optional game art"). Nothing of it is in this
// repo. Everything here is pure, so it can be tested; state/gameart.ts loads
// the manifest and components fall back to the art pack when a key is absent.

export interface GameArtImage { file: string; width: number; height: number }

export interface GameArtManifest {
  version: number;
  source?: string;
  images: Record<string, GameArtImage>;
  aliases?: Record<string, string>;
  emblem?: unknown;
  skipped?: Record<string, string>;
}

/** A picture the manifest names: where it is and its pixel size. */
export interface GameArtRef { url: string; width: number; height: number }

export interface GameArt {
  /** The directory the manifest lives in, with a trailing slash. */
  root: string;
  manifest: GameArtManifest;
  /** A key's picture: `aliases` first, then `images`. Null when absent. */
  get(key: string): GameArtRef | null;
  has(key: string): boolean;
}

export function gameArtRoot(serverBase: string): string {
  return serverBase.replace(/\/+$/, "") + "/art/game/";
}

/** Checks a manifest's shape; null for anything this client can't read. */
export function parseManifest(raw: unknown): GameArtManifest | null {
  if (!raw || typeof raw !== "object") return null;
  const m = raw as Partial<GameArtManifest>;
  if (m.version !== 1 || !m.images || typeof m.images !== "object") return null;
  const images: Record<string, GameArtImage> = {};
  for (const [key, v] of Object.entries(m.images)) {
    if (!v || typeof v.file !== "string" || !safeFile(v.file)) continue;
    images[key] = { file: v.file, width: Number(v.width) || 0, height: Number(v.height) || 0 };
  }
  const aliases: Record<string, string> = {};
  if (m.aliases && typeof m.aliases === "object") {
    for (const [from, to] of Object.entries(m.aliases)) if (typeof to === "string") aliases[from] = to;
  }
  return { version: 1, source: typeof m.source === "string" ? m.source : undefined, images, aliases, emblem: m.emblem, skipped: m.skipped };
}

/** A file must stay inside the manifest's directory. */
function safeFile(file: string): boolean {
  return !/^([a-z][a-z0-9+.-]*:|\/|\\)/i.test(file) && !file.split(/[\\/]/).includes("..");
}

export function gameArt(root: string, manifest: GameArtManifest): GameArt {
  const get = (key: string): GameArtRef | null => {
    const img = manifest.images[manifest.aliases?.[key] ?? key] ?? manifest.images[key];
    return img ? { url: root + img.file.split("/").map(encodeURIComponent).join("/"), width: img.width, height: img.height } : null;
  };
  return { root, manifest, get, has: (key) => get(key) !== null };
}

type Fetch = (url: string) => Promise<{ ok: boolean; status: number; json(): Promise<unknown> }>;

/** Loads a server's game art. Null when it has none (404), won't give it to
 *  this client, can't be reached, or sends something unreadable: all of
 *  those mean "use the art pack". Never throws. */
export async function loadGameArt(serverBase: string, fetcher: Fetch = (u) => fetch(u)): Promise<GameArt | null> {
  const root = gameArtRoot(serverBase);
  try {
    const res = await fetcher(root + "manifest.json");
    if (!res.ok) return null;
    const manifest = parseManifest(await res.json());
    return manifest ? gameArt(root, manifest) : null;
  } catch {
    return null;
  }
}

// ---- keys ----

/** `map/<map>/preview`: the map file name without path or `.map`. */
export function mapKey(map: string): string {
  const name = map.split(/[\\/]/).pop()!.replace(/\.map$/i, "").trim().toLowerCase();
  return `map/${name}/preview`;
}

const GAMETYPES: Array<[RegExp, string]> = [
  [/\b(ctf|flags?|capture)\b/, "ctf"],
  [/\b(assault|bomb)\b/, "assault"],
  [/\b(king|hill|koth)\b/, "king_of_the_hill"],
  [/\b(odd ?ball|ball)\b/, "oddball"],
  [/\bjuggernaut\b/, "juggernaut"],
  [/\bterritor(y|ies)\b/, "territories"],
  [/\bslayer\b/, "slayer"],
];

/** The game-type key a variant name points at, if its name says. Variant
 *  names are free text ("Team Slayer", "1 Flag CTF"), so this goes by word. */
export function gametypeKey(variant: string | null | undefined): string | null {
  const v = (variant ?? "").toLowerCase().replace(/[_-]+/g, " ");
  for (const [re, name] of GAMETYPES) if (re.test(v)) return `gametype/${name}`;
  return null;
}

/** Our derived ladder (experience points) spread over the game's levels
 *  1..50: each rank's threshold lands on a level, linear in between. */
const LEVELS: Array<[exp: number, level: number]> = [
  [0, 1], [5, 5], [15, 10], [30, 18], [60, 26], [120, 35], [250, 45], [500, 50],
];

export function levelForExp(exp: number): number {
  if (!(exp > 0)) return 1;
  for (let i = 1; i < LEVELS.length; i++) {
    const [e0, l0] = LEVELS[i - 1]!, [e1, l1] = LEVELS[i]!;
    if (exp < e1) return Math.floor(l0 + ((exp - e0) / (e1 - e0)) * (l1 - l0));
  }
  return 50;
}

export const rankKey = (level: number) => `rank/${Math.min(50, Math.max(1, Math.round(level)))}`;

// ---- emblems ----

export type Rgb = readonly [number, number, number];

/** A player's emblem: which layers, and the profile colours that fill them. */
export interface EmblemSpec {
  /** Foreground layer, 0..63. */
  fg: number;
  /** Background layer, 0..31. */
  bg: number;
  /** Foreground mask a. */
  primary: Rgb;
  /** Foreground mask b. */
  secondary: Rgb;
  /** Background mask a. */
  background: Rgb;
  /** Background mask b. */
  background2: Rgb;
}

/** Player colours to pick from until telemetry says what a player chose. */
export const EMBLEM_COLOURS: Rgb[] = [
  [236, 236, 236], [110, 110, 110], [28, 28, 28], [190, 40, 34], [226, 106, 30], [240, 190, 40],
  [126, 150, 60], [40, 130, 60], [246, 150, 170], [130, 60, 150], [40, 150, 160], [110, 180, 230],
  [36, 70, 160], [130, 90, 200], [120, 80, 44], [200, 160, 110], [150, 30, 70], [70, 90, 70],
];

function fnv(seed: string, salt: number): number {
  let h = (2166136261 ^ salt) >>> 0;
  for (let i = 0; i < seed.length; i++) h = Math.imul(h ^ seed.charCodeAt(i), 16777619);
  // Final mix, so near-identical seeds don't land on neighbouring picks.
  h ^= h >>> 15; h = Math.imul(h, 0x2c1b3c6d); h ^= h >>> 12;
  return h >>> 0;
}

/** A stable emblem for a seed (an XUID), within the layers the server has. */
export function emblemChoice(seed: string, fgCount = 64, bgCount = 32): EmblemSpec {
  const n = EMBLEM_COLOURS.length;
  const primary = fnv(seed, 3) % n;
  // The other colours step away from the primary so the layers stay legible.
  const secondary = (primary + 1 + (fnv(seed, 4) % (n - 1))) % n;
  const background = (primary + 1 + (fnv(seed, 5) % (n - 1))) % n;
  const background2 = (background + 1 + (fnv(seed, 6) % (n - 1))) % n;
  return {
    fg: fnv(seed, 1) % Math.max(1, fgCount),
    bg: fnv(seed, 2) % Math.max(1, bgCount),
    primary: EMBLEM_COLOURS[primary]!, secondary: EMBLEM_COLOURS[secondary]!,
    background: EMBLEM_COLOURS[background]!, background2: EMBLEM_COLOURS[background2]!,
  };
}

export const emblemCacheKey = (e: EmblemSpec) =>
  [e.fg, e.bg, e.primary, e.secondary, e.background, e.background2].map((x) => (typeof x === "number" ? x : x.join("."))).join("|");

/** How many consecutive `<prefix><n>` keys the manifest has, from 0. */
export function countKeys(art: GameArt, prefix: string, max: number): number {
  let n = 0;
  while (n < max && art.has(prefix + n)) n++;
  return n;
}

/**
 * Colours an emblem from its two layer images (RGBA bytes, same size).
 *
 * A layer holds two masks, not colours: mask a is the red channel, mask b
 * the blue one. The image's alpha copies mask b and is not coverage, so it
 * is ignored. Per pixel, channels in 0..1:
 *
 *   rgb      = colour_a * red + colour_b * blue
 *   coverage = background: 1;  foreground: max(red, blue)
 *
 * The foreground's rgb is already weighted by its masks, so it goes over
 * the background as `fg + bg * (1 - coverage)`. The result is opaque.
 */
export function compositeEmblem(
  bg: ArrayLike<number>, fg: ArrayLike<number>,
  colours: Pick<EmblemSpec, "primary" | "secondary" | "background" | "background2">,
  out: Uint8ClampedArray = new Uint8ClampedArray(bg.length),
): Uint8ClampedArray {
  const { primary: fa, secondary: fb, background: ba, background2: bb } = colours;
  for (let i = 0; i + 3 < bg.length; i += 4) {
    const br = bg[i]! / 255, bbm = bg[i + 2]! / 255;
    const fr = (fg[i] ?? 0) / 255, fbm = (fg[i + 2] ?? 0) / 255;
    const keep = 1 - Math.max(fr, fbm);
    for (let c = 0; c < 3; c++) {
      const under = Math.min(255, ba[c]! * br + bb[c]! * bbm);
      out[i + c] = Math.round(fa[c]! * fr + fb[c]! * fbm + under * keep);
    }
    out[i + 3] = 255;
  }
  return out;
}

export function parseHexColour(hex: string): Rgb | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  const n = parseInt(m[1]!, 16);
  return [n >> 16, (n >> 8) & 255, n & 255];
}
