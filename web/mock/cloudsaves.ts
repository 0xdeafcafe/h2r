// Cloud saves for the mock (docs/live/live-api.md "Cloud saves"), so the
// dashboard's Games screen lists and deletes saves without a server:
// `GET /v1/accounts/{xuid}/storage`, `GET` and `DELETE .../storage/{kind}/{name}`,
// for the account's own token only. Each account starts with a few invented
// saves (a profile named after it, two variants, a campaign save, one with
// a picture of its own); nothing here is the game's.

import { createHash } from "node:crypto";
import { parseJws } from "../src/state/verify";
import type { Reply } from "./admin";
import type { World } from "./world";

const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });
const QUOTA = 64 * 1024 * 1024;

interface Item { kind: string; name: string; version: number; sha256?: string; size: number; modified: number; stored_at: string; deleted?: boolean; bytes?: Uint8Array }

/** SaveMeta.xbx as xapi's `encode_meta` writes it. */
function meta(name: string): Uint8Array {
  const text = `﻿Name=${name}\r\n`;
  const out = new Uint8Array(text.length * 2);
  for (let i = 0; i < text.length; i++) { out[i * 2] = text.charCodeAt(i) & 0xff; out[i * 2 + 1] = text.charCodeAt(i) >> 8; }
  return out;
}

/** An invented 64x64 DXT1 save picture: two colours in diagonal bands. */
function picture(): Uint8Array {
  const header = 0x40, data = 16 * 16 * 8, out = new Uint8Array(header + data), dv = new DataView(out.buffer);
  out.set([0x58, 0x50, 0x52, 0x30]);
  const format = (6 << 24) | (6 << 20) | (1 << 16) | (0x0c << 8) | (2 << 4) | 1;
  [header + data, header, 0x00040001, 0, 0, format, 0, 0xffffffff].forEach((v, i) => dv.setUint32(4 + i * 4, v >>> 0, true));
  for (let b = 0; b < 256; b++) {
    const o = header + b * 8, band = ((b % 16) + (b >> 4)) % 4;
    dv.setUint16(o, [0xfd20, 0x2b05, 0xffe0, 0x0410][band]!, true);
    dv.setUint16(o + 2, 0x0000, true);
    dv.setUint32(o + 4, 0, true);
  }
  return out;
}

const dirOf = (name: string) => createHash("sha256").update(name).digest("hex").slice(0, 12).toUpperCase();

export class CloudSaves {
  readonly accounts = new Map<string, Map<string, Item>>();

  constructor(private world: World) {}

  private seed(owner: string): Map<string, Item> {
    let items = this.accounts.get(owner);
    if (items) return items;
    items = new Map();
    const now = Date.now(), day = 86_400_000;
    const put = (name: string, bytes: Uint8Array, at: number) => items!.set(`save/${name}`, {
      kind: "save", name, version: 1, sha256: createHash("sha256").update(bytes).digest("hex"), size: bytes.length,
      modified: at, stored_at: new Date(at).toISOString(), bytes,
    });
    const container = (title: string, payload: Array<[string, number]>, at: number, own?: Uint8Array) => {
      const dir = dirOf(title);
      put(`${dir}/SaveMeta.xbx`, meta(title), at);
      for (const [file, size] of payload) put(`${dir}/${file}`, new Uint8Array(size).fill(7), at);
      if (own) put(`${dir}/SaveImage.xbx`, own, at);
    };
    const tag = this.world.accounts.get(owner)?.gamertag ?? "Player";
    container(tag, [["profile", 12_288]], now - 2 * 3600_000);
    container("Rocket Ball Mk II", [["variant", 0x144]], now - 3 * day, picture());
    container("Sniper Duel", [["variant", 0x144]], now - 9 * day);
    container("Campaign: Heroic", [["checkpoint", 0x3fe000]], now - day);
    put("TitleMeta.xbx", meta("Halo 2"), now - 30 * day);
    items.set("settings/" + Buffer.from(tag).toString("hex"), { kind: "settings", name: Buffer.from(tag).toString("hex"), version: 1, sha256: "0".repeat(64), size: 40, modified: now, stored_at: new Date(now).toISOString(), bytes: new Uint8Array(40) });
    this.accounts.set(owner, items);
    return items;
  }

  handle(req: Request, u: URL): Reply | Response | null {
    const m = u.pathname.match(/^\/v1\/accounts\/([0-9a-f]{16})\/storage(?:\/(save|settings)\/(.+))?$/);
    if (!m) return null;
    const claims = parseJws((req.headers.get("authorization") ?? "").replace(/^Bearer /, "").trim())?.payload as { iss?: string; lxm?: string } | undefined;
    const caller = /^acct:([0-9a-f]{16})@/.exec(claims?.iss ?? "")?.[1];
    const owner = m[1]!;
    if (!caller || caller !== owner) return fail(403, "forbidden", "cloud saves are their owner's");
    if (!this.world.accounts.has(owner)) return fail(404, "not_found", "no such account");
    const items = this.seed(owner);
    const wire = ({ bytes: _b, ...i }: Item) => i;
    if (!m[2]) {
      if (req.method !== "GET") return fail(404, "not_found", "no route");
      if (claims?.lxm !== "storage.read") return fail(401, "unauthorized", "this needs a storage.read token");
      const list = [...items.values()];
      return { status: 200, body: { items: list.map(wire), used_bytes: list.filter((i) => !i.deleted).reduce((n, i) => n + i.size, 0), quota_bytes: QUOTA, item_bytes: 8 * 1024 * 1024 } };
    }
    const key = `${m[2]}/${m[3]!.split("/").map(decodeURIComponent).join("/")}`;
    const it = items.get(key);
    if (req.method === "GET") {
      if (claims?.lxm !== "storage.read") return fail(401, "unauthorized", "this needs a storage.read token");
      if (!it || it.deleted) return fail(404, "not_found", "no such item");
      return new Response(it.bytes as Uint8Array<ArrayBuffer>, { headers: { etag: `"${it.version}"`, "cache-control": "private, no-store" } });
    }
    if (req.method === "DELETE") {
      if (claims?.lxm !== "storage.write") return fail(401, "unauthorized", "this needs a storage.write token");
      if (!it || it.deleted) return fail(404, "not_found", "no such item");
      if (req.headers.get("if-match")?.replace(/"/g, "") !== String(it.version)) return { status: 409, body: { error: "conflict", message: "changed since", current: wire(it) } };
      const tomb: Item = { kind: it.kind, name: it.name, version: it.version + 1, size: 0, modified: Date.now(), stored_at: new Date().toISOString(), deleted: true };
      items.set(key, tomb);
      return { status: 200, body: wire(tomb) };
    }
    return fail(404, "not_found", "no route");
  }
}
