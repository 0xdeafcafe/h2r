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
//   5. append an event (a friend request) and read it back from the log;
//   6. warm timings: /v1/snapshot ten times.
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

// 6. Warm.
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
