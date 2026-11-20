// Web Theater uses the archive of a finished game and named ranges of it.
// Reads use the usual single-use read token, so game visibility also applies
// to film lists and shared clips.

import { issueToken } from "../admin/token";
import type { LocalAccount } from "../state/account";
import type { TickRange } from "../state/replay";
import { getJson, requireOk, type Page, type Reader } from "./http";
import type { HostFilmSummary } from "./host-film";
import type { Xuid } from "./types";

export type FilmKind = "recent" | "saved";

export interface FilmSummary {
  has_host_film?: boolean;
  ticks_per_second?: number | null;
  summary?: HostFilmSummary | null;
  game_id: string;
  map: string;
  mode: string;
  variant: string | null;
  started_at: string;
  duration_s: number | null;
  result: "win" | "loss" | "draw" | null;
}

export interface ClipInput extends TickRange {
  title: string;
  pov?: number | null;
  camera?: string | null;
}

export interface Clip extends ClipInput {
  id: string;
  game: string;
  by: Xuid;
  created_ms: number;
}

export interface PlayerClip {
  clip: Clip;
  film: FilmSummary;
}

interface FilmWriter { api: string; aud: string; me: LocalAccount }

export function filmListUrl(api: string, player: Xuid, kind: FilmKind | "clips"): string {
  const url = new URL(api + "/films");
  url.searchParams.set("player", player);
  url.searchParams.set("kind", kind);
  return url.toString();
}

export const filmSaveUrl = (api: string, game: string): string => `${api}/films/${encodeURIComponent(game)}/save`;
export const gameClipsUrl = (api: string, game: string): string => `${api}/films/${encodeURIComponent(game)}/clips`;
export const clipUrl = (api: string, id: string): string => `${api}/clips/${encodeURIComponent(id)}`;

async function readPages<T>(url: string, reader: Reader, signal?: AbortSignal): Promise<T[]> {
  const out: T[] = [];
  const pageUrl = new URL(url);
  let cursor: string | null = null;
  for (let i = 0; i < 50; i++) {
    if (cursor !== null) pageUrl.searchParams.set("cursor", cursor);
    const page = await getJson<Page<T>>(pageUrl.toString(), signal, await reader());
    out.push(...page.items);
    if (page.next === null || page.next === cursor) break;
    cursor = page.next;
  }
  return out;
}

export async function readFilms(api: string, reader: Reader, player: Xuid, kind: FilmKind, signal?: AbortSignal): Promise<FilmSummary[]> {
  return readPages<FilmSummary>(filmListUrl(api, player, kind), reader, signal);
}

export async function readPlayerClips(api: string, reader: Reader, player: Xuid, signal?: AbortSignal): Promise<PlayerClip[]> {
  return readPages<PlayerClip>(filmListUrl(api, player, "clips"), reader, signal);
}

export async function readGameClips(api: string, reader: Reader, game: string, signal?: AbortSignal): Promise<Clip[]> {
  return readPages<Clip>(gameClipsUrl(api, game), reader, signal);
}

export async function readClip(api: string, reader: Reader, id: string, signal?: AbortSignal): Promise<Clip> {
  return getJson<Clip>(clipUrl(api, id), signal, await reader());
}

async function write(ctx: FilmWriter, url: string, method: "POST" | "DELETE", body?: ClipInput): Promise<Response> {
  const bearer = await issueToken(ctx.me, ctx.aud, "films.write");
  const headers: Record<string, string> = { accept: "application/json", authorization: `Bearer ${bearer}` };
  if (body) headers["content-type"] = "application/json";
  return requireOk(await fetch(url, { method, headers, body: body && JSON.stringify(body), cache: "no-store" }));
}

export async function setFilmSaved(ctx: FilmWriter, game: string, saved: boolean): Promise<void> {
  await write(ctx, filmSaveUrl(ctx.api, game), saved ? "POST" : "DELETE");
}

export async function createClip(ctx: FilmWriter, game: string, clip: ClipInput): Promise<Clip> {
  const r = await write(ctx, gameClipsUrl(ctx.api, game), "POST", clip);
  return await r.json() as Clip;
}

export async function deleteClip(ctx: FilmWriter, id: string): Promise<void> {
  await write(ctx, clipUrl(ctx.api, id), "DELETE");
}
