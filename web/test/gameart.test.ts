import { compositeEmblem } from "../mock/emblem";
import { emblemPath } from "../src/lib/emblem";
import { describe, expect, test } from "bun:test";
import { inflateSync } from "node:zlib";
import {
  countKeys, emblemChoice, gameArt, gameArtRoot, gametypeKey, levelForExp,
  artBase, emblemFromIndices, emblemPalette, loadGameArt, mapKey, normaliseMask, medalKey, weaponKey, parseHexColour, parseManifest, rankKey, EMBLEM_COLOURS, type EmblemSpec,
} from "../src/lib/gameart";
import { MEDALS, RANKS } from "../src/state/derive";
import { encodePng, mockGameArt } from "../mock/gameart";
import { openAccountRequest } from "../mock/account";
import { generateKey, signAccountRequest } from "../src/state/account";
import { ACCOUNT_VECTOR } from "./helpers";

const MANIFEST = {
  version: 1,
  source: "abc",
  images: {
    "rank/12": { file: "rank/12.png", width: 28, height: 26 },
    "gametype/0": { file: "gametype/0.png", width: 32, height: 32 },
    "map/lockout/preview": { file: "map/lock out.png", width: 224, height: 207 },
    "evil/up": { file: "../../etc/passwd", width: 1, height: 1 },
    "evil/abs": { file: "https://elsewhere.example/x.png", width: 1, height: 1 },
  },
  aliases: { "gametype/slayer": "gametype/0", "gametype/ghost": "gametype/99" },
  emblem: {},
  skipped: {},
};

const reply = (status: number, body: unknown) => async () => ({
  ok: status >= 200 && status < 300, status,
  json: async () => { if (body instanceof Error) throw body; return body; },
});

describe("game art manifest", () => {
  test("loads from <server>/art/game/manifest.json and resolves files against it", async () => {
    let asked = "";
    const art = await loadGameArt("http://127.0.0.1:8074/", async (u) => { asked = u; return reply(200, MANIFEST)(); });
    expect(asked).toBe("http://127.0.0.1:8074/art/game/manifest.json");
    expect(art!.get("rank/12")).toEqual({ url: "http://127.0.0.1:8074/art/game/rank/12.png", width: 28, height: 26 });
    expect(art!.get("map/lockout/preview")!.url).toBe("http://127.0.0.1:8074/art/game/map/lock%20out.png");
  });

  test("a manifest redirected to its published copy roots every file there", async () => {
    const published = "https://live-art.example.net/art/0123456789abcdef/";
    const art = await loadGameArt("https://live.example.net", async () => ({ ...(await reply(200, MANIFEST)()), url: published + "manifest.json" }));
    expect(art!.root).toBe(published);
    expect(art!.get("rank/12")!.url).toBe(published + "rank/12.png");
    // Same place, not redirected: the server's own address.
    expect(artBase("http://h/art/game/", "http://h/art/game/manifest.json")).toBe("http://h/art/game/");
    expect(artBase("http://h/art/game/", "https://b.example/art/x/manifest.json?v=1")).toBe("https://b.example/art/x/");
    // Unknown, or not where a manifest would be: the server's address.
    for (const odd of [undefined, "", "not a url", "https://b.example/elsewhere.json", "data:,manifest.json", "blob:https://h/manifest.json"]) {
      expect(artBase("http://h/art/game/", odd)).toBe("http://h/art/game/");
    }
  });

  test("looks a key up in aliases first, then images", () => {
    const art = gameArt(gameArtRoot("http://h"), parseManifest(MANIFEST)!);
    expect(art.get("gametype/slayer")!.url).toBe("http://h/art/game/gametype/0.png");
    expect(art.get("gametype/0")!.url).toBe("http://h/art/game/gametype/0.png");
    expect(art.get("gametype/ghost")).toBeNull(); // an alias to nothing
    expect(art.has("rank/13")).toBe(false);
    expect(art.has("weapon/3")).toBe(false);
  });

  test("drops files that would leave the manifest's directory", () => {
    const art = gameArt("http://h/art/game/", parseManifest(MANIFEST)!);
    expect(art.get("evil/up")).toBeNull();
    expect(art.get("evil/abs")).toBeNull();
  });

  test("absent, refused, unreachable or unreadable all mean no game art, never an error", async () => {
    expect(await loadGameArt("http://h", reply(404, { error: "not_found" }))).toBeNull();
    expect(await loadGameArt("http://h", reply(403, {}))).toBeNull();
    expect(await loadGameArt("http://h", reply(200, new SyntaxError("not json")))).toBeNull();
    expect(await loadGameArt("http://h", async () => { throw new TypeError("network"); })).toBeNull();
    expect(await loadGameArt("http://h", reply(200, { version: 2, images: {} }))).toBeNull();
    expect(await loadGameArt("http://h", reply(200, "<html>"))).toBeNull();
    expect(await loadGameArt("http://h", reply(200, { version: 1 }))).toBeNull();
  });

  test("counts the emblem layers a server has", () => {
    const images = Object.fromEntries([0, 1, 2, 4].map((n) => [`emblem/fg/${n}`, { file: `f${n}.png`, width: 1, height: 1 }]));
    const art = gameArt("http://h/", parseManifest({ version: 1, images })!);
    expect(countKeys(art, "emblem/fg/", 64)).toBe(3);
    expect(countKeys(art, "emblem/bg/", 32)).toBe(0);
  });
});

describe("game art keys", () => {
  test("map previews go by map file name, without path or extension", () => {
    expect(mapKey("lockout")).toBe("map/lockout/preview");
    expect(mapKey("Lockout.map")).toBe("map/lockout/preview");
    expect(mapKey("scenarios\\multi\\ascension\\ascension")).toBe("map/ascension/preview");
    expect(mapKey("maps/coagulation.map")).toBe("map/coagulation/preview");
  });

  test("game types go by the words of the variant name", () => {
    expect(gametypeKey("Team Slayer")).toBe("gametype/slayer");
    expect(gametypeKey("1 Flag CTF")).toBe("gametype/ctf");
    expect(gametypeKey("Multi Flag")).toBe("gametype/ctf");
    expect(gametypeKey("Capture the Cushion")).toBe("gametype/ctf");
    expect(gametypeKey("King of the Hill")).toBe("gametype/king_of_the_hill");
    expect(gametypeKey("Crazy King")).toBe("gametype/king_of_the_hill");
    expect(gametypeKey("oddball")).toBe("gametype/oddball");
    expect(gametypeKey("Team Ball")).toBe("gametype/oddball");
    expect(gametypeKey("Juggernaut")).toBe("gametype/juggernaut");
    expect(gametypeKey("Single Bomb Assault")).toBe("gametype/assault");
    expect(gametypeKey("3 Plots Territories")).toBe("gametype/territories");
    expect(gametypeKey("king_of_the_hill")).toBe("gametype/king_of_the_hill");
    expect(gametypeKey("Rumble Pit")).toBeNull();
    expect(gametypeKey(null)).toBeNull();
  });

  test("our derived medals map onto the game's where one means the same", () => {
    expect(medalKey("twofer")).toBe("medal/double_kill");
    expect(medalKey("hat-trick")).toBe("medal/triple_kill");
    expect(medalKey("full-house")).toBe("medal/killtacular");
    expect(medalKey("encore")).toBe("medal/kill_frenzy");
    expect(medalKey("hot-streak")).toBe("medal/killing_spree");
    expect(medalKey("wildfire")).toBe("medal/running_riot");
    expect(medalKey("untouchable")).toBe("medal/rampage");
    for (const id of ["opening-act", "last-word", "payback"]) expect(medalKey(id)).toBeNull();
    // Every derived medal is either mapped or deliberately left to the pack.
    expect(MEDALS.filter((m) => medalKey(m.id)).length).toBe(7);
  });

  test("weapons go by the words of the reported name", () => {
    expect(weaponKey("sword")).toBe("weapon/energy_sword");
    expect(weaponKey("energy_sword")).toBe("weapon/energy_sword");
    expect(weaponKey("energy_blade")).toBe("weapon/energy_sword");
    expect(weaponKey("Plasma Pistol")).toBe("weapon/plasma_pistol");
    expect(weaponKey("plasma_rifle")).toBe("weapon/plasma_rifle");
    expect(weaponKey("sniper")).toBe("weapon/sniper_rifle");
    expect(weaponKey("Sniper Rifle")).toBe("weapon/sniper_rifle");
    expect(weaponKey("beam-rifle")).toBe("weapon/beam_rifle");
    expect(weaponKey("sentinel beam")).toBe("weapon/sentinel_beam");
    expect(weaponKey("sentinel_rpg")).toBeNull();
    expect(weaponKey("rocket")).toBe("weapon/rocket_launcher");
    expect(weaponKey("smg")).toBe("weapon/smg");
    expect(weaponKey("Submachine Gun")).toBe("weapon/smg");
    for (const w of ["rifle", "shotgun", "grenade", "pistol", "blaster", "", null]) expect(weaponKey(w)).toBeNull();
  });

  test("our rank ladder spreads over levels 1..50, each threshold on a higher level", () => {
    expect(levelForExp(0)).toBe(1);
    expect(levelForExp(-3)).toBe(1);
    expect(levelForExp(NaN)).toBe(1);
    expect(levelForExp(1e9)).toBe(50);
    let last = 0;
    for (const [threshold] of RANKS) {
      const level = levelForExp(threshold);
      expect(level).toBeGreaterThan(last);
      last = level;
    }
    for (let exp = 0, prev = 1; exp <= 600; exp++) {
      const level = levelForExp(exp);
      expect(level).toBeGreaterThanOrEqual(prev);
      expect(level).toBeLessThanOrEqual(50);
      prev = level;
    }
    expect(rankKey(levelForExp(15))).toBe("rank/10");
    expect(rankKey(0)).toBe("rank/1");
    expect(rankKey(99)).toBe("rank/50");
  });
});

const px = (...p: number[][]) => Uint8Array.from(p.flat());
const COLOURS = { primary: [200, 100, 0], secondary: [0, 50, 250], background: [10, 20, 30], background2: [100, 100, 100] } as const;

describe("emblem compositing", () => {
  test("background: rgb = colour_a * red + colour_b * blue, opaque whatever its alpha", () => {
    const bg = px([255, 255, 0, 0], [0, 0, 255, 255], [0, 0, 0, 0], [128, 128, 0, 0]);
    const none = new Uint8Array(16);
    const out = compositeEmblem(bg, none, COLOURS);
    expect([...out]).toEqual([
      10, 20, 30, 255, // all mask a: the background colour, though alpha was 0
      100, 100, 100, 255, // all mask b
      0, 0, 0, 255, // neither mask: black, still opaque
      5, 10, 15, 255, // half of mask a
    ]);
  });

  test("foreground: coverage is max(red, blue) over the background; its alpha is ignored", () => {
    const bg = px([255, 255, 0, 0], [255, 255, 0, 0], [255, 255, 0, 0], [255, 255, 0, 0]);
    const fg = px(
      [255, 255, 0, 0], // mask a, alpha 0: still fully covers
      [0, 0, 255, 255], // mask b
      [0, 0, 0, 255], // no mask, alpha 255: covers nothing
      [128, 128, 0, 0], // half mask a: half primary, half background
    );
    const out = compositeEmblem(bg, fg, COLOURS);
    expect([...out.slice(0, 4)]).toEqual([200, 100, 0, 255]);
    expect([...out.slice(4, 8)]).toEqual([0, 50, 250, 255]);
    expect([...out.slice(8, 12)]).toEqual([10, 20, 30, 255]);
    const half = 128 / 255;
    expect([...out.slice(12, 16)]).toEqual([
      Math.round(200 * half + 10 * (1 - half)), Math.round(100 * half + 20 * (1 - half)), Math.round(0 * half + 30 * (1 - half)), 255,
    ]);
  });

  test("both masks at once add, and clamp at white", () => {
    const out = compositeEmblem(px([0, 0, 0, 0]), px([255, 255, 255, 255]), { ...COLOURS, primary: [200, 200, 200], secondary: [100, 20, 0] });
    expect([...out]).toEqual([255, 220, 200, 255]);
  });

  test("green is not read (it only mirrors red)", () => {
    const a = compositeEmblem(px([255, 0, 0, 0]), px([0, 255, 0, 0]), COLOURS);
    expect([...a]).toEqual([10, 20, 30, 255]);
  });

  test("a player's emblem is stable, in range, and differs between players", () => {
    const a = emblemChoice("0009000000000001");
    expect(emblemChoice("0009000000000001")).toEqual(a);
    const seen = new Set<string>();
    for (let i = 0; i < 200; i++) {
      const e = emblemChoice("00090000000" + i.toString(16).padStart(5, "0"));
      expect(e.fg).toBeGreaterThanOrEqual(0); expect(e.fg).toBeLessThan(64);
      expect(e.bg).toBeGreaterThanOrEqual(0); expect(e.bg).toBeLessThan(32);
      for (const c of [e.primary, e.secondary, e.background, e.background2]) expect(EMBLEM_COLOURS).toContain(c);
      // The layers stay legible: neighbours never share a colour.
      expect(e.secondary).not.toBe(e.primary);
      expect(e.background).not.toBe(e.primary);
      expect(e.background2).not.toBe(e.background);
      seen.add(emblemPath("seed", null, e));
    }
    expect(seen.size).toBeGreaterThan(190);
    const few = emblemChoice("x", 3, 2);
    expect(few.fg).toBeLessThan(3); expect(few.bg).toBeLessThan(2);
  });

  test("colours come from the manifest's palette when it has one", () => {
    const palette = emblemPalette({ palette: ["#cccccc", "#555555", "#aa2222", "oops", "#dd6622"] });
    expect(palette).toEqual([[204, 204, 204], [85, 85, 85], [170, 34, 34], [221, 102, 34]]);
    for (let i = 0; i < 50; i++) {
      const e = emblemChoice("seed" + i, 64, 32, palette);
      for (const c of [e.primary, e.secondary, e.background, e.background2]) expect(palette).toContain(c);
    }
    // No palette (older extract, or not found on the disc): our own colours.
    expect(emblemPalette({ palette: [] })).toBe(EMBLEM_COLOURS);
    expect(emblemPalette(null)).toBe(EMBLEM_COLOURS);
    expect(parseManifest({ version: 1, images: {}, palette: "nope" })!.palette).toEqual([]);
    // A profile's own values are indices into it.
    expect(emblemFromIndices(palette, { fg: 7, bg: 3, primary: 2, secondary: 0, background: 1, background2: 3 }))
      .toEqual({ fg: 7, bg: 3, primary: [170, 34, 34], secondary: [204, 204, 204], background: [85, 85, 85], background2: [221, 102, 34] });
  });

  test("the cache key tells layers and colours apart", () => {
    const e: EmblemSpec = { fg: 1, bg: 2, primary: [1, 2, 3], secondary: [4, 5, 6], background: [7, 8, 9], background2: [0, 0, 0] };
    expect(emblemPath("p", null, e)).toBe(emblemPath("p", null, { ...e }));
    expect(emblemPath("p", null, { ...e, fg: 2 })).not.toBe(emblemPath("p", null, e));
    expect(emblemPath("p", null, { ...e, primary: [1, 2, 4] })).not.toBe(emblemPath("p", null, e));
    expect(parseHexColour("#ff8000")).toEqual([255, 128, 0]);
    expect(parseHexColour("nope")).toBeNull();
  });
});

/** Decodes the PNGs this repo's mock writes (8-bit RGBA, no filtering). */
function decodePng(bytes: Uint8Array): { width: number; height: number; rgba: Uint8Array } {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const width = view.getUint32(16), height = view.getUint32(20);
  let at = 8, idat: Uint8Array | null = null;
  while (at < bytes.length) {
    const len = view.getUint32(at), type = String.fromCharCode(...bytes.subarray(at + 4, at + 8));
    if (type === "IDAT") idat = bytes.subarray(at + 8, at + 8 + len);
    at += 12 + len;
  }
  const raw = new Uint8Array(inflateSync(idat!));
  const rgba = new Uint8Array(width * height * 4);
  for (let y = 0; y < height; y++) rgba.set(raw.subarray(y * (width * 4 + 1) + 1, (y + 1) * (width * 4 + 1)), y * width * 4);
  return { width, height, rgba };
}

describe("the mock's fake game art", () => {
  const art = mockGameArt(new URL("../public/art", import.meta.url).pathname);

  test("its manifest reads like a real one, with every key family the app uses", () => {
    const m = parseManifest(JSON.parse(JSON.stringify(art.manifest)))!;
    const a = gameArt("http://h/art/game/", m);
    expect(countKeys(a, "emblem/fg/", 99)).toBe(64);
    expect(countKeys(a, "emblem/bg/", 99)).toBe(32);
    for (let level = 1; level <= 50; level++) expect(a.has(`rank/${level}`)).toBe(true);
    for (const name of ["slayer", "king_of_the_hill", "oddball", "juggernaut", "ctf", "assault", "territories"]) expect(a.has(`gametype/${name}`)).toBe(true);
    expect(a.has(mapKey("teacup"))).toBe(true);
    expect(a.has(mapKey("lighthouse"))).toBe(false); // one map without, so the fallback shows
    expect(a.has("live/0")).toBe(false);
    expect(a.has(weaponKey("sword")!)).toBe(true);
    expect(a.has(medalKey("twofer")!)).toBe(true);
    expect(emblemPalette(m)).toHaveLength(18);
    expect(emblemPalette(m)[0]).toEqual(EMBLEM_COLOURS[17]!);
  });

  test("serves every file it names, and nothing else", async () => {
    expect(await art.file("manifest.exe")).toBeNull();
    expect(await art.file("../package.json")).toBeNull();
    const rank = await art.file(art.manifest.images["rank/50"]!.file);
    expect(rank!.type).toBe("image/webp");
    expect(String.fromCharCode(...rank!.bytes.subarray(0, 4))).toBe("RIFF");
    const map = await art.file(art.manifest.images["map/teacup/preview"]!.file);
    expect(decodePng(map!.bytes)).toMatchObject({ width: 320, height: 180 });
  });

  test("PNG round trip", () => {
    const rgba = Uint8Array.from([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24]);
    expect([...decodePng(encodePng(rgba, 3, 2)).rgba]).toEqual([...rgba]);
  });

  test("its emblem layers are masks (green = red, alpha = blue) and colour as the rules say", async () => {
    const fgFile = await art.file("emblem/fg/0.png"), bgFile = await art.file("emblem/bg/1.png");
    const fg = decodePng(fgFile!.bytes), bg = decodePng(bgFile!.bytes);
    expect(fg.width).toBe(bg.width);
    let covered = 0;
    for (let i = 0; i < fg.rgba.length; i += 4) {
      expect(fg.rgba[i + 1]).toBe(fg.rgba[i]!);
      expect(fg.rgba[i + 3]).toBe(fg.rgba[i + 2]!);
      if (fg.rgba[i] === 255) covered++;
    }
    expect(covered).toBeGreaterThan(100);
    const out = compositeEmblem(bg.rgba, fg.rgba, COLOURS);
    const at = (x: number, y: number) => [...out.slice((y * fg.width + x) * 4, (y * fg.width + x) * 4 + 4)];
    const mid = fg.width >> 1;
    expect(at(mid, mid)).toEqual([200, 100, 0, 255]); // foreground 0 is a disc: primary at the centre
    expect(at(2, 2)).toEqual([10, 20, 30, 255]); // background 1's top half is mask a
    expect(at(2, fg.height - 3)).toEqual([100, 100, 100, 255]); // and its bottom half mask b
  });
});

describe("the mock's account endpoint checks aud and iat like the real server", () => {
  const NOW = 1_800_000_000;

  test("accepts a request for this server made just now", async () => {
    const key = await generateKey();
    const opened = await openAccountRequest(await signAccountRequest(key, "Sneaky Pete", "mock.h2live.local", NOW - 20), "mock.h2live.local", NOW);
    expect(opened).toMatchObject({ gamertag: "Sneaky Pete" });
  });

  test("the live-proto vector opens for its own audience and time only", async () => {
    expect(await openAccountRequest(ACCOUNT_VECTOR, "localhost", NOW)).toMatchObject({ gamertag: "Sneaky Pete" });
    expect(await openAccountRequest(ACCOUNT_VECTOR, "localhost", NOW + 300)).toMatchObject({ gamertag: "Sneaky Pete" });
    expect(await openAccountRequest(ACCOUNT_VECTOR, "localhost", NOW + 301)).toEqual({ error: "account request iat is more than 5 minutes off" });
    expect(await openAccountRequest(ACCOUNT_VECTOR, "localhost", NOW - 301)).toEqual({ error: "account request iat is more than 5 minutes off" });
    expect(await openAccountRequest(ACCOUNT_VECTOR, "mock.h2live.local", NOW)).toEqual({ error: "account request is for another server" });
  });

  test("a body without aud or iat is refused", async () => {
    const key = await generateKey();
    const { signCompact, TYP_ACCOUNT } = await import("../src/state/account");
    const { canonicalJson } = await import("../src/state/verify");
    const old = await signCompact(key.privateKey, { alg: "EdDSA", kid: key.publicJwk.kid, typ: TYP_ACCOUNT }, canonicalJson({ gamertag: "Old Client", jwk: key.publicJwk }));
    expect(await openAccountRequest(old, "mock.h2live.local", NOW)).toEqual({ error: "bad body: aud and iat are required" });
  });
});

describe("the mock's weapon masks", () => {
  test("are single faint masks (blue with matching alpha), stretched to full strength for tinting", async () => {
    const art = mockGameArt(new URL("../public/art", import.meta.url).pathname);
    const f = await art.file("weapon/0.png");
    const png = decodePng(f!.bytes);
    let on = 0;
    for (let i = 0; i < png.rgba.length; i += 4) {
      expect(png.rgba[i]).toBe(0);
      expect(png.rgba[i + 1]).toBe(0);
      expect(png.rgba[i + 3]).toBe(png.rgba[i + 2]!);
      expect(png.rgba[i + 3]).toBeLessThanOrEqual(68);
      if (png.rgba[i + 3] === 68) on++;
    }
    expect(on).toBeGreaterThan(50);
    const mask = normaliseMask(png.rgba);
    let full = 0;
    for (let i = 0; i < mask.length; i += 4) {
      expect([mask[i], mask[i + 1], mask[i + 2]]).toEqual([255, 255, 255]);
      expect(mask[i + 3]).toBe(Math.round((png.rgba[i + 3]! / 68) * 255));
      if (mask[i + 3] === 255) full++;
    }
    expect(full).toBe(on);
  });

  test("normalising keeps the shape's soft edge and survives an empty picture", () => {
    expect([...normaliseMask(Uint8Array.from([0, 0, 68, 68, 0, 0, 34, 34, 0, 0, 0, 0]))]).toEqual([255, 255, 255, 255, 255, 255, 255, 128, 255, 255, 255, 0]);
    expect([...normaliseMask(new Uint8Array(8))]).toEqual([255, 255, 255, 0, 255, 255, 255, 0]);
  });
});
