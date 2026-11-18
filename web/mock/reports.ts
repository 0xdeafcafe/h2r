// Made-up player reports for the mock (docs/live/live-admin.md §8), so the
// dashboard's report queue (/dash/server/reports) runs without a server:
// `GET /v1/admin/reports?state=`, dismiss and note, and a player's own
// `POST /v1/accounts/by-xuid/{xuid}/report` with the server's dedupe. The
// reports are invented between the mock's own accounts; admins as in
// mock/admin.ts (H2_MOCK_ADMIN=1).

import { parseJws } from "../src/state/verify";
import type { Admin, Reply } from "./admin";
import type { World } from "./world";

const ok = (body: unknown, status = 200): Reply => ({ status, body });
const fail = (status: number, error: string, message: string): Reply => ({ status, body: { error, message } });

const KINDS = ["gamertag", "gameplay", "screaming", "harassment", "lewdness", "good_attitude", "good_session", "type_10", "type_11", "type_12"];
const negative = (k: string) => k !== "good_attitude" && k !== "good_session";

interface Who { xuid: string; gamertag: string }
interface MockReport {
  id: number;
  target: Who;
  by: Who;
  kind: string;
  reason?: string;
  game_id?: string;
  at: string;
  review?: { action: "dismiss" | "note"; note?: string; by?: string; at: string };
}

export class Reports {
  readonly reports: MockReport[] = [];
  private next = 1;

  constructor(private world: World, private admin: Admin) {
    const people: Who[] = [...world.accounts.values()].map((a) => ({ xuid: a.xuid, gamertag: a.gamertag }));
    const p = (i: number): Who => people[i % Math.max(1, people.length)] ?? { xuid: `00090000000000${String(i).padStart(2, "0")}`, gamertag: `Player ${i}` };
    const at = (minAgo: number) => new Date(Date.now() - minAgo * 60_000).toISOString();
    const add = (r: Omit<MockReport, "id">) => this.reports.push({ id: this.next++, ...r });
    add({ target: p(1), by: p(0), kind: "harassment", at: at(240) });
    add({ target: p(1), by: p(2), kind: "screaming", at: at(180), review: { action: "note", note: "Talked to them in a game.", by: p(3).gamertag, at: at(170) } });
    add({ target: p(4), by: p(0), kind: "gamertag", reason: "Rude name.", at: at(90) });
    add({ target: p(1), by: p(3), kind: "good_session", at: at(60) });
    add({ target: p(2), by: p(4), kind: "gameplay", at: at(20) });
  }

  /** `GET /v1/admin/reports?state=` as an admin reads it. */
  view(state: string) {
    const keep = (r: MockReport) => state === "all" || (negative(r.kind) && (state === "closed" ? !!r.review : !r.review));
    const items = this.reports.filter(keep).sort((a, b) => b.id - a.id)
      .map((r) => ({ ...r, negative: negative(r.kind) }));
    const players: Record<string, { complaints: number; open: number; commendations: number; notes: { report: number; note: string; at: string }[] }> = {};
    for (const i of items) players[i.target.xuid] ??= { complaints: 0, open: 0, commendations: 0, notes: [] };
    for (const r of this.reports) {
      const rec = players[r.target.xuid];
      if (!rec) continue;
      if (!negative(r.kind)) { rec.commendations++; continue; }
      rec.complaints++;
      if (!r.review) rec.open++;
      else if (r.review.action === "note") rec.notes.push({ report: r.id, note: r.review.note ?? "", at: r.review.at });
    }
    return { items, players, position: this.world.log.length };
  }

  async handle(req: Request, u: URL): Promise<Reply | null> {
    const path = u.pathname;
    let m: RegExpMatchArray | null;
    if ((m = path.match(/^\/v1\/accounts\/by-xuid\/([0-9a-f]{16})\/report$/)) && req.method === "POST") {
      const claims = parseJws((req.headers.get("authorization") ?? "").replace(/^Bearer /, "").trim())?.payload as { iss?: string; lxm?: string } | undefined;
      const xuid = /^acct:([0-9a-f]{16})@/.exec(claims?.iss ?? "")?.[1];
      const me = xuid ? this.world.accounts.get(xuid) : undefined;
      if (!me || claims?.lxm !== "reports.write") return fail(401, "unauthorized", "this needs a reports.write token");
      const target = this.world.accounts.get(m[1]!);
      if (!target) return fail(404, "not_found", "no such account");
      if (target.xuid === me.xuid) return fail(400, "invalid", "a player can't report themselves");
      const b = (await req.json().catch(() => null)) as { kind?: unknown; reason?: unknown; game_id?: unknown } | null;
      if (typeof b?.kind !== "string" || !KINDS.includes(b.kind)) return fail(400, "invalid", "unknown kind");
      const open = this.reports.find((r) => r.by.xuid === me.xuid && r.target.xuid === target.xuid && r.kind === b.kind && !r.review);
      if (open) return ok({ ok: true, position: open.id, duplicate: true });
      const r: MockReport = {
        id: this.next++, target: { xuid: target.xuid, gamertag: target.gamertag }, by: { xuid: me.xuid, gamertag: me.gamertag }, kind: b.kind,
        ...(typeof b.reason === "string" && b.reason.trim() ? { reason: b.reason.trim() } : {}),
        ...(typeof b.game_id === "string" ? { game_id: b.game_id } : {}),
        at: new Date().toISOString(),
      };
      this.reports.push(r);
      return ok({ ok: true, position: r.id, duplicate: false }, 201);
    }
    if (!path.startsWith("/v1/admin/reports")) return null;
    const c = this.admin.caller(req.headers.get("authorization"), "reports.moderate");
    if (!c.ok) return c.reply;
    if (path === "/v1/admin/reports" && req.method === "GET") {
      const state = u.searchParams.get("state") ?? "open";
      if (!["open", "closed", "all"].includes(state)) return fail(400, "invalid", "state is open, closed or all");
      return ok(this.view(state));
    }
    if ((m = path.match(/^\/v1\/admin\/reports\/(\d+)\/(dismiss|note)$/)) && req.method === "POST") {
      const r = this.reports.find((x) => x.id === Number(m![1]));
      if (!r) return fail(404, "not_found", "no such report");
      if (!negative(r.kind)) return fail(400, "invalid", "commendations are not reviewed");
      if (r.review) return fail(409, "conflict", "that report was already reviewed");
      const b = (await req.json().catch(() => null)) as { note?: unknown } | null;
      const note = typeof b?.note === "string" ? b.note.trim() : "";
      if (m[2] === "note" && !note) return fail(400, "invalid", "a note needs its text");
      const by = /^acct:([0-9a-f]{16})@/.exec(c.author)?.[1];
      r.review = { action: m[2] as "dismiss" | "note", ...(note ? { note } : {}), by: by ? this.world.accounts.get(by)?.gamertag : undefined, at: new Date().toISOString() };
      return ok({ ok: true, position: this.world.log.length });
    }
    return fail(404, "not_found", `no route ${req.method} ${path}`);
  }
}
