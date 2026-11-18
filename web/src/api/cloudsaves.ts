// The signed-in player's cloud saves (docs/live/live-cloud-saves.md, the
// endpoints in docs/live/live-api.md "Cloud saves"), as the dashboard's
// Games screen shows them: one row per save container, the way the original
// Xbox dashboard's Memory screen listed a game's saves.
//
//   - GET    /v1/accounts/{xuid}/storage                (lxm storage.read)
//   - GET    /v1/accounts/{xuid}/storage/{kind}/{name}  (lxm storage.read)
//   - DELETE /v1/accounts/{xuid}/storage/{kind}/{name}  (lxm storage.write,
//            If-Match: <version>)
//
// A `save` item is one file below U:, named by its path: a save container
// is `<12 hex digits>/` holding `SaveMeta.xbx` (`Name=<save name>`, UTF-16LE)
// and the game's files, perhaps a `SaveImage.xbx` of its own. Files at the
// top (TitleMeta.xbx, TitleImage.xbx, SaveImage.xbx) are the title's.
// Parsing is pure and defensive; the fetches take a fetch function so tests
// can answer them.

import type { LocalAccount } from "../state/account";
import { issueToken } from "../admin/token";
import { ApiError } from "./http";

export const STORAGE_READ = "storage.read";
export const STORAGE_WRITE = "storage.write";
export const SAVE_META = "SaveMeta.xbx";
export const SAVE_IMAGE = "SaveImage.xbx";
/** The original dashboard counted saves in 16 KiB blocks. */
export const BLOCK_BYTES = 16 * 1024;

export interface StorageItem {
  kind: string;
  name: string;
  version: number;
  sha256?: string;
  size: number;
  /** The writer's time of the change, Unix milliseconds. */
  modified: number;
  deleted: boolean;
}

export interface StorageList {
  items: StorageItem[];
  used_bytes: number;
  /** 0 when the server says none. */
  quota_bytes: number;
}

/** One save container on U:. */
export interface CloudSave {
  /** The container's directory, 12 hex digits. */
  dir: string;
  /** Its live files, the metadata among them. */
  files: StorageItem[];
  size: number;
  /** The newest file's time, Unix milliseconds. */
  modified: number;
  meta: StorageItem | null;
  /** The container's own save image, when it has one. */
  image: StorageItem | null;
}

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : 0);

/** `GET .../storage`, dropping items it can't read. */
export function parseStorageList(body: unknown): StorageList {
  const b = obj(body);
  const items: StorageItem[] = [];
  for (const raw of Array.isArray(b.items) ? b.items : []) {
    const i = obj(raw);
    if (typeof i.kind !== "string" || typeof i.name !== "string" || !i.name) continue;
    const item: StorageItem = { kind: i.kind, name: i.name, version: num(i.version), size: num(i.size), modified: num(i.modified), deleted: i.deleted === true };
    if (typeof i.sha256 === "string") item.sha256 = i.sha256;
    items.push(item);
  }
  return { items, used_bytes: num(b.used_bytes), quota_bytes: num(b.quota_bytes) };
}

const CONTAINER = /^[0-9A-Fa-f]{12}$/;
const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** The live save containers, newest first. */
export function groupSaves(items: readonly StorageItem[]): CloudSave[] {
  const by = new Map<string, CloudSave>();
  for (const i of items) {
    if (i.kind !== "save" || i.deleted) continue;
    const [dir, ...rest] = i.name.split("/");
    if (!dir || !CONTAINER.test(dir) || !rest.length) continue;
    const key = dir.toUpperCase();
    let s = by.get(key);
    if (!s) by.set(key, (s = { dir, files: [], size: 0, modified: 0, meta: null, image: null }));
    s.files.push(i);
    s.size += i.size;
    s.modified = Math.max(s.modified, i.modified);
    const file = rest.join("/");
    if (same(file, SAVE_META)) s.meta = i;
    else if (same(file, SAVE_IMAGE)) s.image = i;
  }
  return [...by.values()].sort((a, b) => b.modified - a.modified || a.dir.localeCompare(b.dir));
}

/** The title's own save image at the top of U:, the picture of every save
 *  that has none of its own. */
export function sharedSaveImage(items: readonly StorageItem[]): StorageItem | null {
  return items.find((i) => i.kind === "save" && !i.deleted && same(i.name, SAVE_IMAGE)) ?? null;
}

/** Whole blocks a size takes, at least one. */
export function blocks(bytes: number): number {
  return Math.max(1, Math.ceil(bytes / BLOCK_BYTES));
}

/** `key=value` from a metadata file: two bytes the reader skips, then
 *  UTF-16LE lines ended by CRLF (xapi's `parse_meta`); at most 127 units. */
export function metaValue(bytes: Uint8Array, key: string): string | null {
  if (bytes.length < 2) return null;
  const units = new Uint16Array(Math.floor((bytes.length - 2) / 2));
  for (let i = 0; i < units.length; i++) units[i] = bytes[2 + i * 2]! | (bytes[3 + i * 2]! << 8);
  const text = String.fromCharCode(...units);
  for (const raw of text.split("\n")) {
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
    if (line.startsWith(key + "=")) return line.slice(key.length + 1, key.length + 1 + 127).replace(/\0+$/, "");
  }
  return null;
}

// ---- requests ----

export interface SavesCtx { api: string; aud: string; me: LocalAccount }

/** Each `/`-separated segment percent-encoded, as the API wants. */
export function itemPath(xuid: string, kind: string, name: string): string {
  return `/accounts/${encodeURIComponent(xuid)}/storage/${encodeURIComponent(kind)}/${name.split("/").map(encodeURIComponent).join("/")}`;
}

async function fail(r: Response): Promise<never> {
  let code = "http_error", message = `HTTP ${r.status}`;
  try {
    const b = obj(await r.json());
    if (typeof b.error === "string") { code = b.error; message = typeof b.message === "string" ? b.message : message; }
  } catch { /* not JSON */ }
  throw new ApiError(r.status, code, message);
}

async function call(c: SavesCtx, path: string, lxm: string, init: RequestInit, fetchFn: typeof fetch): Promise<Response> {
  const token = await issueToken(c.me, c.aud, lxm);
  const r = await fetchFn(`${c.api}${path}`, { cache: "no-store", ...init, headers: { authorization: `Bearer ${token}`, ...(init.headers as Record<string, string> | undefined) } });
  return r.ok ? r : fail(r);
}

/** The account's items; null from a server without cloud saves (404) or
 *  one that keeps none (503). */
export async function fetchStorage(c: SavesCtx, fetchFn: typeof fetch = fetch): Promise<StorageList | null> {
  try {
    const r = await call(c, `/accounts/${encodeURIComponent(c.me.xuid)}/storage`, STORAGE_READ, { headers: { accept: "application/json" } }, fetchFn);
    return parseStorageList(await r.json());
  } catch (e) {
    if (e instanceof ApiError && (e.status === 404 || e.status === 503)) return null;
    throw e;
  }
}

/** One item's bytes. */
export async function fetchItem(c: SavesCtx, item: Pick<StorageItem, "kind" | "name">, fetchFn: typeof fetch = fetch): Promise<Uint8Array> {
  const r = await call(c, itemPath(c.me.xuid, item.kind, item.name), STORAGE_READ, {}, fetchFn);
  return new Uint8Array(await r.arrayBuffer());
}

/** Deletes every file of a save, each against the version listed, so a
 *  file another machine changed since is left alone (409, thrown). The
 *  metadata goes last, and only once the rest are gone: a save cut short
 *  keeps its name and can be deleted again. */
export async function deleteSave(c: SavesCtx, save: CloudSave, fetchFn: typeof fetch = fetch): Promise<void> {
  const del = async (f: StorageItem) => {
    try {
      await call(c, itemPath(c.me.xuid, f.kind, f.name), STORAGE_WRITE, { method: "DELETE", headers: { "if-match": String(f.version) } }, fetchFn);
    } catch (e) {
      if (!(e instanceof ApiError && e.status === 404)) throw e; // 404: already gone
    }
  };
  let first: unknown = null;
  for (const f of save.files) {
    if (f === save.meta) continue;
    try { await del(f); } catch (e) { first ??= e; }
  }
  if (first) throw first;
  if (save.meta) await del(save.meta);
}
