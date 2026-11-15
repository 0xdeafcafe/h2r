// Event signature checks (docs/live-security.md §2): each wire event carries
// the author's compact JWS (EdDSA / Ed25519) over its canonical body. We
// verify with WebCrypto, off the hot path and in batches, and report a
// status per log position.

import type { AccountKeyEntry, WireEvent } from "../api/types";

export type VerifyStatus = "verified" | "bad" | "unsigned" | "unknown";

export interface VerifyResult {
  position: number;
  status: VerifyStatus;
  reason?: string;
}

export interface Jwk { kty: string; crv: string; x: string; kid?: string }


/** Signature results for a set of log positions (one game's events). */
export interface VerifyCounts { verified: number; bad: number; other: number; pending: number }

export function countResults(positions: readonly number[], results: Readonly<Record<number, VerifyResult | undefined>>): VerifyCounts {
  const c = { verified: 0, bad: 0, other: 0, pending: 0 };
  for (const p of positions) {
    const r = results[p];
    if (!r) c.pending++;
    else if (r.status === "verified") c.verified++;
    else if (r.status === "bad") c.bad++;
    else c.other++;
  }
  return c;
}

/** The summary line for those results. With no events at all it says so,
 *  rather than reporting zero verified, which reads like a failure. */
export function summaryLine(c: VerifyCounts): string {
  const plural = (n: number) => (n === 1 ? "" : "s");
  if (c.bad) return `${c.bad} event${plural(c.bad)} failed signature checks`;
  if (!c.verified && !c.pending && !c.other) return "No events loaded to check yet";
  if (!c.verified && !c.other) return `Checking ${c.pending} signed event${plural(c.pending)}…`;
  let s = `${c.verified} signed event${plural(c.verified)} verified`;
  if (c.pending) s += ` · ${c.pending} checking`;
  if (c.other) s += ` · ${c.other} unchecked`;
  return s;
}

// ---------- encoding helpers ----------

export function b64urlEncode(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function b64urlDecode(s: string): Uint8Array<ArrayBuffer> {
  const t = s.replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(t + "===".slice((t.length + 3) % 4));
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** RFC 8785-style canonical JSON: sorted keys, no whitespace. */
export function canonicalJson(v: unknown): string {
  if (v === null || typeof v !== "object") return JSON.stringify(v ?? null);
  if (Array.isArray(v)) return "[" + v.map((x) => canonicalJson(x === undefined ? null : x)).join(",") + "]";
  const o = v as Record<string, unknown>;
  const keys = Object.keys(o).filter((k) => o[k] !== undefined).sort();
  return "{" + keys.map((k) => JSON.stringify(k) + ":" + canonicalJson(o[k])).join(",") + "}";
}

const enc = new TextEncoder();
const dec = new TextDecoder();

/** RFC 7638 thumbprint of an Ed25519 JWK. */
export async function jwkThumbprint(jwk: Jwk): Promise<string> {
  const json = canonicalJson({ crv: jwk.crv, kty: jwk.kty, x: jwk.x });
  return b64urlEncode(new Uint8Array(await crypto.subtle.digest("SHA-256", enc.encode(json))));
}

export interface ParsedJws {
  header: { alg?: string; kid?: string };
  payload: unknown;
  signingInput: Uint8Array<ArrayBuffer>;
  signature: Uint8Array<ArrayBuffer>;
}

export function parseJws(jws: string): ParsedJws | null {
  const parts = jws.split(".");
  if (parts.length !== 3) return null;
  try {
    return {
      header: JSON.parse(dec.decode(b64urlDecode(parts[0]!))),
      payload: JSON.parse(dec.decode(b64urlDecode(parts[1]!))),
      signingInput: enc.encode(parts[0] + "." + parts[1]),
      signature: b64urlDecode(parts[2]!),
    };
  } catch {
    return null;
  }
}

/** Does the signed body describe this wire event? For a batch JWS (an
 *  array payload) the event's `jws_index` names its element. */
export function bodyMatches(payload: unknown, ev: WireEvent): boolean {
  const one = (b: unknown): boolean => {
    if (!b || typeof b !== "object") return false;
    const body = b as Record<string, unknown>;
    if (body.type !== ev.type) return false;
    if (body.stream !== undefined && body.stream !== ev.stream) return false;
    // The wire `at` is a copy of the signed one (security §2, exact rules).
    if (body.at !== ev.at) return false;
    if (body.author !== undefined && ev.author !== undefined && body.author !== ev.author) return false;
    return canonicalJson(body.data) === canonicalJson(ev.data);
  };
  if (!Array.isArray(payload)) return ev.jws_index === undefined && one(payload);
  if (typeof ev.jws_index !== "number") return false;
  return one(payload[ev.jws_index]);
}

// ---------- keys ----------

export interface KeySource {
  /** Server keys from /.well-known/h2live.json. */
  serverKeys(): Promise<Jwk[]>;
  /** Account keys for an author id (`acct:<xuid>@host`), from
   *  /v1/accounts/by-xuid/{xuid}/keys; null if unknown. */
  accountKeys(author: string): Promise<AccountKeyEntry[] | null>;
}

interface CachedKey { key: CryptoKey; validFrom: number | null; validTo: number | null }

/** The XUID in an `acct:<xuid>@<host>` author id. */
export function authorXuid(author: string): string | null {
  return /^acct:([0-9a-fA-F]{16})@/.exec(author)?.[1]?.toLowerCase() ?? null;
}

export class KeyCache {
  private byKid = new Map<string, Promise<CachedKey | null>>();
  private serverLoaded: Promise<void> | null = null;
  private authors = new Map<string, Promise<void>>();

  constructor(private readonly src: KeySource) {}

  private async importJwk(jwk: Jwk, validFrom: number | null, validTo: number | null): Promise<CachedKey | null> {
    try {
      const key = await crypto.subtle.importKey("jwk", { kty: jwk.kty, crv: jwk.crv, x: jwk.x }, { name: "Ed25519" }, false, ["verify"]);
      return { key, validFrom, validTo };
    } catch {
      return null;
    }
  }

  private async addServerKeys(keys: Jwk[]) {
    for (const k of keys) {
      const kid = k.kid ?? (await jwkThumbprint(k));
      if (!this.byKid.has(kid)) this.byKid.set(kid, this.importJwk(k, null, null));
    }
  }

  private addAccountKeys(keys: AccountKeyEntry[]) {
    for (const k of keys) {
      if (!k?.jwk || !k.kid) continue;
      if (!this.byKid.has(k.kid)) this.byKid.set(k.kid, this.importJwk(k.jwk, k.valid_from ?? null, k.valid_to ?? null));
    }
  }

  /** Finds `kid`, fetching the author's keys once if it is unknown. */
  async get(kid: string, author: string | undefined): Promise<CachedKey | null> {
    if (!this.byKid.has(kid)) {
      if (!this.serverLoaded) this.serverLoaded = this.src.serverKeys().then((k) => this.addServerKeys(k), () => {});
      await this.serverLoaded;
    }
    if (!this.byKid.has(kid) && author) {
      // One fetch per author, shared by every event waiting on it.
      let p = this.authors.get(author);
      if (!p) {
        p = this.src.accountKeys(author).then((keys) => { if (keys) this.addAccountKeys(keys); }, () => {});
        this.authors.set(author, p);
      }
      await p;
    }
    return (await this.byKid.get(kid)) ?? null;
  }

  /** Lets a later fetch retry an author (e.g. after a key rotation). */
  forget(author: string) {
    this.authors.delete(author);
  }
}

/** Account keys are valid from `valid_from` (inclusive) until `valid_to` (exclusive). */
function keyValidAt(k: CachedKey, position: number): boolean {
  if (k.validFrom !== null && position < k.validFrom) return false;
  if (k.validTo !== null && position >= k.validTo) return false;
  return true;
}

/** Verifies one event's signature and that its copies match what was signed. */
export async function verifyEvent(ev: WireEvent, keys: KeyCache): Promise<VerifyResult> {
  const position = ev.position;
  if (!ev.jws) return { position, status: "unsigned" };
  const jws = parseJws(ev.jws);
  if (!jws) return { position, status: "bad", reason: "malformed JWS" };
  if (jws.header.alg !== "EdDSA") return { position, status: "unknown", reason: `alg ${jws.header.alg}` };
  const kid = jws.header.kid ?? ev.kid;
  if (!kid) return { position, status: "unknown", reason: "no kid" };
  if (ev.kid && jws.header.kid && ev.kid !== jws.header.kid) return { position, status: "bad", reason: "kid mismatch" };
  let k: CachedKey | null;
  try {
    k = await keys.get(kid, ev.author);
  } catch {
    return { position, status: "unknown", reason: "key fetch failed" };
  }
  if (!k) return { position, status: "unknown", reason: `unknown key ${kid.slice(0, 8)}` };
  let ok = false;
  try {
    ok = await crypto.subtle.verify({ name: "Ed25519" }, k.key, jws.signature, jws.signingInput);
  } catch {
    return { position, status: "unknown", reason: "Ed25519 not supported here" };
  }
  if (!ok) return { position, status: "bad", reason: "signature does not verify" };
  if (!bodyMatches(jws.payload, ev)) return { position, status: "bad", reason: "event differs from what was signed" };
  if (!keyValidAt(k, position)) return { position, status: "bad", reason: "key was not valid at this position" };
  return { position, status: "verified" };
}

/** hex(sha256(prev || utf8(jws))): `prev` is the raw 32 bytes of the
 *  previous event's hash in the stream, empty for the first. */
export async function chainHash(prev: string | null, jws: string): Promise<string> {
  const p = prev ? hexToBytes(prev) : new Uint8Array(0);
  const j = enc.encode(jws);
  const buf = new Uint8Array(p.length + j.length);
  buf.set(p, 0);
  buf.set(j, p.length);
  return bytesToHex(new Uint8Array(await crypto.subtle.digest("SHA-256", buf)));
}

export function hexToBytes(h: string): Uint8Array {
  const out = new Uint8Array(h.length >> 1);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(h.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function bytesToHex(b: Uint8Array): string {
  let s = "";
  for (const x of b) s += x.toString(16).padStart(2, "0");
  return s;
}

/** Queues events and verifies them in batches, yielding between batches
 *  so rendering never waits on crypto. Also checks the per-stream hash
 *  chain whenever consecutive versions of a stream have been seen. */
export class Verifier {
  private queue: WireEvent[] = [];
  private running = false;
  private chain = new Map<string, { version: number; hash: string }>();

  constructor(
    private readonly keys: KeyCache,
    private readonly onResults: (r: VerifyResult[]) => void,
    private readonly batchSize = 64,
    private readonly yieldFn: () => Promise<void> = () => new Promise((r) => setTimeout(r, 0)),
  ) {}

  enqueue(ev: WireEvent): void {
    this.queue.push(ev);
    if (!this.running) void this.drain();
  }

  get pending(): number {
    return this.queue.length;
  }

  /** Resolves when the queue is empty (for tests). */
  async idle(): Promise<void> {
    // A macrotask wait, so pending crypto can complete between checks.
    while (this.running || this.queue.length) await new Promise((r) => setTimeout(r, 1));
  }

  private async drain() {
    this.running = true;
    try {
      while (this.queue.length) {
        await this.yieldFn();
        const batch = this.queue.splice(0, this.batchSize);
        const results = await Promise.all(batch.map((ev) => verifyEvent(ev, this.keys)));
        // The hash chain is sequential per stream; batches arrive in order.
        for (let i = 0; i < batch.length; i++) {
          const ev = batch[i]!;
          const r = results[i]!;
          if (!ev.jws || !ev.hash) continue;
          const prev = this.chain.get(ev.stream);
          const first = ev.version === 1 || ev.version === 0;
          if ((prev && prev.version + 1 === ev.version) || (!prev && first)) {
            const expect = await chainHash(prev?.hash ?? null, ev.jws);
            if (expect !== ev.hash.toLowerCase() && r.status === "verified") {
              results[i] = { position: ev.position, status: "bad", reason: "hash chain broken" };
            }
          }
          this.chain.set(ev.stream, { version: ev.version, hash: ev.hash.toLowerCase() });
        }
        this.onResults(results);
      }
    } finally {
      this.running = false;
    }
  }
}
