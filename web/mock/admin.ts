// The server screen's endpoints for the mock (docs/live/live-admin.md §5):
// GET /v1/server, GET /v1/admin/status, and admin invites (GET/POST
// /v1/invites, POST /v1/invites/{jti}/revoke). Invite codes are short:
// generated ones 25 characters in five groups, or the admin's own (sent as
// JSON {event, code}), matched ignoring case, spaces and dashes. The usage
// reads (GET /v1/admin/usage, /v1/admin/usage/accounts) answer from the
// deck's seeded sample, not the mock's own traffic.
//
// H2_MOCK_ADMIN=1 makes every account signed up in the browser an admin
// (their account reads carry `admin: true`, so the dashboard and the stats
// site show the admin screens). Mock only: tokens and events are parsed,
// not verified; the real server checks both signatures and the admin.

import { HOST, type World } from "./world";
import { parseJws } from "../src/state/verify";
import { mockOverview, usageMock } from "../src/deck/mock";
import { ACCOUNT_SORTS, type AccountUsage } from "../src/admin/usage";

const MAX_DAYS = 30;

export interface Reply { status: number; body: unknown }
const ok = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });

interface MockInvite { jti: string; by: string; at: string; exp: number; position: number; revoked: boolean; code: string; custom: boolean }

/** The generated codes' alphabet: no vowels, no look-alikes. */
export const INVITE_ALPHABET = "BCDFGHJKMPQRTVWXY2346789";

export function generateInviteCode(): string {
  const b = crypto.getRandomValues(new Uint8Array(25));
  const chars = [...b].map((x) => INVITE_ALPHABET[x % INVITE_ALPHABET.length]).join("");
  return chars.match(/.{5}/g)!.join("-");
}

/** How codes compare: case, spaces and dashes don't matter. */
export function inviteKey(code: string): string {
  return code.replace(/[\s-]/g, "").toUpperCase();
}

export class Admin {
  readonly startedAt = Date.now();
  private invites = new Map<string, MockInvite>();
  constructor(private world: World, readonly everyoneAdmin: boolean) {}

  /** Whether an account is an admin here: browser sign-ups, with H2_MOCK_ADMIN=1. */
  isAdmin(a: { external?: boolean } | undefined): boolean {
    return this.everyoneAdmin && !!a?.external;
  }

  /** The account behind a bearer token, if it is an admin (not verified). */
  caller(auth: string | null, lxm: string): { ok: true; author: string } | { ok: false; reply: Reply } {
    const t = auth?.startsWith("Bearer ") ? parseJws(auth.slice(7).trim()) : null;
    const claims = t?.payload as { iss?: string; lxm?: string } | undefined;
    if (!claims?.iss) return { ok: false, reply: fail(401, "unauthorized", "this needs Authorization: Bearer <token>") };
    if (claims.lxm !== lxm) return { ok: false, reply: fail(401, "unauthorized", `token lxm is not ${lxm}`) };
    const xuid = /^acct:([0-9a-f]{16})@/.exec(claims.iss)?.[1];
    const a = xuid ? this.world.accounts.get(xuid) : undefined;
    if (!this.isAdmin(a)) return { ok: false, reply: fail(403, "forbidden", "this needs an admin (mock: run with H2_MOCK_ADMIN=1)") };
    return { ok: true, author: claims.iss };
  }

  private facts() {
    const accounts = [...this.world.accounts.values()];
    return {
      id: HOST, software: "h2recomp web mock 0.1", url: null, invite_only: false, private: false, federation: false,
      counts: {
        accounts: accounts.length,
        online: accounts.filter((a) => a.online).length,
        live_games: [...this.world.games.values()].filter((g) => !g.ended_at).length,
        sessions: 0,
      },
      position: this.world.log.length,
    };
  }

  private state(i: MockInvite): string {
    return i.revoked ? "revoked" : i.exp < Date.now() / 1000 ? "expired" : "open";
  }

  /** The open invite a typed code names, if any. */
  inviteFor(code: string): MockInvite | undefined {
    const k = inviteKey(code);
    return [...this.invites.values()].find((i) => inviteKey(i.code) === k && this.state(i) === "open");
  }

  async handle(req: Request, u: URL): Promise<Reply | null> {
    const path = u.pathname;
    const auth = req.headers.get("authorization");
    if (path === "/v1/server" && req.method === "GET") return ok(this.facts());
    if (path === "/v1/admin/status" && req.method === "GET") {
      const c = this.caller(auth, "admin.read");
      if (!c.ok) return c.reply;
      const counts = { open: 0, used: 0, revoked: 0, expired: 0 } as Record<string, number>;
      for (const i of this.invites.values()) counts[this.state(i)]! += 1;
      const admins = [...this.world.accounts.values()].filter((a) => this.isAdmin(a)).map((a) => ({ gamertag: a.gamertag, source: "configured" }));
      return ok({
        ...this.facts(),
        started_at: new Date(this.startedAt).toISOString(),
        uptime_s: Math.floor((Date.now() - this.startedAt) / 1000),
        public: { url: null, tunnel: null },
        game_art: { present: false, shared: false },
        admins,
        log: { position: this.world.log.length, head: this.world.log.length, store: "memory" },
        invites: counts,
        // A local server shows its data directory to a browser on the same machine.
        data_dir: "~/Library/Application Support/h2recomp/live/server",
      });
    }
    if ((path === "/v1/admin/usage" || path === "/v1/admin/usage/accounts") && req.method === "GET") {
      const c = this.caller(auth, "admin.read");
      if (!c.ok) return c.reply;
      return usageReply(path, u.searchParams);
    }
    if (path === "/v1/invites" && req.method === "GET") {
      const c = this.caller(auth, "admin.read");
      if (!c.ok) return c.reply;
      const items = [...this.invites.values()].sort((a, b) => b.position - a.position).map((i) => {
        const state = this.state(i);
        const tag = this.world.accounts.get(/^acct:([0-9a-f]{16})@/.exec(i.by)?.[1] ?? "")?.gamertag;
        return { jti: i.jti, by: i.by, by_gamertag: tag, at: i.at, exp: i.exp, state, custom: i.custom, invite: state === "open" && !i.custom ? i.code : null };
      });
      return ok({ items, position: this.world.log.length });
    }
    if (path === "/v1/invites" && req.method === "POST") {
      const c = this.caller(auth, "invites.write");
      if (!c.ok) return c.reply;
      const text = (await req.text()).trim();
      let compact = text, custom: string | null = null;
      if (text.startsWith("{")) {
        let b: { event?: unknown; code?: unknown };
        try { b = JSON.parse(text); } catch { return fail(400, "invalid", "expected a JWS or JSON {event, code}"); }
        if (typeof b.event !== "string" || typeof b.code !== "string") return fail(400, "invalid", "expected JSON {event, code}");
        if (b.code.length < 4 || b.code.length > 32 || !/^[\x20-\x7e]+$/.test(b.code)) return fail(400, "invalid", "a code is 4 to 32 printable ASCII characters");
        compact = b.event;
        custom = b.code;
      }
      const ev = parseJws(compact)?.payload as { stream?: string; type?: string; author?: string; at?: string; data?: { jti?: unknown; exp?: unknown } } | undefined;
      if (!ev || ev.stream !== "invites" || ev.type !== "InviteCreated") return fail(400, "invalid", "expected an InviteCreated event on invites");
      if (ev.author !== c.author) return fail(403, "forbidden", "the event's author is not the token's issuer");
      const { jti, exp } = ev.data ?? {};
      const now = Date.now() / 1000;
      if (typeof jti !== "string" || jti.length < 16) return fail(400, "invalid", "jti is 16 to 64 of [A-Za-z0-9_-]");
      if (typeof exp !== "number" || exp <= now || exp > now + MAX_DAYS * 86400 + 300) return fail(400, "invalid", `exp is in the future, at most ${MAX_DAYS} days`);
      if (this.invites.has(jti)) return fail(409, "conflict", "that jti is taken");
      if (custom !== null && this.inviteFor(custom)) return fail(409, "conflict", "an open invite already has that code");
      const [w] = this.world.append("invites", [{ type: "InviteCreated", data: { jti, exp } }], null);
      const i: MockInvite = {
        jti, by: c.author, at: ev.at ?? new Date().toISOString(), exp, position: w?.position ?? this.world.log.length, revoked: false,
        code: custom ?? generateInviteCode(), custom: custom !== null,
      };
      this.invites.set(jti, i);
      return ok({ invite: i.code, jti, exp, position: i.position, custom: i.custom }, 201);
    }
    const m = path.match(/^\/v1\/invites\/([^/]+)\/revoke$/);
    if (m && req.method === "POST") {
      const c = this.caller(auth, "invites.write");
      if (!c.ok) return c.reply;
      const jti = decodeURIComponent(m[1]!);
      const i = this.invites.get(jti);
      if (!i) return fail(404, "not_found", "no such invite");
      if (this.state(i) !== "open") return fail(409, "conflict", `that invite is ${this.state(i)}`);
      i.revoked = true;
      const [w] = this.world.append("invites", [{ type: "InviteRevoked", data: { jti } }], null);
      return ok({ jti, state: "revoked", position: w?.position ?? this.world.log.length });
    }
    return null;
  }
}

/** The usage endpoints (docs/live/live-api.md "Usage") on the deck's fixed
 *  sample, ending today: plausible numbers to lay the screen out against,
 *  not this mock's own traffic. */
export function usageReply(path: string, q: URLSearchParams, today = new Date().toISOString().slice(0, 10)): Reply {
  const days = q.has("days") ? Number(q.get("days")) : 30;
  if (!Number.isInteger(days) || days < 1 || days > 400) return fail(400, "bad_request", "days is 1 to 400");
  const sample = usageMock(today, 400);
  if (path === "/v1/admin/usage") return ok(mockOverview(sample, days));
  const sort = (q.get("sort") ?? "uploaded") as (typeof ACCOUNT_SORTS)[number];
  if (!ACCOUNT_SORTS.includes(sort)) return fail(400, "bad_request", `sort is one of ${ACCOUNT_SORTS.join(", ")}`);
  const limit = q.has("limit") ? Number(q.get("limit")) : 50;
  if (!Number.isInteger(limit) || limit < 1 || limit > 500) return fail(400, "bad_request", "limit is 1 to 500");
  const key = (a: AccountUsage): number =>
    sort === "stored" ? a.stored_total : sort === "messages" ? a.messages_total : (a[sort] as number);
  const items = [...sample.accounts].sort((a, b) => key(b) - key(a));
  return ok({ days, sort, total: items.length, items: items.slice(0, limit) });
}
