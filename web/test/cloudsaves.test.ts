// The Games screen's cloud saves (api/cloudsaves.ts) and the save pictures
// (lib/xpr.ts). Every name, save and picture here is invented.

import { describe, expect, test } from "bun:test";
import { blocks, deleteSave, fetchItem, fetchStorage, groupSaves, itemPath, metaValue, parseStorageList, sharedSaveImage, type SavesCtx, type StorageItem } from "../src/api/cloudsaves";
import { decodeXpr } from "../src/lib/xpr";

const item = (name: string, over: Partial<StorageItem> = {}): StorageItem => ({ kind: "save", name, version: 1, sha256: "ab".repeat(32), size: 100, modified: 1000, deleted: false, ...over });

/** A SaveMeta.xbx as xapi's `encode_meta` writes it. */
function meta(name: string, extra = ""): Uint8Array {
  const text = `﻿Name=${name}\r\n${extra}`;
  const out = new Uint8Array(text.length * 2);
  for (let i = 0; i < text.length; i++) { out[i * 2] = text.charCodeAt(i) & 0xff; out[i * 2 + 1] = text.charCodeAt(i) >> 8; }
  return out;
}

describe("the storage list", () => {
  test("reads items, usage and quota, dropping what it can't read", () => {
    const l = parseStorageList({
      items: [
        { kind: "save", name: "0123456789AB/SaveMeta.xbx", version: 2, sha256: "cd".repeat(32), size: 40, modified: 5, stored_at: "x" },
        { kind: "save", name: "" },
        { kind: 3, name: "x" },
        { kind: "save", name: "0123456789AB/old", version: 3, size: 0, modified: 6, deleted: true },
      ],
      used_bytes: 40, quota_bytes: 64 * 1024 * 1024, item_bytes: 8,
    });
    expect(l.items).toHaveLength(2);
    expect(l.items[0]).toEqual({ kind: "save", name: "0123456789AB/SaveMeta.xbx", version: 2, sha256: "cd".repeat(32), size: 40, modified: 5, deleted: false });
    expect(l.items[1]!.deleted).toBe(true);
    expect(l.used_bytes).toBe(40);
    expect(l.quota_bytes).toBe(64 * 1024 * 1024);
    expect(parseStorageList(null)).toEqual({ items: [], used_bytes: 0, quota_bytes: 0 });
  });

  test("groups a container's files into one save, newest first, without tombstones, settings or the title's files", () => {
    const saves = groupSaves([
      item("0123456789AB/SaveMeta.xbx", { size: 50, modified: 10 }),
      item("0123456789AB/data", { size: 4096, modified: 30 }),
      item("0123456789AB/gone", { deleted: true, size: 0, modified: 99 }),
      item("FEDCBA987654/SaveMeta.xbx", { modified: 50 }),
      item("FEDCBA987654/SaveImage.xbx", { modified: 40 }),
      item("TitleMeta.xbx", { modified: 70 }),
      item("SaveImage.xbx", { modified: 70 }),
      item("notacontainer/file", { modified: 70 }),
      item("6869", { kind: "settings", modified: 80 }),
    ]);
    expect(saves.map((s) => s.dir)).toEqual(["FEDCBA987654", "0123456789AB"]);
    const [a, b] = saves;
    expect(a!.image?.name).toBe("FEDCBA987654/SaveImage.xbx");
    expect(b!.size).toBe(4146);
    expect(b!.modified).toBe(30);
    expect(b!.files).toHaveLength(2);
    expect(b!.meta?.name).toBe("0123456789AB/SaveMeta.xbx");
    expect(b!.image).toBeNull();
  });

  test("finds the title's shared save image at the top of U:", () => {
    expect(sharedSaveImage([item("0123456789AB/SaveImage.xbx"), item("SaveImage.xbx")])?.name).toBe("SaveImage.xbx");
    expect(sharedSaveImage([item("SaveImage.xbx", { deleted: true })])).toBeNull();
  });

  test("sizes in the original dashboard's 16 KiB blocks, at least one", () => {
    expect(blocks(0)).toBe(1);
    expect(blocks(16 * 1024)).toBe(1);
    expect(blocks(16 * 1024 + 1)).toBe(2);
    expect(blocks(0x144)).toBe(1);
  });
});

describe("save metadata", () => {
  test("reads the save's name from SaveMeta.xbx", () => {
    expect(metaValue(meta("Invented Slayer"), "Name")).toBe("Invented Slayer");
    expect(metaValue(meta("Profile One", "NoCopy=1\r\n"), "NoCopy")).toBe("1");
    expect(metaValue(meta("Ünïcode ✓"), "Name")).toBe("Ünïcode ✓");
  });
  test("is null without the key or the bytes", () => {
    expect(metaValue(meta("x"), "Title")).toBeNull();
    expect(metaValue(new Uint8Array(1), "Name")).toBeNull();
  });
});

describe("requests", () => {
  const ctx = async (): Promise<SavesCtx> => {
    const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
    const jwk = (await crypto.subtle.exportKey("jwk", pair.privateKey)) as JsonWebKey;
    return { api: "https://live.test/v1", aud: "srv", me: { id: "acct", xuid: "0009000000000001", gamertag: "Tester", key: jwk.d!, x: jwk.x! } as SavesCtx["me"] };
  };

  test("names each item by its percent-encoded segments", () => {
    expect(itemPath("0009000000000001", "save", "0123456789AB/Save Meta.xbx")).toBe("/accounts/0009000000000001/storage/save/0123456789AB/Save%20Meta.xbx");
  });

  test("lists with a storage.read token; a server without cloud saves is null", async () => {
    const c = await ctx();
    const seen: Array<{ url: string; auth: string }> = [];
    const ok = (async (url: string, init?: RequestInit) => {
      seen.push({ url, auth: (init?.headers as Record<string, string>).authorization! });
      return Response.json({ items: [], used_bytes: 0, quota_bytes: 10 });
    }) as unknown as typeof fetch;
    expect((await fetchStorage(c, ok))?.quota_bytes).toBe(10);
    expect(seen[0]!.url).toBe("https://live.test/v1/accounts/0009000000000001/storage");
    const claims = JSON.parse(atob(seen[0]!.auth.split(".")[1]!.replace(/-/g, "+").replace(/_/g, "/")));
    expect(claims.lxm).toBe("storage.read");
    const missing = (async () => Response.json({ error: "not_found", message: "no" }, { status: 404 })) as unknown as typeof fetch;
    expect(await fetchStorage(c, missing)).toBeNull();
    const broken = (async () => Response.json({ error: "internal", message: "boom" }, { status: 500 })) as unknown as typeof fetch;
    await expect(fetchStorage(c, broken)).rejects.toThrow("boom");
  });

  test("fetches an item's bytes", async () => {
    const c = await ctx();
    const f = (async () => new Response(new Uint8Array([1, 2, 3]))) as unknown as typeof fetch;
    expect([...await fetchItem(c, item("0123456789AB/data"), f)]).toEqual([1, 2, 3]);
  });

  test("deletes a save file by file against each version, the metadata last", async () => {
    const c = await ctx();
    const calls: Array<{ url: string; method?: string; match: string; lxm: string }> = [];
    const f = (async (url: string, init?: RequestInit) => {
      const h = init?.headers as Record<string, string>;
      calls.push({ url, method: init?.method, match: h["if-match"]!, lxm: JSON.parse(atob(h.authorization!.split(".")[1]!.replace(/-/g, "+").replace(/_/g, "/"))).lxm });
      return url.endsWith("/gone") ? Response.json({ error: "not_found", message: "gone" }, { status: 404 }) : Response.json({});
    }) as unknown as typeof fetch;
    const [save] = groupSaves([item("0123456789AB/SaveMeta.xbx", { version: 4 }), item("0123456789AB/data", { version: 7 }), item("0123456789AB/gone")]);
    await deleteSave(c, save!, f);
    expect(calls.map((x) => [x.url.split("/storage/")[1], x.method, x.match, x.lxm])).toEqual([
      ["save/0123456789AB/data", "DELETE", "7", "storage.write"],
      ["save/0123456789AB/gone", "DELETE", "1", "storage.write"],
      ["save/0123456789AB/SaveMeta.xbx", "DELETE", "4", "storage.write"],
    ]);
  });

  test("keeps the metadata when another file could not be deleted", async () => {
    const c = await ctx();
    const urls: string[] = [];
    const f = (async (url: string) => {
      urls.push(url);
      return url.endsWith("/data") ? Response.json({ error: "conflict", message: "changed" }, { status: 409 }) : Response.json({});
    }) as unknown as typeof fetch;
    const [save] = groupSaves([item("0123456789AB/SaveMeta.xbx"), item("0123456789AB/data"), item("0123456789AB/more")]);
    await expect(deleteSave(c, save!, f)).rejects.toThrow("changed");
    expect(urls.some((u) => u.endsWith("SaveMeta.xbx"))).toBe(false);
    expect(urls.some((u) => u.endsWith("/more"))).toBe(true);
  });
});

/** An XPR0 with one square DXT texture, every block `block`. */
function xpr(logSide: number, fmt: number, block: number[]): Uint8Array {
  const side = 1 << logSide, per = fmt === 0x0c ? 8 : 16;
  const data = Math.ceil(side / 4) ** 2 * per, header = 0x40;
  const out = new Uint8Array(header + data);
  const dv = new DataView(out.buffer);
  out.set([0x58, 0x50, 0x52, 0x30]);
  const format = (logSide << 24) | (logSide << 20) | (1 << 16) | (fmt << 8) | (2 << 4) | 1;
  [header + data, header, 0x00040001, 0, 0, format >>> 0, 0, 0xffffffff].forEach((v, i) => dv.setUint32(4 + i * 4, v >>> 0, true));
  for (let o = header; o < out.length; o += per) out.set(block, o);
  return out;
}

describe("save pictures", () => {
  test("decodes a DXT1 save image", () => {
    const img = decodeXpr(xpr(6, 0x0c, [0x00, 0xf8, 0, 0, 0, 0, 0, 0]))!;
    expect([img.width, img.height]).toEqual([64, 64]);
    expect([...img.data.subarray(0, 4)]).toEqual([255, 0, 0, 255]);
  });
  test("DXT1's punch-through index 3 is transparent", () => {
    const img = decodeXpr(xpr(2, 0x0c, [0x00, 0x00, 0x1f, 0x00, 0xff, 0xff, 0xff, 0xff]))!;
    expect([...img.data.subarray(0, 4)]).toEqual([0, 0, 0, 0]);
  });
  test("decodes DXT3 and DXT5 alpha", () => {
    const d3 = decodeXpr(xpr(2, 0x0e, [0x0f, 0, 0, 0, 0, 0, 0, 0, 0x1f, 0, 0, 0, 0, 0, 0, 0]))!;
    expect([...d3.data.subarray(0, 4)]).toEqual([0, 0, 255, 255]);
    expect(d3.data[7]).toBe(0);
    const d5 = decodeXpr(xpr(2, 0x0f, [200, 100, 0, 0, 0, 0, 0, 0, 0xe0, 0x07, 0, 0, 0, 0, 0, 0]))!;
    expect([...d5.data.subarray(0, 4)]).toEqual([0, 255, 0, 200]);
  });
  test("is null for anything else", () => {
    expect(decodeXpr(new Uint8Array(4))).toBeNull();
    const linear = xpr(2, 0x0c, [0, 0, 0, 0, 0, 0, 0, 0]);
    linear[25] = 0x12; // LIN_A8R8G8B8
    expect(decodeXpr(linear)).toBeNull();
    const short = xpr(6, 0x0c, [0, 0, 0, 0, 0, 0, 0, 0]).subarray(0, 0x50);
    expect(decodeXpr(short)).toBeNull();
  });
});
