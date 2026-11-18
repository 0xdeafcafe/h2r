// Player reports for admins (docs/live/live-admin.md, "Player reports"): the
// queue's parsing, the review rules and the two calls, with the token's
// scope checked. Every gamertag, id and note is invented.

import { describe, expect, test } from "bun:test";
import { KIND_LABELS, REPORTS_MODERATE, fetchReports, parseReports, recordFor, reviewPath, reviewProblem, reviewReport } from "../src/admin/reports";
import type { LocalAccount } from "../src/state/account";
import { parseJws } from "../src/state/verify";

const RFC = { d: "nWGxne_9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A", x: "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo", kid: "kPrK_qmxVWaYVA9wwBF6Iuo3vVzz7TxHCTwXBygrS4k" };
const me = (): LocalAccount => ({ xuid: "0009000000000042", gamertag: "afr", id: "acct:0009000000000042@localhost", server: "http://127.0.0.1:8074", server_id: "localhost", kid: RFC.kid, key: RFC.d, x: RFC.x });

function recorder(reply: (url: string, init: RequestInit) => Response) {
  const calls: Array<{ url: string; init: RequestInit }> = [];
  const f = (async (url: string, init: RequestInit = {}) => { calls.push({ url, init }); return reply(url, init); }) as unknown as typeof fetch;
  return { f, calls };
}
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const lxm = (init: RequestInit) => (parseJws((init.headers as Record<string, string>).authorization!.slice(7))!.payload as { lxm: string }).lxm;

const T = { xuid: "0009000000000001", gamertag: "Grunt Birthday" };
const R = { xuid: "0009000000000002", gamertag: "Noble Six" };

const QUEUE = {
  items: [
    { id: 7, target: T, by: R, kind: "harassment", negative: true, at: "2026-11-18T09:00:00Z", game_id: "g1" },
    { id: 12, target: T, by: R, kind: "type_10", negative: true, reason: "words", at: "2026-11-18T10:00:00Z" },
    { id: 9, target: T, by: { xuid: "0009000000000003" }, kind: "good_session", negative: false, at: "2026-11-18T09:30:00Z",
      review: { action: "note", note: "kept", by: "Boss", at: "2026-11-18T11:00:00Z" } },
    { id: 13, target: T, by: R, kind: "cheating", negative: true, at: "x" },
    { id: 0, target: T, by: R, kind: "gameplay" },
  ],
  players: { [T.xuid]: { complaints: 3, open: 2, commendations: 1, notes: [{ report: 5, note: "warned", at: "2026-11-17T08:00:00Z" }, { report: 6 }] } },
  position: 40,
};

describe("the report queue", () => {
  test("reads reports newest first, dropping unknown kinds and bad ids", () => {
    const l = parseReports(QUEUE);
    expect(l.items.map((r) => r.id)).toEqual([12, 9, 7]);
    expect(l.items[0]).toMatchObject({ kind: "type_10", reason: "words", game_id: null, review: null, negative: true });
    expect(l.items[2]!.game_id).toBe("g1");
    expect(l.items[1]!.by.gamertag).toBe("unknown");
    expect(l.items[1]!.negative).toBe(false);
    expect(l.items[1]!.review).toEqual({ action: "note", note: "kept", by: "Boss", at: "2026-11-18T11:00:00Z" });
    expect(l.position).toBe(40);
    expect(parseReports(null)).toEqual({ items: [], players: {}, position: null });
  });

  test("the reported player's record, notes without text dropped", () => {
    const l = parseReports(QUEUE);
    expect(recordFor(l, T.xuid)).toEqual({ complaints: 3, open: 2, commendations: 1, notes: [{ report: 5, note: "warned", at: "2026-11-17T08:00:00Z" }] });
    expect(recordFor(l, "0009ffffffffffff")).toEqual({ complaints: 0, open: 0, commendations: 0, notes: [] });
  });

  test("every kind has a label of our own", () => {
    expect(Object.keys(KIND_LABELS)).toHaveLength(10);
    expect(KIND_LABELS.type_12).toBe("Feedback type 12");
  });

  test("a note needs text; neither may be over 500 characters", () => {
    expect(reviewProblem("note", "  ")).toBe("write the note first");
    expect(reviewProblem("note", "warned")).toBeNull();
    expect(reviewProblem("dismiss", "")).toBeNull();
    expect(reviewProblem("dismiss", "x".repeat(501))).toBe("a note is at most 500 characters");
    expect(reviewPath(7, "dismiss")).toBe("/admin/reports/7/dismiss");
  });
});

describe("the calls", () => {
  test("the queue is read with a reports.moderate token, for the state asked", async () => {
    const { f, calls } = recorder(() => json(QUEUE));
    const l = await fetchReports("http://h/v1", "localhost", me(), "closed", f);
    expect(l.items).toHaveLength(3);
    expect(calls[0]!.url).toBe("http://h/v1/admin/reports?state=closed");
    expect(lxm(calls[0]!.init)).toBe(REPORTS_MODERATE);
  });

  test("a review posts the trimmed note; a bad one is never sent", async () => {
    const { f, calls } = recorder(() => json({ ok: true, position: 41 }));
    await reviewReport("http://h/v1", "localhost", me(), 7, "note", "  warned in chat ", f);
    expect(calls[0]!.url).toBe("http://h/v1/admin/reports/7/note");
    expect(calls[0]!.init.method).toBe("POST");
    expect(JSON.parse(calls[0]!.init.body as string)).toEqual({ note: "warned in chat" });
    expect(lxm(calls[0]!.init)).toBe(REPORTS_MODERATE);
    await expect(reviewReport("http://h/v1", "localhost", me(), 7, "note", " ", f)).rejects.toThrow("write the note first");
    expect(calls).toHaveLength(1);
  });

  test("the server's refusal comes back as its error", async () => {
    const { f } = recorder(() => json({ error: "conflict", message: "that report was already reviewed" }, 409));
    await expect(reviewReport("http://h/v1", "localhost", me(), 7, "dismiss", "", f)).rejects.toMatchObject({ status: 409, code: "conflict" });
  });
});
