// The signed-in player's cloud saves for the Games screen (api/cloudsaves.ts):
// the list, each save's name from its SaveMeta.xbx and its own save image
// when it has one, loaded once per account and server and again after a
// delete. Names and pictures are kept by content hash for the page's life,
// so a reload fetches only what changed.

import { createEffect, createSignal, on } from "solid-js";
import { identity } from "./identity";
import { live } from "./live";
import { deleteSave, fetchItem, fetchStorage, groupSaves, metaValue, sharedSaveImage, type CloudSave, type SavesCtx, type StorageItem, type StorageList } from "../api/cloudsaves";
import { decodeXpr, type Rgba } from "../lib/xpr";

const names = new Map<string, Promise<string | null>>();
const images = new Map<string, Promise<string | null>>();

const ctx = (): SavesCtx | null => {
  const me = identity.me(), wk = live.wellKnown();
  return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
};

/** A decoded picture as a PNG data URL; null without a canvas. */
function dataUrl(img: Rgba): string | null {
  if (typeof document === "undefined") return null;
  const c = document.createElement("canvas");
  c.width = img.width; c.height = img.height;
  const g = c.getContext("2d");
  if (!g) return null;
  g.putImageData(new ImageData(img.data as Uint8ClampedArray<ArrayBuffer>, img.width, img.height), 0, 0);
  return c.toDataURL("image/png");
}

function once(cache: Map<string, Promise<string | null>>, item: StorageItem, load: () => Promise<string | null>): Promise<string | null> {
  const key = item.sha256 ?? `${item.name}@${item.version}`;
  let p = cache.get(key);
  if (!p) {
    p = load().catch(() => { cache.delete(key); return null; });
    cache.set(key, p);
  }
  return p;
}

export interface SaveRow extends CloudSave {
  /** The SaveMeta.xbx name; null until read, or when it has none. */
  title: string | null;
  /** The save's own picture (or the title's shared one on U:), decoded. */
  picture: string | null;
}

/** The cloud saves of whoever is signed in. `state` is `none` with nobody
 *  signed in or a server that keeps no saves. */
export function createCloudSaves() {
  const [list, setList] = createSignal<StorageList | null>(null);
  const [rows, setRows] = createSignal<SaveRow[]>([]);
  const [state, setState] = createSignal<"loading" | "ready" | "none" | "error">("loading");
  const [err, setErr] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  let seq = 0;

  async function load() {
    const c = ctx(), n = ++seq;
    if (!c) { setList(null); setRows([]); setState("none"); return; }
    try {
      const l = await fetchStorage(c);
      if (n !== seq) return;
      setList(l);
      if (!l) { setRows([]); setState("none"); return; }
      const saves = groupSaves(l.items);
      const shared = sharedSaveImage(l.items);
      setRows(saves.map((s) => ({ ...s, title: null, picture: null })));
      setState("ready");
      setErr(null);
      const picture = (item: StorageItem) => once(images, item, async () => {
        const img = decodeXpr(await fetchItem(c, item));
        return img ? dataUrl(img) : null;
      });
      const sharedPicture = shared ? picture(shared) : Promise.resolve(null);
      await Promise.all(saves.map(async (s) => {
        const [title, own] = await Promise.all([
          s.meta ? once(names, s.meta, async () => metaValue(await fetchItem(c, s.meta!), "Name")) : null,
          s.image ? picture(s.image) : null,
        ]);
        const pic = own ?? (await sharedPicture);
        if (n !== seq) return;
        setRows((rs) => rs.map((r) => (r.dir === s.dir ? { ...r, title, picture: pic } : r)));
      }));
    } catch (x) {
      if (n !== seq) return;
      setState("error");
      setErr((x as Error).message);
    }
  }

  createEffect(on(() => [identity.me()?.xuid, live.server().base, live.wellKnown()?.id], () => { setState("loading"); void load(); }));

  async function remove(save: CloudSave) {
    const c = ctx();
    if (!c || busy()) return;
    setBusy(save.dir); setErr(null);
    try { await deleteSave(c, save); } catch (x) { setErr(`couldn't delete it: ${(x as Error).message}`); }
    await load();
    setBusy(null);
  }

  return { list, rows, state, err, busy, remove, reload: load };
}

