// Signed event writes from the browser (docs/live-security.md §2 and §3),
// the way `h2 live friend ...` and `h2 live team ...` send them: the body
// {stream, type, data, author, kid, at, nonce} as canonical JSON in a
// compact JWS with header {alg: "EdDSA", kid, typ: "h2live-event"}, POSTed
// to /v1/events as application/jose with a fresh `events.write` token.

import { importSeed, signCompact, type LocalAccount } from "./account";
import { canonicalJson } from "./verify";
import { issueToken, randomId } from "../admin/token";
import { ApiError } from "../api/http";
import type { EventWrite } from "../api/social";

export const TYP_EVENT = "h2live-event";

export interface EventBody { stream: string; type: string; data: unknown; author: string; kid: string; at: string; nonce: string }

/** What the account signs for one event, stamped at `nowMs`. */
export function eventBody(me: Pick<LocalAccount, "id" | "kid">, w: EventWrite, nowMs: number, nonce: string = randomId()): EventBody {
  return { stream: w.stream, type: w.type, data: w.data, author: me.id, kid: me.kid, at: new Date(nowMs).toISOString(), nonce };
}

/** The compact JWS of `body`, signed with the account's key. */
export async function signEvent(me: LocalAccount, body: EventBody): Promise<string> {
  const key = await importSeed(me.key, me.x);
  return signCompact(key.privateKey, { alg: "EdDSA", kid: key.publicJwk.kid, typ: TYP_EVENT }, canonicalJson(body));
}

/** A refused write: the ordinary error plus a team write's `team_error`. */
export class WriteError extends ApiError {
  constructor(status: number, code: string, message: string, readonly teamError?: string) { super(status, code, message); }
}

/** Signs and sends one event; resolves to its log position. */
export async function postEvent(
  apiRoot: string, aud: string, me: LocalAccount, w: EventWrite,
  fetchFn: typeof fetch = fetch, nowMs: number = Date.now(),
): Promise<number | null> {
  const jws = await signEvent(me, eventBody(me, w, nowMs));
  const token = await issueToken(me, aud, "events.write", nowMs);
  const r = await fetchFn(apiRoot + "/events", {
    method: "POST",
    headers: { "content-type": "application/jose", authorization: `Bearer ${token}` },
    body: jws,
  });
  if (!r.ok) {
    let code = "http_" + r.status, message = r.statusText, teamError: string | undefined;
    try {
      const b = await r.json();
      if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; }
      if (typeof b?.team_error === "string") teamError = b.team_error;
    } catch { /* not JSON */ }
    throw new WriteError(r.status, code, message, teamError);
  }
  try { const b = await r.json(); return typeof b?.position === "number" ? b.position : null; } catch { return null; }
}
