// Player reports for admins (docs/live/live-admin.md, "Player reports"): the
// queue of complaints players filed about each other from the game's
// player-details screens, each with the reported player's record, and the
// two things an admin does with one: dismiss it, or close it with a note
// kept on the player's record. Every call carries a `reports.moderate`
// token, accepted only for admins. Parsing is defensive like ./content.ts.

import type { LocalAccount } from "../state/account";
import { adminCall } from "./content";

export const REPORTS_MODERATE = "reports.moderate";

export type ReportKind =
  | "gamertag" | "gameplay" | "screaming" | "harassment" | "lewdness"
  | "good_attitude" | "good_session" | "type_10" | "type_11" | "type_12";

export type ReportState = "open" | "closed" | "all";
export type ReviewAction = "dismiss" | "note";

/** How the admin page names each kind. These are our own words, not the
 *  game's; the last three are XDK feedback types whose names are not
 *  confirmed, shown by number. */
export const KIND_LABELS: Record<ReportKind, string> = {
  gamertag: "Offensive gamertag",
  gameplay: "Unsporting play",
  screaming: "Screaming",
  harassment: "Harassment",
  lewdness: "Lewdness",
  good_attitude: "Good attitude",
  good_session: "Good session",
  type_10: "Feedback type 10",
  type_11: "Feedback type 11",
  type_12: "Feedback type 12",
};

const isKind = (v: unknown): v is ReportKind => typeof v === "string" && v in KIND_LABELS;

export interface Who { xuid: string; gamertag: string }

export interface PlayerReport {
  id: number;
  target: Who;
  by: Who;
  kind: ReportKind;
  negative: boolean;
  reason: string;
  game_id: string | null;
  at: string;
  review: { action: ReviewAction; note: string; by: string | null; at: string } | null;
}

export interface PlayerRecord {
  complaints: number;
  open: number;
  commendations: number;
  notes: { report: number; note: string; at: string }[];
}

export interface ReportList {
  /** Newest first. */
  items: PlayerReport[];
  /** By the reported player's XUID. */
  players: Record<string, PlayerRecord>;
  position: number | null;
}

const obj = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const list = (v: unknown): Record<string, unknown>[] => (Array.isArray(v) ? v.map(obj) : []);
const count = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0);
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const who = (v: unknown): Who => {
  const o = obj(v);
  return { xuid: str(o.xuid), gamertag: str(o.gamertag) || "unknown" };
};

/** `GET /v1/admin/reports`, dropping entries it can't read. */
export function parseReports(body: unknown): ReportList {
  const o = obj(body);
  const items: PlayerReport[] = [];
  for (const r of list(o.items)) {
    if (!count(r.id) || !isKind(r.kind)) continue;
    const v = r.review === undefined || r.review === null ? null : obj(r.review);
    items.push({
      id: count(r.id),
      target: who(r.target),
      by: who(r.by),
      kind: r.kind,
      negative: r.negative !== false,
      reason: str(r.reason),
      game_id: str(r.game_id) || null,
      at: str(r.at),
      review: v && (v.action === "dismiss" || v.action === "note")
        ? { action: v.action, note: str(v.note), by: str(v.by) || null, at: str(v.at) }
        : null,
    });
  }
  items.sort((a, b) => b.id - a.id);
  const players: Record<string, PlayerRecord> = {};
  for (const [xuid, raw] of Object.entries(obj(o.players))) {
    const p = obj(raw);
    players[xuid] = {
      complaints: count(p.complaints),
      open: count(p.open),
      commendations: count(p.commendations),
      notes: list(p.notes).filter((n) => str(n.note)).map((n) => ({ report: count(n.report), note: str(n.note), at: str(n.at) })),
    };
  }
  return { items, players, position: typeof o.position === "number" ? o.position : null };
}

/** The reported player's record, empty when the server sent none. */
export function recordFor(l: ReportList, xuid: string): PlayerRecord {
  return l.players[xuid] ?? { complaints: 0, open: 0, commendations: 0, notes: [] };
}

/** What a review needs before it can be sent: a note needs its text;
 *  a dismissal may have one. Null when it is fine. */
export function reviewProblem(action: ReviewAction, note: string): string | null {
  const t = note.trim();
  if (action === "note" && !t) return "write the note first";
  if ([...t].length > 500) return "a note is at most 500 characters";
  return null;
}

export function reviewPath(id: number, action: ReviewAction): string {
  return `/admin/reports/${id}/${action}`;
}

export async function fetchReports(api: string, aud: string, me: LocalAccount, state: ReportState = "open", fetchFn: typeof fetch = fetch): Promise<ReportList> {
  return parseReports(await adminCall(api, `/admin/reports?state=${state}`, aud, me, REPORTS_MODERATE, {}, fetchFn));
}

export async function reviewReport(api: string, aud: string, me: LocalAccount, id: number, action: ReviewAction, note: string, fetchFn: typeof fetch = fetch): Promise<void> {
  const problem = reviewProblem(action, note);
  if (problem) throw new Error(problem);
  await adminCall(api, reviewPath(id, action), aud, me, REPORTS_MODERATE, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ note: note.trim() }),
  }, fetchFn);
}
