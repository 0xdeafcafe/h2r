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
//   7. warm timings: /v1/snapshot ten times;
//   8. with --wake S: a hibernation wake. A socket is opened, then
//      nothing is sent for S seconds. The next request must find a new
//      hub (it reports a cold start), and the socket, still open, must get
//      the next event, from the session the new hub rebuilt from its
//      attachment;
//   9. game ingest: start a game, send a frame on /v1/ingest, and see it
//      reach a JSON reader and, through the packed tick, a packed reader;
//      a token for the wrong method is closed with 4401;
//  10. R2: a 9 MiB content upload (more than one 8 MiB part, so a
//      multipart upload streamed from the request body) and a cloud save,
//      each read back; with --admin NAME (a configured admin's gamertag,
//      free on this server), a 2 MiB title file, more than a Durable
//      Object's SQLite row holds, published and read back, then a private
//      Clanker Training run, its live stream and its R2 report; with --big,
//      an upload of exactly the content cap (95 MiB, the largest whole MiB
//      under the edge's 100 MB body limit; sent to the hub once, not teed
//      for a retry) read back, and one byte more refused by the hub's own
//      413, not the edge's;
//  11. with --busy N: a busy stream. N sockets follow the log while 60
//      events are appended; how long each append takes, and how long until
//      every socket has the event;
//  12. the co-op relay (its own Durable Object per session): two members of
//      a co-op session open it and hear of each other, a stranger is
//      refused; the round trip of a datagram, and datagrams a second at
//      co-op's rate (30 a second each way, for longer than a ticket lasts,
//      so it is renewed) and near the relay's limit.
// It prints one JSON object and exits non-zero if a step fails.
//
// --seed N: then grows the log by 2N events (friend requests, declined),
// paced under the write rate, so a restarted hub has a log to replay. --cold-only: steps 1, 2 and 6.
import { decodeArchive } from "../../../web/src/api/archive";
import { decodeChunk } from "../../../web/src/api/chunk";
import { canonicalJson } from "../../../web/src/state/verify";
import { createAccount, importSeed, signCompact, type LocalAccount } from "../../../web/src/state/account";
import { eventBody, postEvent, TYP_EVENT } from "../../../web/src/state/sign";
import { issueToken, randomId } from "../../../web/src/admin/token";

const args = process.argv.slice(2);
const base = (args.find((a) => /^https?:/.test(a)) ?? "http://127.0.0.1:8787").replace(/\/$/, "");
const seedAt = args.indexOf("--seed");
const seed = seedAt >= 0 ? Number(args[seedAt + 1]) : 0;
const coldOnly = args.includes("--cold-only");
const adminAt = args.indexOf("--admin");
const adminTag = adminAt >= 0 ? args[adminAt + 1] : "";
const busyAt = args.indexOf("--busy");
const busy = busyAt >= 0 ? Number(args[busyAt + 1]) : 0;
const big = args.includes("--big");
const wakeAt = args.indexOf("--wake");
const wakeWait = wakeAt >= 0 ? Number(args[wakeAt + 1]) : 0;
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
const t3 = performance.now();
const a = await createAccount(api, base, aud, tag()).catch((e) => fail("sign-up", e));
const b = await createAccount(api, base, aud, tag()).catch((e) => fail("sign-up", e));
out.sign_up = { ms: +(performance.now() - t3).toFixed(1), accounts: [a!.gamertag, b!.gamertag] };

// 4. Sign in: a read with the account's own token.
const token = await issueToken(a!, aud, "read");
const mine = await timed(api + "/snapshot?auth=1", { headers: { authorization: `Bearer ${token}` } });
if (!mine.r.ok) fail("sign-in", `${mine.r.status} ${mine.body}`);
const seen = (JSON.parse(mine.body).accounts?.items ?? []).some((x: { xuid: string }) => x.xuid === a!.xuid);
if (!seen) fail("sign-in", "the new account is not in the snapshot");
out.sign_in = { ms: mine.ms, cache_control: mine.r.headers.get("cache-control") };

// A /v1/stream socket: its messages as they come, and how it closed.
type Msg = {
  kind: string;
  position?: number;
  event?: { position: number; type: string };
  frame?: { game_id: string; tick: number };
};
function subscribe(query: string, path = "/v1/stream") {
  const ws = new WebSocket(`${base.replace(/^http/, "ws")}${path}${query}`);
  const got: Msg[] = [];
  let binary = 0;
  const waiters: (() => void)[] = [];
  let closed: { code: number; reason: string } | null = null;
  const poke = () => waiters.splice(0).forEach((w) => w());
  ws.onmessage = (m) => {
    if (typeof m.data === "string") got.push(JSON.parse(m.data));
    else binary++;
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
  // The first binary message (packed frames), within `ms`.
  const untilBinary = async (ms = 5000) => {
    const end = Date.now() + ms;
    while (binary === 0) {
      if (closed) throw new Error(`closed ${closed.code} ${closed.reason}`);
      const left = end - Date.now();
      if (left <= 0) throw new Error("no binary message in time");
      await new Promise<void>((ok) => {
        waiters.push(ok);
        setTimeout(ok, left);
      });
    }
  };
  return { ws, got, opened, until, close, untilBinary };
}

// Each authenticated socket needs its own single-use read token.
async function signIn(socket: ReturnType<typeof subscribe>, who: LocalAccount) {
  await socket.opened;
  socket.ws.send(JSON.stringify({ kind: "auth", token: await issueToken(who, aud, "read") }));
}

// Friend events are visible to the participants and their friends.
const t5s = performance.now();
const live = subscribe("?auth=1");
let caughtUp: Msg | undefined;
try {
  await signIn(live, a!);
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
const eventsUrl = `${api}/events?stream=account-${a!.xuid}&from=${position! - 1}&limit=100`;
const publicLog = await timed(eventsUrl);
if (!publicLog.r.ok || JSON.parse(publicLog.body).items.some((e: {position: number}) => e.position === position))
  fail("anonymous-private-event", publicLog.r.status);
const participantLog = await timed(eventsUrl + "&auth=1", {
  headers: {authorization: `Bearer ${await issueToken(a!, aud, "read")}`},
});
if (!participantLog.r.ok) fail("append-read", `${participantLog.r.status} ${participantLog.body}`);
const log = JSON.parse(participantLog.body);
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
const replay = subscribe(`?auth=1&from=${position! - 1}&stream=account-${a!.xuid}`);
try {
  await signIn(replay, a!);
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
  await signIn(authed, a!);
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

// 8. A hibernation wake.
if (wakeWait > 0) {
  const sleeper = subscribe("?auth=1");
  try {
    await signIn(sleeper, a!);
    await sleeper.until((m) => m.kind === "caught_up");
  } catch (e) {
    fail("wake-subscribe", e);
  }
  await Bun.sleep(wakeWait * 1000);
  const t8 = performance.now();
  const after = await timed(base + "/healthz");
  const timing = after.r.headers.get("server-timing");
  if (!timing?.startsWith("start;")) fail("wake", `the hub did not restart in ${wakeWait} s (server-timing ${timing})`);
  let woke: number | null = null;
  try {
    woke = await postEvent(api, aud, b!, {
      stream: `account-${b!.xuid}`, type: "FriendDeclined", data: { from: a!.xuid, to: b!.xuid },
    });
    await sleeper.until((m) => m.kind === "event" && m.event?.position === woke);
  } catch (e) {
    fail("wake-receive", e);
  }
  out.wake = {
    idle_s: wakeWait,
    restart: timing,
    // From the first request after the idle until the open socket had the event.
    receive_ms: +(performance.now() - t8).toFixed(1),
  };
  sleeper.ws.close();
  // Step 8 declined the request: ask again, as the seed loop expects.
  await postEvent(api, aud, a!, { stream: `account-${a!.xuid}`, type: "FriendRequested", data: { from: a!.xuid, to: b!.xuid } });
}

let archiveGame = "";
// 9. Game ingest.
{
  const t9 = performance.now();
  const writeToken = () => issueToken(a!, aud, "games.write");
  const started = await timed(api + "/games", {
    method: "POST",
    headers: { authorization: `Bearer ${await writeToken()}`, "content-type": "application/json" },
    body: JSON.stringify({
      console: "Smoke", map: "lockout", mode: "multiplayer", variant: "Team Slayer",
      team_play: true, score_limit: 50, tick: 0,
      players: [
        { index: 0, name: "Host", xuid: a!.xuid, team: 0, local: true },
        { index: 1, name: "Guest", team: 1 },
      ],
    }),
  });
  if (started.r.status !== 201) fail("ingest-start-game", `${started.r.status} ${started.body}`);
  const gameId: string = JSON.parse(started.body).game_id;
  const json = subscribe(`?stream=game-${gameId}&games=${gameId}&exclude=GameFrame`);
  const packed = subscribe(`?games=${gameId}&frames=packed`);
  const game = subscribe("", "/v1/ingest");
  try {
    await Promise.all([json.opened, packed.opened, game.opened]);
    await json.until((m) => m.kind === "caught_up");
    await packed.until((m) => m.kind === "caught_up");
    game.ws.send(JSON.stringify({ kind: "auth", token: await writeToken() }));
    const frame = (tick: number) => JSON.stringify({
      kind: "frame",
      frame: { game_id: gameId, tick, players: [{ index: 0, alive: true, pos: [1, 2, 0.5], yaw: 0.25 }] },
    });
    game.ws.send(frame(30));
    await json.until((m) => m.kind === "frame" && m.frame?.tick === 30);
    await packed.untilBinary();
    // Two disjoint source chunks force two alarm steps and a durable cursor.
    await Bun.sleep(80);
    game.ws.send(frame(960));
    await json.until((m) => m.kind === "frame" && m.frame?.tick === 960);
    // Every reply on the ingest socket is a refusal.
    const refused = game.got.filter((m) => m.kind === "error");
    if (refused.length) throw new Error(`refused: ${JSON.stringify(refused)}`);
  } catch (e) {
    fail("ingest", e);
  }
  const wrong = subscribe("", "/v1/ingest");
  try {
    await wrong.opened;
    wrong.ws.send(JSON.stringify({ kind: "auth", token: await issueToken(a!, aud, "read") }));
    const c = await wrong.close();
    if (c?.code !== 4401) throw new Error(`closed with ${JSON.stringify(c)}`);
  } catch (e) {
    fail("ingest-wrong-method", e);
  }
  out.ingest = { game: gameId, start_subscribe_and_frame_ms: +(performance.now() - t9).toFixed(1) };
  try {
    const end = eventBody(a!, {stream:"game-" + gameId, type:"GameEnded", data:{tick:960,reason:"finished"}}, Date.now());
    const key = await importSeed(a!.key, a!.x);
    const compact = await signCompact(key.privateKey, {alg:"EdDSA", kid:key.publicJwk.kid, typ:TYP_EVENT}, canonicalJson([end]));
    const closeAt = performance.now();
    const closed = await timed(api + "/games/" + gameId + "/events", {
      method:"POST", headers:{authorization:`Bearer ${await writeToken()}`, "content-type":"application/jose"}, body:compact,
    });
    if (closed.r.status !== 201) fail("archive-close", closed.body);
    await json.until(m => m.kind === "event" && m.event?.type === "ReplayArchivePublished", 30_000);
    const publishedAt = performance.now();
    const url = api + "/games/" + gameId;
    const index = await (await fetch(url + "/chunks?format=archive")).json();
    if (index.items?.length !== 2 || !/^[0-9a-f]{64}$/.test(index.archive_sha256)) fail("archive-index", JSON.stringify(index));
    const readMs: number[] = [];
    for (const [i, tick] of [30, 960].entries()) {
      const raw = await fetch(url + "/chunks/" + tick);
      const readAt = performance.now();
      const segment = await fetch(url + "/archive/" + index.archive_sha256 + "/" + i);
      if (!raw.ok || !segment.ok || segment.headers.get("content-encoding") !== "br" ||
          !segment.headers.get("cache-control")?.includes("immutable")) fail("archive-headers", segment.status);
      const archived = decodeArchive(new Uint8Array(await segment.arrayBuffer()));
      readMs.push(+(performance.now() - readAt).toFixed(1));
      const source = await decodeChunk(new Uint8Array(await raw.arrayBuffer()));
      if (archived.length !== 1 || archived[0]?.tick !== tick || canonicalJson(archived) !== canonicalJson(source))
        fail("archive-r2-parity", "source and archive differ at tick " + tick);
    }
    archiveGame = gameId;
    out.archive = {segments:index.items.length, archive_sha256:index.archive_sha256,
      close_ms:closed.ms, close_to_publication_ms:+(publishedAt - closeAt).toFixed(1),
      segment_fetch_and_decode_ms:readMs};
  } catch (e) { fail("archive-alarm", e); }
  for (const s of [json, packed, game]) s.ws.close();
}

async function simRun(admin: LocalAccount, other: LocalAccount) {
  const runs = api + "/admin/sim-runs";
  const t = performance.now();
  const batch = randomId();
  const bots = [{ name: "Bolt", arm: "smoke", tuning: {} }];
  const metadata = {
    run_id: randomId(), batch_id: batch, seed: "42", gametype: "slayer",
    pace: "unpaced", bots,
  };
  const id = "sim-" + metadata.run_id;
  const headers = async (who: LocalAccount, method: string) => ({
    authorization: `Bearer ${await issueToken(who, aud, method)}`,
    "content-type": "application/json",
  });
  const read = async (path: string) => timed(path, { headers: await headers(admin, "read") });
  const start = async (who: LocalAccount) => timed(runs, {
    method: "POST", headers: await headers(who, "games.write"),
    body: JSON.stringify({
      map: "lockout", mode: "multiplayer", variant: "Slayer", tick: 0,
      team_play: false, sim: metadata,
      players: [{ index: 0, name: "Bolt", team: 0, bot: true }],
    }),
  });
  if ((await start(other)).r.status !== 403) throw new Error("non-admin sim start accepted");
  const started = await start(admin);
  if (started.r.status !== 201 || JSON.parse(started.body).game_id !== id) throw new Error(started.body);

  const filtered = () => read(`${runs}?batch=${batch}&tuning=smoke&map=lockout`);
  const running = await filtered();
  if (JSON.parse(running.body).items?.[0]?.status !== "running") throw new Error(running.body);
  const reader = subscribe(`?auth=1&games=${id}&stream=game-${id}&frames=packed`);
  const ingest = subscribe("", "/v1/ingest");
  try {
    await Promise.all([reader.opened, ingest.opened]);
    reader.ws.send(JSON.stringify({ kind: "auth", token: await issueToken(admin, aud, "read") }));
    await reader.until((m) => m.kind === "caught_up");
    ingest.ws.send(JSON.stringify({ kind: "auth", token: await issueToken(admin, aud, "games.write") }));
    ingest.ws.send(JSON.stringify({
      kind: "frame", frame: { game_id: id, tick: 2, players: [{ index: 0, alive: true, pos: [1, 2, 3], yaw: 0 }] },
    }));
    await reader.untilBinary();
    const end = eventBody(admin, { stream: `game-${id}`, type: "GameEnded", data: { reason: "finished", tick: 2, players: [] } }, Date.now());
    const key = await importSeed(admin.key, admin.x);
    const jws = await signCompact(key.privateKey, { alg: "EdDSA", kid: admin.kid, typ: TYP_EVENT }, canonicalJson([end]));
    ingest.ws.send(JSON.stringify({ kind: "events", game_id: id, jws }));
    await ingest.until((m) => m.kind === "appended");
  } finally {
    reader.ws.close();
    ingest.ws.close();
  }

  const report = {
    format: "h2-sim-report/1",
    match: { map: "lockout", game_type: "slayer", seed: 42, bot: bots },
    result: { end: "tick_limit" }, ticks: 2, wall: { speedup: 2.8 }, players: [],
  };
  const uploaded = await timed(`${runs}/${id}/report`, {
    method: "PUT", headers: await headers(admin, "games.write"), body: JSON.stringify(report),
  });
  if (uploaded.r.status !== 204) throw new Error(uploaded.body);
  const detail = await read(`${runs}/${id}`);
  const d = JSON.parse(detail.body);
  if (d.run?.status !== "finished" || canonicalJson(d.report) !== canonicalJson(report)) throw new Error(detail.body);
  const chunk = await read(`${api}/games/${id}/chunks`);
  const tick = JSON.parse(chunk.body).items?.[0]?.from_tick;
  if (tick !== 2) throw new Error(chunk.body);
  const bytes = await fetch(`${api}/games/${id}/chunks/${tick}`, { headers: await headers(admin, "read") });
  if (!bytes.ok || !(await bytes.arrayBuffer()).byteLength) throw new Error("sim replay bytes missing");
  if ((await fetch(`${runs}/${id}`)).status !== 401) throw new Error("anonymous report read accepted");
  if ((await fetch(`${api}/games/${id}/chunks`)).status !== 404) throw new Error("anonymous replay read accepted");
  out.sim = { id, round_trip_ms: +(performance.now() - t).toFixed(1) };
}

// 10. R2.
{
  const hex = async (b: Uint8Array<ArrayBuffer>) =>
    [...new Uint8Array(await crypto.subtle.digest("SHA-256", b))].map((x) => x.toString(16).padStart(2, "0")).join("");
  const random = (n: number) => {
    const b = new Uint8Array(n);
    for (let i = 0; i < n; i += 65536) crypto.getRandomValues(b.subarray(i, Math.min(n, i + 65536)));
    return b;
  };
  const same = (x: Uint8Array, y: Uint8Array) => x.length === y.length && x.every((v, i) => v === y[i]);
  const r2: Record<string, unknown> = {};

  // Content: 9 MiB, streamed into a multipart upload.
  const content = random(9 * 1024 * 1024);
  const sha = await hex(content);
  let t = performance.now();
  const up = await timed(`${api}/content/${sha}`, {
    method: "PUT",
    headers: { authorization: `Bearer ${await issueToken(a!, aud, "content.write")}` },
    body: content,
  });
  if (up.r.status !== 201) fail("r2-content-put", `${up.r.status} ${up.body}`);
  r2.content_put_ms = +(performance.now() - t).toFixed(1);
  t = performance.now();
  const back = new Uint8Array(await (await fetch(`${api}/content/${sha}`)).arrayBuffer());
  if (!same(back, content)) fail("r2-content-get", `${back.length} bytes back, not the ${content.length} sent`);
  r2.content_get_ms = +(performance.now() - t).toFixed(1);
  // A body that does not hash to its path is refused, and not stored.
  const wrong = await fetch(`${api}/content/${"0".repeat(64)}`, {
    method: "PUT",
    headers: { authorization: `Bearer ${await issueToken(a!, aud, "content.write")}` },
    body: random(1024),
  });
  if (wrong.status !== 400) fail("r2-content-mismatch", `${wrong.status} ${await wrong.text()}`);
  if ((await fetch(`${api}/content/${"0".repeat(64)}`)).status !== 404) fail("r2-content-mismatch", "it was stored");

  // The cap (live_server::content::DEFAULT_UPLOAD_BYTES), and past it.
  if (big) {
    const cap = 95 * 1024 * 1024;
    const blob = random(cap);
    const bigSha = await hex(blob);
    t = performance.now();
    const put = await timed(`${api}/content/${bigSha}`, {
      method: "PUT",
      headers: { authorization: `Bearer ${await issueToken(a!, aud, "content.write")}` },
      body: blob,
    });
    if (put.r.status !== 201 && put.r.status !== 200) fail("r2-content-big-put", `${put.r.status} ${put.body}`);
    r2.content_big_put_ms = +(performance.now() - t).toFixed(1);
    const read = new Uint8Array(await (await fetch(`${api}/content/${bigSha}`)).arrayBuffer());
    if (!same(read, blob)) fail("r2-content-big-get", `${read.length} bytes back, not the ${blob.length} sent`);
    const over = new Uint8Array(cap + 1);
    over.set(blob);
    const refused = await fetch(`${api}/content/${await hex(over)}`, {
      method: "PUT",
      headers: { authorization: `Bearer ${await issueToken(a!, aud, "content.write")}` },
      body: over,
    });
    const why = await refused.text();
    if (refused.status !== 413 || !why.includes("too_large")) fail("r2-content-over-cap", `${refused.status} ${why}`);
  }

  // A cloud save.
  const save = random(300 * 1024);
  const item = `${api}/accounts/${a!.xuid}/storage/save/smoke-slot`;
  t = performance.now();
  const put = await timed(item, {
    method: "PUT",
    headers: {
      authorization: `Bearer ${await issueToken(a!, aud, "storage.write")}`,
      "if-match": "0",
      "x-h2-sha256": await hex(save),
    },
    body: save,
  });
  if (put.r.status !== 201) fail("r2-save-put", `${put.r.status} ${put.body}`);
  const got = new Uint8Array(
    await (await fetch(item, { headers: { authorization: `Bearer ${await issueToken(a!, aud, "storage.read")}` } })).arrayBuffer(),
  );
  if (!same(got, save)) fail("r2-save-get", `${got.length} bytes back`);
  r2.save_round_trip_ms = +(performance.now() - t).toFixed(1);

  // A title file, as an admin.
  if (adminTag) {
    const admin = await createAccount(api, base, aud, adminTag);
    const uri = api + "/admin/replay-archives/" + archiveGame;
    const info = await (await fetch(uri, {headers:{authorization:`Bearer ${await issueToken(admin, aud, "admin.read")}`}})).json();
    const verify = async (cursor: string | null) => {
      const r = await timed(uri + "/step", {method:"POST",
        headers:{authorization:`Bearer ${await issueToken(admin, aud, "admin.write")}`, "content-type":"application/json"},
        body:JSON.stringify({mode:"verify",source_position:info.source_position,cursor})});
      if (!r.r.ok) fail("archive-verify", r.body);
      return JSON.parse(r.body);
    };
    const verifyAt = performance.now();
    const first = await verify(null);
    const last = await verify(first.next);
    if (first.done || !first.next || !last.done || last.totals.frames !== 2 || last.totals.segments !== 2 ||
        last.archive_sha256 !== info.archive_sha256 || last.backup_sha256 !== info.backup_sha256 ||
        canonicalJson(await verify(first.next)) !== canonicalJson(last)) fail("archive-retry", JSON.stringify(last));
    r2.verified_archive = { ...last.totals, verify_with_retry_ms:+(performance.now() - verifyAt).toFixed(1) };
    const file = random(2 * 1024 * 1024);
    const url = `${api}/titles/4d530064/files/smoke.bin`;
    t = performance.now();
    const pub = await timed(url, {
      method: "PUT",
      headers: { authorization: `Bearer ${await issueToken(admin, aud, "titles.write")}` },
      body: file,
    });
    if (pub.r.status !== 200) fail("r2-title-put", `${pub.r.status} ${pub.body}`);
    const read = new Uint8Array(await (await fetch(url)).arrayBuffer());
    if (!same(read, file)) fail("r2-title-get", `${read.length} bytes back`);
    r2.title_file_round_trip_ms = +(performance.now() - t).toFixed(1);
    try {
      await simRun(admin, a!);
    } catch (e) {
      fail("sim-run", e);
    }
  }
  out.r2 = r2;
}

// 11. A busy stream.
if (busy > 0) {
  const readers = Array.from({ length: busy }, () => subscribe("?auth=1"));
  try {
    await Promise.all(readers.map((r) => signIn(r, a!)));
    await Promise.all(readers.map((r) => r.until((m) => m.kind === "caught_up", 15000)));
  } catch (e) {
    fail("busy-subscribe", e);
  }
  const appendMs: number[] = [];
  const fanoutMs: number[] = [];
  const t11 = performance.now();
  for (let i = 0; i < 30; i++) {
    for (const [who, type, stream] of [
      [b!, "FriendDeclined", `account-${b!.xuid}`],
      [a!, "FriendRequested", `account-${a!.xuid}`],
    ] as const) {
      const t = performance.now();
      const position = await postEvent(api, aud, who, { stream, type, data: { from: a!.xuid, to: b!.xuid } });
      appendMs.push(performance.now() - t);
      try {
        await Promise.all(readers.map((r) => r.until((m) => m.kind === "event" && m.event?.position === position)));
      } catch (e) {
        fail("busy-fanout", e);
      }
      fanoutMs.push(performance.now() - t);
    }
    // Under the server's write rate (10 a second per author and method).
    await Bun.sleep(110);
  }
  const p = (xs: number[], q: number) => +[...xs].sort((x, y) => x - y)[Math.floor(q * (xs.length - 1))].toFixed(1);
  out.busy = {
    sockets: busy,
    events: appendMs.length,
    append_ms: { p50: p(appendMs, 0.5), p90: p(appendMs, 0.9), max: p(appendMs, 1) },
    // From sending the append until every socket had the event.
    all_sockets_ms: { p50: p(fanoutMs, 0.5), p90: p(fanoutMs, 0.9), max: p(fanoutMs, 1) },
    wall_ms: +(performance.now() - t11).toFixed(0),
  };
  for (const r of readers) r.ws.close();
}

// 12. The co-op relay.
{
  const t12 = performance.now();
  const pause = () => Bun.sleep(110);
  for (const u of [a!, b!]) {
    await postEvent(api, aud, u, { stream: `presence-${u.xuid}`, type: "SignedIn", data: { xuid: u.xuid, state: "online" } });
  }
  const made = await timed(api + "/sessions", {
    method: "POST",
    headers: { authorization: `Bearer ${await issueToken(a!, aud, "sessions.write")}`, "content-type": "application/json" },
    body: JSON.stringify({
      addr: "127.0.0.1", port: 31000, host_address: Array(36).fill(0),
      slots: { public_filled: 1, public_open: 1, private_filled: 0, private_open: 0 },
      attributes: {}, flags: "coop",
    }),
  });
  if (made.r.status !== 201) fail("relay-session", `${made.r.status} ${made.body}`);
  const sessionId: string = JSON.parse(made.body).session_id;
  await pause();
  const invite = await postEvent(api, aud, a!, {
    stream: `mailbox-${b!.xuid}`, type: "GameInviteSent", data: { from: a!.xuid, session_id: sessionId },
  });
  await pause();
  await postEvent(api, aud, b!, {
    stream: `mailbox-${b!.xuid}`, type: "GameInviteAnswered",
    data: { by: b!.xuid, session_id: sessionId, invite_id: invite, answer: "accept" },
  });

  type Relay = { ws: WebSocket; text: any[]; got: number; closed: { code: number; reason: string } | null; onBinary?: (d: ArrayBuffer) => void };
  const relay = async (who: typeof a) => {
    const ws = new WebSocket(`${base.replace(/^http/, "ws")}/v1/sessions/${sessionId}/relay`);
    ws.binaryType = "arraybuffer";
    const r: Relay = { ws, text: [], got: 0, closed: null };
    ws.onmessage = (m) => {
      if (typeof m.data === "string") r.text.push(JSON.parse(m.data));
      else {
        r.got++;
        r.onBinary?.(m.data as ArrayBuffer);
      }
    };
    ws.onclose = (c) => (r.closed = { code: c.code, reason: c.reason });
    await new Promise<void>((ok, no) => {
      ws.onopen = () => ok();
      ws.onerror = (e) => no(e);
    });
    ws.send(JSON.stringify({ kind: "auth", token: await issueToken(who!, aud, "sessions.relay") }));
    return r;
  };
  const waitFor = async (what: () => boolean, ms: number, why: string) => {
    const end = Date.now() + ms;
    while (!what()) {
      if (Date.now() > end) throw new Error(why);
      await Bun.sleep(5);
    }
  };
  let host: Relay, guest: Relay;
  try {
    host = await relay(a);
    guest = await relay(b);
    const present = (r: Relay) => r.text.some((m) => m.kind === "peer" && m.present);
    await waitFor(() => present(host) && present(guest), 5000, `no peer notice: ${JSON.stringify([host.text, guest.text, host.closed, guest.closed])}`);
  } catch (e) {
    fail("relay-join", e);
  }
  const joinMs = +(performance.now() - t12).toFixed(1);
  const stranger = await relay(await createAccount(api, base, aud, tag()));
  try {
    await waitFor(() => stranger.closed !== null, 5000, "the stranger was not closed");
    if (stranger.closed!.code !== 4403) throw new Error(`closed with ${JSON.stringify(stranger.closed)}`);
  } catch (e) {
    fail("relay-stranger", e);
  }

  // Round trips: the guest sends each datagram straight back.
  guest!.onBinary = (d) => guest!.ws.send(d);
  const rtts: number[] = [];
  try {
    for (let i = 0; i < 300; i++) {
      const before = host!.got;
      const t = performance.now();
      host!.ws.send(new Uint8Array(64));
      await waitFor(() => host!.got > before, 2000, `round trip ${i} lost`);
      rtts.push(performance.now() - t);
    }
  } catch (e) {
    fail("relay-rtt", e);
  }
  guest!.onBinary = undefined;

  // Datagrams both ways at `rate` a second each for `seconds`.
  const flood = async (rate: number, seconds: number) => {
    const [h0, g0] = [host!.got, guest!.got];
    const each = Math.round(rate * seconds);
    const t = performance.now();
    for (let i = 0; i < each; i++) {
      host!.ws.send(new Uint8Array(400));
      guest!.ws.send(new Uint8Array(400));
      const due = t + ((i + 1) * 1000) / rate;
      const wait = due - performance.now();
      if (wait > 1) await Bun.sleep(wait);
    }
    await Bun.sleep(500);
    const received = host!.got - h0 + (guest!.got - g0);
    if (host!.closed || guest!.closed) throw new Error(`closed: ${JSON.stringify([host!.closed, guest!.closed])}`);
    return { each_way_per_s: rate, seconds, sent: 2 * each, received, per_s: +(received / ((performance.now() - t) / 1000)).toFixed(0) };
  };
  let coop, near;
  try {
    // Longer than a ticket lasts (15 s): it is renewed under way.
    coop = await flood(30, 17);
    near = await flood(200, 5);
  } catch (e) {
    fail("relay-flood", e);
  }
  const p = (xs: number[], q: number) => +[...xs].sort((x, y) => x - y)[Math.floor(q * (xs.length - 1))].toFixed(2);
  out.relay = {
    join_ms: joinMs,
    rtt_ms: { p50: p(rtts, 0.5), p99: p(rtts, 0.99), max: p(rtts, 1) },
    coop_rate: coop,
    near_limit: near,
  };
  host!.ws.close();
  guest!.ws.close();
}

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
