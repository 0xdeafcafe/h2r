// Mute lists for the mock (docs/live/live-api.md "Mute list"), so the
// dashboard's Mute and Unmute run without a server: `GET`, `PUT` and
// `DELETE /v1/accounts/by-xuid/{xuid}/mutes[/{target}]`, for the account's
// own token (admins as in mock/admin.ts may read). Lists start empty.

import { parseJws } from "../src/state/verify";
import type { Admin, Reply } from "./admin";
import type { World } from "./world";

const ok = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });
const MAX = 250;

export class Mutes {
  readonly lists = new Map<string, string[]>();

  constructor(private world: World, private admin: Admin) {}

  handle(req: Request, u: URL): Reply | null {
    const m = u.pathname.match(/^\/v1\/accounts\/by-xuid\/([0-9a-f]{16})\/mutes(?:\/([0-9a-f]{16}|[^/]+))?$/);
    if (!m) return null;
    const claims = parseJws((req.headers.get("authorization") ?? "").replace(/^Bearer /, "").trim())?.payload as { iss?: string; lxm?: string } | undefined;
    const caller = /^acct:([0-9a-f]{16})@/.exec(claims?.iss ?? "")?.[1];
    if (!caller) return fail(401, "unauthorized", "this needs an account token");
    const owner = m[1]!, target = m[2];
    if (!this.world.accounts.has(owner)) return fail(404, "not_found", "no such account");
    const list = this.lists.get(owner) ?? [];
    if (!target && req.method === "GET") {
      if (claims?.lxm !== "read") return fail(401, "unauthorized", "this needs a read token");
      if (caller !== owner && !this.admin.isAdmin(this.world.accounts.get(caller))) return fail(403, "forbidden", "a mute list is its owner's");
      return ok({ items: list.map((x) => ({ xuid: x, gamertag: this.world.accounts.get(x)?.gamertag ?? "" })), position: this.world.log.length });
    }
    if (!target || (req.method !== "PUT" && req.method !== "DELETE")) return fail(404, "not_found", `no route ${req.method} ${u.pathname}`);
    if (claims?.lxm !== "mutes.write") return fail(401, "unauthorized", "this needs a mutes.write token");
    if (caller !== owner) return fail(403, "forbidden", "only the account changes its mute list");
    const mute = req.method === "PUT";
    if (mute && target === owner) return fail(400, "invalid", "a player can't mute themselves");
    if (mute && !this.world.accounts.has(target)) return fail(404, "not_found", "no such account");
    if (list.includes(target) === mute) return ok({ ok: true, changed: false });
    if (mute && list.length >= MAX) return fail(409, "conflict", `a mute list holds at most ${MAX} players`);
    this.lists.set(owner, mute ? [...list, target] : list.filter((x) => x !== target));
    return ok({ ok: true, position: this.world.log.length, changed: true }, mute ? 201 : 200);
  }
}
