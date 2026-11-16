// The server screen's endpoints for the mock (docs/live-admin.md §5):
// GET /v1/server, GET /v1/admin/status, and admin invites (GET/POST
// /v1/invites, POST /v1/invites/{jti}/revoke).
//
// H2_MOCK_ADMIN=1 makes every account signed up in the browser an admin
// (their account reads carry `admin: true`, so the dashboard and the stats
// site show the admin screens). Mock only: tokens and events are parsed,
// not verified; the real server checks both signatures and the admin.

import { HOST, type World } from "./world";
import { signJws } from "./sign";
import { parseJws } from "../src/state/verify";

const MAX_DAYS = 30;

export interface Reply { status: number; body: unknown }
const ok = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });

interface MockInvite { jti: string; by: string; at: string; exp: number; position: number; revoked: boolean }

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

  private code(i: MockInvite): string {
    return signJws(this.world.serverKey, { aud: HOST, jti: i.jti, exp: i.exp });
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
    if (path === "/v1/invites" && req.method === "GET") {
      const c = this.caller(auth, "admin.read");
      if (!c.ok) return c.reply;
      const items = [...this.invites.values()].sort((a, b) => b.position - a.position).map((i) => {
        const state = this.state(i);
        const tag = this.world.accounts.get(/^acct:([0-9a-f]{16})@/.exec(i.by)?.[1] ?? "")?.gamertag;
        return { jti: i.jti, by: i.by, by_gamertag: tag, at: i.at, exp: i.exp, state, ...(state === "open" ? { invite: this.code(i) } : {}) };
      });
      return ok({ items, position: this.world.log.length });
    }
    if (path === "/v1/invites" && req.method === "POST") {
      const c = this.caller(auth, "invites.write");
      if (!c.ok) return c.reply;
      const ev = parseJws((await req.text()).trim())?.payload as { stream?: string; type?: string; author?: string; at?: string; data?: { jti?: unknown; exp?: unknown } } | undefined;
      if (!ev || ev.stream !== "invites" || ev.type !== "InviteCreated") return fail(400, "invalid", "expected an InviteCreated event on invites");
      if (ev.author !== c.author) return fail(403, "forbidden", "the event's author is not the token's issuer");
      const { jti, exp } = ev.data ?? {};
      const now = Date.now() / 1000;
      if (typeof jti !== "string" || jti.length < 16) return fail(400, "invalid", "jti is 16 to 64 of [A-Za-z0-9_-]");
      if (typeof exp !== "number" || exp <= now || exp > now + MAX_DAYS * 86400 + 300) return fail(400, "invalid", `exp is in the future, at most ${MAX_DAYS} days`);
      if (this.invites.has(jti)) return fail(409, "conflict", "that jti is taken");
      const [w] = this.world.append("invites", [{ type: "InviteCreated", data: { jti, exp } }], null);
      const i: MockInvite = { jti, by: c.author, at: ev.at ?? new Date().toISOString(), exp, position: w?.position ?? this.world.log.length, revoked: false };
      this.invites.set(jti, i);
      return ok({ invite: this.code(i), jti, exp, position: i.position }, 201);
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
