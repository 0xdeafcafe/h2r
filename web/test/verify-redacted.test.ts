// Events the server redacted for this reader (addresses or presence details
// hidden, signature dropped) get their own status: they are not tampering,
// and they are not "unsigned" either.
import { describe, expect, test } from "bun:test";
import { World } from "../mock/world";
import { KeyCache, countResults, isRedacted, summaryLine, verifyEvent, verifyMarkText, type VerifyResult } from "../src/state/verify";
import type { WireEvent } from "../src/api/types";

function world() {
  const w = new World();
  w.seed(60_000);
  return w;
}

function keys(w: World) {
  return new KeyCache({
    serverKeys: async () => [w.serverKey.jwk],
    accountKeys: async (author) => {
      const xuid = /^acct:([0-9a-f]{16})@/.exec(author)?.[1];
      const a = xuid ? w.accounts.get(xuid) : undefined;
      return a ? w.accountKeys(a).keys : null;
    },
  });
}

/** What the server sends a public reader for a SessionCreated: addresses
 *  gone, `redacted: true`, no signature. */
function redactedSession(): WireEvent {
  return {
    position: 7,
    stream: "session-1",
    version: 1,
    type: "SessionCreated",
    at: "2026-11-18T00:00:00Z",
    data: { session_id: "1", name: "Lockout", redacted: true },
    author: "acct:0000000000000001@live",
    kid: "k",
    jws: "",
    hash: "00",
  };
}

describe("redacted events", () => {
  test("a redacted event without a signature reads as redacted, not unsigned", async () => {
    const w = world();
    const ev = redactedSession();
    expect(isRedacted(ev)).toBe(true);
    expect((await verifyEvent(ev, keys(w))).status).toBe("redacted");
  });

  test("an unsigned event that is not redacted stays unsigned", async () => {
    const w = world();
    const ev = { ...redactedSession(), data: { session_id: "1", name: "Lockout" } };
    expect(isRedacted(ev)).toBe(false);
    expect((await verifyEvent(ev, keys(w))).status).toBe("unsigned");
  });

  test("a signed event that claims to be redacted is still checked, and fails", async () => {
    const w = world();
    const e = w.log.find((x) => x.type === "SignedIn")!;
    const forged: WireEvent = { ...e, data: { ...(e.data as object), redacted: true } };
    expect((await verifyEvent(e, keys(w))).status).toBe("verified");
    const r = await verifyEvent(forged, keys(w));
    expect(r.status).toBe("bad");
  });

  test("the mark says the addresses are hidden and only the server checked it", () => {
    const m = verifyMarkText({ position: 7, status: "redacted" });
    expect(m.text).toBe("redacted");
    expect(m.label).toBe("addresses hidden, signature checked by the server only");
    expect(m.icon).not.toBe(verifyMarkText({ position: 7, status: "bad" }).icon);
    expect(m.text).not.toContain("tamper");
  });

  test("counts and the summary line keep redacted apart from failed and unchecked", () => {
    const r: Record<number, VerifyResult> = {
      1: { position: 1, status: "verified" },
      2: { position: 2, status: "redacted" },
      3: { position: 3, status: "redacted" },
      4: { position: 4, status: "unknown" },
    };
    const c = countResults([1, 2, 3, 4], r);
    expect(c).toEqual({ verified: 1, bad: 0, redacted: 2, other: 1, pending: 0 });
    expect(summaryLine(c)).toBe("1 signed event verified · 2 redacted · 1 unchecked");
    expect(summaryLine(countResults([2], r))).toBe("0 signed events verified · 1 redacted");
  });
});
