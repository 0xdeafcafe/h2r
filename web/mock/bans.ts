// Bans for the mock (docs/live/live-admin.md "Bans"), so the dashboard's
// ban list (/dash/server/bans), the Ban buttons on reports and players and
// a banned player's notice run without a server: `GET /v1/admin/bans`,
// ban and lift, `GET /v1/accounts/by-xuid/{xuid}/ban`, and the public
// `GET /v1/accounts/{who}/commendations` over mock/reports.ts. One
// invented, already-ended ban to start; admins as in mock/admin.ts
// (H2_MOCK_ADMIN=1). The mock refuses nothing else for a banned account.

import { parseJws } from "../src/state/verify";
import type { Admin, Reply } from "./admin";
import type { Reports } from "./reports";
import type { World } from "./world";

const ok = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });

interface Who { xuid: string; gamertag: string }
interface MockBan {
  id: number;
  target: Who;
  reason: string;
  until?: string;
  report?: number;
  by?: string;
  at: string;
  lifted?: { note?: string; by?: string; at: string };
  replaced?: boolean;
}

export class Bans {
  readonly bans: MockBan[] = [];
  private next = 1000;

  constructor(private world: World, private admin: Admin, private reports: Reports) {
    const someone = [...world.accounts.values()][3];
    if (someone) {
      const at = (minAgo: number) => new Date(Date.now() - minAgo * 60_000).toISOString();
      this.bans.push({ id: this.next++, target: { xuid: someone.xuid, gamertag: someone.gamertag }, reason: "Cooling off after a heated game.", until: at(60), at: at(60 * 25) });
    }
  }

  state(b: MockBan, nowMs = Date.now()): "active" | "expired" | "lifted" | "replaced" {
    if (b.lifted) return "lifted";
    if (b.replaced) return "replaced";
    if (b.until && Date.parse(b.until) <= nowMs) return "expired";
    return "active";
  }

  /** The ban in force on `xuid`, if any. */
  inForce(xuid: string): MockBan | undefined {
    return this.bans.find((b) => b.target.xuid === xuid && this.state(b) === "active");
  }

  view(state: string) {
    const items = this.bans.filter((b) => state === "all" || this.state(b) === "active").sort((a, b) => b.id - a.id)
      .map((b) => ({ ...b, replaced: undefined, state: this.state(b) }));
    return { items, position: this.world.log.length };
  }

  private gamertagOf(author: string): string | undefined {
    const xuid = /^acct:([0-9a-f]{16})@/.exec(author)?.[1];
    return xuid ? this.world.accounts.get(xuid)?.gamertag : undefined;
  }

  async handle(req: Request, u: URL): Promise<Reply | null> {
    const path = u.pathname;
    let m: RegExpMatchArray | null;
    if ((m = path.match(/^\/v1\/accounts\/([^/]+)\/commendations$/)) && req.method === "GET") {
      const who = decodeURIComponent(m[1]!);
      const a = this.world.accounts.get(who) ?? [...this.world.accounts.values()].find((x) => x.gamertag.toLowerCase() === who.toLowerCase());
      if (!a) return fail(404, "not_found", "no such account");
      const of = (kind: string) => this.reports.reports.filter((r) => r.target.xuid === a.xuid && r.kind === kind).length;
      return ok({ xuid: a.xuid, good_attitude: of("good_attitude"), good_session: of("good_session") });
    }
    if ((m = path.match(/^\/v1\/accounts\/by-xuid\/([0-9a-f]{16})\/ban$/)) && req.method === "GET") {
      const claims = parseJws((req.headers.get("authorization") ?? "").replace(/^Bearer /, "").trim())?.payload as { iss?: string; lxm?: string } | undefined;
      const me = /^acct:([0-9a-f]{16})@/.exec(claims?.iss ?? "")?.[1];
      if (!me || claims?.lxm !== "read") return fail(401, "unauthorized", "this needs a read token");
      if (!this.world.accounts.has(m[1]!)) return fail(404, "not_found", "no such account");
      if (me !== m[1] && !this.admin.isAdmin(this.world.accounts.get(me))) return fail(403, "forbidden", "only the player and admins read a ban");
      const b = this.inForce(m[1]!);
      return ok(b ? { banned: true, ban: { id: b.id, reason: b.reason, ...(b.until ? { until: b.until } : {}), at: b.at } } : { banned: false });
    }
    if (!path.startsWith("/v1/admin/bans")) return null;
    const c = this.admin.caller(req.headers.get("authorization"), "reports.moderate");
    if (!c.ok) return c.reply;
    if (path === "/v1/admin/bans" && req.method === "GET") {
      const state = u.searchParams.get("state") ?? "active";
      if (state !== "active" && state !== "all") return fail(400, "invalid", "state is active or all");
      return ok(this.view(state));
    }
    if ((m = path.match(/^\/v1\/admin\/bans\/([0-9a-f]{16})(\/lift)?$/)) && req.method === "POST") {
      const target = this.world.accounts.get(m[1]!);
      if (!target) return fail(404, "not_found", "no such account");
      const b = (await req.json().catch(() => null)) as { reason?: unknown; until?: unknown; report?: unknown; note?: unknown } | null;
      const by = this.gamertagOf(c.author);
      if (m[2]) {
        const ban = this.inForce(target.xuid);
        if (!ban) return fail(409, "conflict", "that account is not banned");
        const note = typeof b?.note === "string" ? b.note.trim() : "";
        ban.lifted = { ...(note ? { note } : {}), ...(by ? { by } : {}), at: new Date().toISOString() };
        return ok({ ok: true, position: this.world.log.length });
      }
      if (this.admin.isAdmin(target)) return fail(403, "forbidden", "admins can't be banned");
      const reason = typeof b?.reason === "string" ? b.reason.trim() : "";
      if (!reason) return fail(400, "invalid", "a ban needs a reason");
      if (b?.until !== undefined && (typeof b.until !== "string" || !(Date.parse(b.until) > Date.now()))) return fail(400, "invalid", "until is an RFC 3339 time in the future");
      for (const old of this.bans) if (old.target.xuid === target.xuid) old.replaced = true;
      const ban: MockBan = {
        id: this.next++, target: { xuid: target.xuid, gamertag: target.gamertag }, reason,
        ...(typeof b?.until === "string" ? { until: b.until } : {}),
        ...(typeof b?.report === "number" ? { report: b.report } : {}),
        ...(by ? { by } : {}), at: new Date().toISOString(),
      };
      this.bans.push(ban);
      return ok({ ok: true, position: ban.id }, 201);
    }
    return fail(404, "not_found", `no route ${req.method} ${path}`);
  }
}
