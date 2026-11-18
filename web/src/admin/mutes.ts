// The mute list (docs/live/live-api.md "Mute list"): the players an account
// muted, kept by the server so it follows the player between the game and
// here. The game builds its voice mute matrix from it, and a mute holds both
// ways. The list is read with the account's `read` token and changed with
// its `mutes.write` token; only the account itself (and admins, reading)
// see it. Parsing is defensive like ./bans.ts.

import type { LocalAccount } from "../state/account";
import { adminCall } from "./content";
import type { Who } from "./reports";

export const MUTES_WRITE = "mutes.write";
/** The game's list size, and the server's cap. */
export const MAX_MUTES = 250;

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const str = (v: unknown): string => (typeof v === "string" ? v : "");

/** `GET /v1/accounts/by-xuid/{xuid}/mutes`, oldest first. */
export function parseMutes(body: unknown): Who[] {
  const items = obj(body).items;
  return (Array.isArray(items) ? items : [])
    .map(obj)
    .filter((i) => /^[0-9a-f]{16}$/.test(str(i.xuid)))
    .map((i) => ({ xuid: str(i.xuid), gamertag: str(i.gamertag) || "unknown" }));
}

const path = (owner: string, target?: string) =>
  `/accounts/by-xuid/${encodeURIComponent(owner)}/mutes${target ? `/${encodeURIComponent(target)}` : ""}`;

export async function fetchMutes(api: string, aud: string, me: LocalAccount, fetchFn: typeof fetch = fetch): Promise<Who[]> {
  return parseMutes(await adminCall(api, path(me.xuid), aud, me, "read", {}, fetchFn));
}

/** Mutes (or unmutes) `target` for `me`; whether anything changed. */
export async function setMute(api: string, aud: string, me: LocalAccount, target: string, mute: boolean, fetchFn: typeof fetch = fetch): Promise<boolean> {
  if (target === me.xuid) throw new Error("you can't mute yourself");
  const r = obj(await adminCall(api, path(me.xuid, target), aud, me, MUTES_WRITE, { method: mute ? "PUT" : "DELETE" }, fetchFn));
  return r.changed === true;
}
