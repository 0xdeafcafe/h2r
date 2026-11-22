// The admin API (docs/live/live-admin.md): the MOTD and the hopper file as
// structured JSON. Reads are public like every read; writes carry a
// self-issued token signed with the signed-in account's key (./token.ts),
// and the server accepts it only for admins.

import { ApiError } from "../api/http";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { issueToken } from "./token";
import { parsePins } from "./catalog";
import { CONTENT_MODERATE } from "./content";
import type { BuiltinVariant, HoppersPatch, HopperView, MotdDoc, PlaylistTemplate, RetailMap } from "./validate";

export interface HoppersReply {
  hoppers: HopperView[];
  /** What the builder can pick without the owner's catalog (older servers
   *  send none). */
  variants?: BuiltinVariant[];
  templates?: PlaylistTemplate[];
  maps?: RetailMap[];
  position?: number;
}

export interface PublishedInfo {
  name: string;
  sha256: string;
  size: number;
}

async function parseError(r: Response): Promise<never> {
  let code = "http_" + r.status;
  let message = r.statusText;
  try {
    const b = await r.json();
    if (typeof b?.error === "string") {
      code = b.error;
      message = b.message ?? message;
    }
  } catch {
    /* not JSON */
  }
  throw new ApiError(r.status, code, message);
}

/** A write: signed per request (the `jti` is single use). */
async function put<T>(path: string, body: unknown, lxm = "titles.write"): Promise<T> {
  const me = identity.me();
  const wk = live.wellKnown();
  if (!me || !wk) throw new ApiError(401, "unauthorized", "sign in first");
  const url = `${live.server().api}${path}`;
  const token = await issueToken(me, wk.id, lxm);
  const r = await fetch(url, {
    method: "PUT",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify(body),
  });
  if (!r.ok) return parseError(r);
  return (await r.json()) as T;
}

/** The published MOTD, or null when none has been published yet. */
export async function getMotd(): Promise<MotdDoc | null> {
  const r = await fetch(`${live.server().api}/titles/${title()}/motd`, {
    headers: { accept: "application/json" },
  });
  if (r.status === 404) return null;
  if (!r.ok) return parseError(r);
  return (await r.json()) as MotdDoc;
}

export async function putMotd(doc: MotdDoc): Promise<{ published: PublishedInfo; motd: MotdDoc }> {
  return put(`/titles/${title()}/motd`, doc);
}

/** The 16 playlists, or null until a base image has been imported
 *  (`h2 live hopper import`). */
export async function getHopper(): Promise<HoppersReply | null> {
  const r = await fetch(`${live.server().api}/titles/${title()}/hopper`, {
    headers: { accept: "application/json" },
  });
  if (r.status === 404) return null;
  if (!r.ok) return parseError(r);
  return (await r.json()) as HoppersReply;
}

export async function putHopper(patch: HoppersPatch): Promise<HoppersReply & { published: PublishedInfo }> {
  return put(`/titles/${title()}/hopper`, patch);
}

/** Which custom maps (map_ids) each hopper allows; a public read. An
 *  older server without pins answers 404: no pins. */
export async function getPins(): Promise<Map<number, string[]>> {
  const r = await fetch(`${live.server().api}/hoppers/pins`, { headers: { accept: "application/json" } });
  if (r.status === 404) return new Map();
  if (!r.ok) return parseError(r);
  return parsePins(await r.json());
}

/** Sets one hopper's pinned custom maps (admins, `content.moderate`). */
export async function putPins(hopperId: number, mapIds: string[]): Promise<{ ok: boolean; position?: number }> {
  return put(`/hoppers/${hopperId}/pins`, { map_ids: mapIds }, CONTENT_MODERATE);
}

/** Halo 2's title id; the editors edit Halo 2's files. */
function title(): string {
  return "4D530064";
}
