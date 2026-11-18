// Smoke test for a Live server, made for the Worker under `wrangler dev`
// (crates/live-worker/smoke/run.sh). It works against any Live server.
// It uses the web app's own sign-up, token and event code (web/src).
//
//   bun crates/live-worker/smoke/smoke.ts http://127.0.0.1:8787
//
// Steps, each timed:
//   1. the first request, which wakes and starts the hub (cold start);
//   2. GET /v1/snapshot, anonymous;
//   3. sign up two accounts;
//   4. sign in: a token-signed read of /v1/snapshot that shows the new account;
//   5. append an event (a friend request) and read it back from the log,
//      while a /v1/stream socket subscribed before it receives it live;
//   6. the stream: replay from a position, a token-authed socket, a bad
//      token closed with 4401, and a plain GET refused;
//   7. warm timings: /v1/snapshot ten times.
// It prints one JSON object and exits non-zero if a step fails.
//
// --seed N: then grows the log by 2N events (friend requests, declined),
// paced under the write rate, so a restarted hub has a log to replay. --cold-only: steps 1, 2 and 6.
import { createAccount } from "../../../web/src/state/account";
import { postEvent } from "../../../web/src/state/sign";
import { issueToken } from "../../../web/src/admin/token";

const args = process.argv.slice(2);
const base = (args.find((a) => !a.startsWith("--") && !/^\d+$/.test(a)) ?? "http://127.0.0.1:8787").replace(/\/$/, "");
const seedAt = args.indexOf("--seed");
const seed = seedAt >= 0 ? Number(args[seedAt + 1]) : 0;
const coldOnly = args.includes("--cold-only");
const api = base + "/v1";
const out: Record<string, unknown> = { base };
const fail = (step: string, why: unknown): never => {
  out.failed = { step, why: String(why) };
  console.log(JSON.stringify(out, null, 2));
  process.exit(1);
};

async function timed(url: string, init?: RequestInit) {
  const t0 = performance.now();
  const r = await fetch(url, init);
  const body = await r.text();
  return { r, body, ms: +(performance.now() - t0).toFixed(1) };
}

// 1. Cold start: the hub starts on the first request it gets.
const health = await timed(base + "/healthz");
if (!health.r.ok) fail("healthz", `${health.r.status} ${health.body}`);
out.cold = {
  ms: health.ms,
  server_timing: health.r.headers.get("server-timing"),
  head: JSON.parse(health.body).position,
};
const wk = await (await fetch(base + "/.well-known/h2live.json")).json();
const aud: string = wk.id;
out.server = { id: aud, url: wk.url };

// 2. The anonymous page-load read.
const snap = await timed(api + "/snapshot");
if (!snap.r.ok) fail("snapshot", `${snap.r.status} ${snap.body}`);
out.snapshot = { ms: snap.ms, bytes: snap.body.length, cache_control: snap.r.headers.get("cache-control"), etag: !!snap.r.headers.get("etag") };

const warmTimes = async () => {
  const warm: number[] = [];
  for (let i = 0; i < 10; i++) warm.push((await timed(api + "/snapshot")).ms);
  warm.sort((x, y) => x - y);
  out.warm_snapshot_ms = { p50: warm[5], min: warm[0], max: warm[9] };
};
if (coldOnly) {
  await warmTimes();
  out.ok = true;
  console.log(JSON.stringify(out, null, 2));
  process.exit(0);
}

// 3. Sign up.
const tag = () => "Smoke" + Math.floor(Math.random() * 1e9).toString(36).slice(0, 6);
let a, b;
const t3 = performance.now();
try {
  a = await createAccount(api, base, aud, tag());
  b = await createAccount(api, base, aud, tag());
} catch (e) {
  fail("sign-up", e);
}
out.sign_up = { ms: +(performance.now() - t3).toFixed(1), accounts: [a!.gamertag, b!.gamertag] };

// 4. Sign in: a read with the account's own token.
const token = await issueToken(a!, aud, "read");
const mine = await timed(api + "/snapshot?auth=1", { headers: { authorization: `Bearer ${token}` } });
if (!mine.r.ok) fail("sign-in", `${mine.r.status} ${mine.body}`);
const seen = (JSON.parse(mine.body).accounts?.items ?? []).some((x: { xuid: string }) => x.xuid === a!.xuid);
if (!seen) fail("sign-in", "the new account is not in the snapshot");
out.sign_in = { ms: mine.ms, cache_control: mine.r.headers.get("cache-control") };

// A /v1/stream socket: its messages as they come, and how it closed.
type Msg = { kind: string; position?: number; event?: { position: number; type: string } };
function subscribe(query: string) {
  const ws = new WebSocket(`${base.replace(/^http/, "ws")}/v1/stream${query}`);
  const got: Msg[] = [];
  const waiters: (() => void)[] = [];
  let closed: { code: number; reason: string } | null = null;
  const poke = () => waiters.splice(0).forEach((w) => w());
  ws.onmessage = (m) => {
    if (typeof m.data === "string") got.push(JSON.parse(m.data));
    poke();
  };
  ws.onclose = (c) => {
    closed = { code: c.code, reason: c.reason };
    poke();
  };
  const opened = new Promise<void>((ok, no) => {
    ws.onopen = () => ok();
    ws.onerror = (e) => no(e);
  });
  // The first message that `pred` accepts, within `ms`.
  const until = async (pred: (m: Msg) => boolean, ms = 5000) => {
    const end = Date.now() + ms;
    for (;;) {
      const m = got.find(pred);
      if (m) return m;
      if (closed) throw new Error(`closed ${closed.code} ${closed.reason}`);
      const left = end - Date.now();
      if (left <= 0) throw new Error(`timed out; got ${JSON.stringify(got)}`);
      await new Promise<void>((ok) => {
        waiters.push(ok);
        setTimeout(ok, left);
      });
    }
  };
  const close = async (ms = 5000) => {
    if (!closed) {
      const end = Date.now() + ms;
      while (!closed && Date.now() < end)
        await new Promise<void>((ok) => {
          waiters.push(ok);
          setTimeout(ok, end - Date.now());
        });
    }
    return closed;
  };
  return { ws, got, opened, until, close };
}

// The live socket, let in and caught up before the append.
const t5s = performance.now();
const live = subscribe("");
let caughtUp: Msg | undefined;
try {
  await live.opened;
  caughtUp = await live.until((m) => m.kind === "caught_up");
} catch (e) {
  fail("stream-subscribe", e);
}
const subscribeMs = +(performance.now() - t5s).toFixed(1);

// 5. Append an event, and find it in the log.
const t5 = performance.now();
let position: number | null = null;
try {
  position = await postEvent(api, aud, a!, {
    stream: `account-${a!.xuid}`, type: "FriendRequested", data: { from: a!.xuid, to: b!.xuid },
  });
} catch (e) {
  fail("append", e);
}
const log = await (await fetch(`${api}/events?stream=account-${a!.xuid}&limit=100`)).json();
const found = (log.items ?? []).some((e: { type: string; position: number }) => e.type === "FriendRequested" && e.position === position);
if (!found) fail("append", `position ${position} is not in the stream`);
out.append = { ms: +(performance.now() - t5).toFixed(1), position };
try {
  await live.until((m) => m.kind === "event" && m.event?.position === position);
} catch (e) {
  fail("stream-receive", e);
}
out.stream = {
  subscribe_ms: subscribeMs,
  caught_up_at: caughtUp!.position,
  // From the append's start until the socket had the event.
  receive_ms: +(performance.now() - t5).toFixed(1),
};
live.ws.close();

// 6. Replay from just before the event: it, then caught_up.
const t6 = performance.now();
const replay = subscribe(`?from=${position! - 1}&stream=account-${a!.xuid}`);
try {
  await replay.opened;
  const done = await replay.until((m) => m.kind === "caught_up");
  const events = replay.got.filter((m) => m.kind === "event").map((m) => m.event!.position);
  if (events.length !== 1 || events[0] !== position) throw new Error(`replayed ${JSON.stringify(events)}`);
  if (replay.got.indexOf(done) !== 1) throw new Error("caught_up came before the replay");
} catch (e) {
  fail("stream-replay", e);
}
(out.stream as Record<string, unknown>).replay_ms = +(performance.now() - t6).toFixed(1);
replay.ws.close();

// A reader that signs in first (?auth=1), and one with a bad token.
const authed = subscribe("?auth=1");
try {
  await authed.opened;
  // A fresh token: each one is good for a single use.
  authed.ws.send(JSON.stringify({ kind: "auth", token: await issueToken(a!, aud, "read") }));
  await authed.until((m) => m.kind === "caught_up");
} catch (e) {
  fail("stream-auth", e);
}
authed.ws.close();
const bad = subscribe("?auth=1");
try {
  await bad.opened;
  bad.ws.send(JSON.stringify({ kind: "auth", token: "not-a-token" }));
  const c = await bad.close();
  if (c?.code !== 4401) throw new Error(`closed with ${JSON.stringify(c)}`);
} catch (e) {
  fail("stream-bad-token", e);
}
const plain = await fetch(api + "/stream");
if (plain.status !== 400) fail("stream-plain-get", `${plain.status} ${await plain.text()}`);
(out.stream as Record<string, unknown>).auth = "ok";

// 7. Warm.
await warmTimes();

if (seed > 0) {
  const t = performance.now();
  for (let i = 0; i < seed; i++) {
    // Step 5 left a request pending: decline it, then ask again.
    await postEvent(api, aud, b!, { stream: `account-${b!.xuid}`, type: "FriendDeclined", data: { from: a!.xuid, to: b!.xuid } });
    await postEvent(api, aud, a!, { stream: `account-${a!.xuid}`, type: "FriendRequested", data: { from: a!.xuid, to: b!.xuid } });
    // Under the server's write rate (10 a second per author and method).
    await Bun.sleep(110);
  }
  const head = (await (await fetch(base + "/healthz")).json()).position;
  out.seed = { events: 2 * seed, ms: +(performance.now() - t).toFixed(0), per_append_ms: +((performance.now() - t) / (2 * seed)).toFixed(2), head };
}
out.ok = true;
console.log(JSON.stringify(out, null, 2));
