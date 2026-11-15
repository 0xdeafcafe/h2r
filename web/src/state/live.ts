// The app's one live connection: a single /v1/stream socket feeding the
// reducer (reactive, batched per tick), the frame hub (not reactive), and
// the signature verifier (async, batched).

import { batch, createRoot, createSignal } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import type { WireEvent, PlayerKilled } from "../api/types";
import { api, type Api } from "../api/http";
import { fetchWellKnown, initialBase, remember, targetFor, wsUrl, type ServerTarget, type WellKnown } from "../api/server";
import { applyEvent, initialState, type LiveState } from "./reducer";
import { FrameHub } from "./frames";
import { StreamClient, type StreamStatus } from "./stream";
import { KeyCache, Verifier, type VerifyResult, type VerifyStatus } from "./verify";

export const frameHub = new FrameHub();

function createLive() {
  const [state, setState] = createStore<LiveState>(initialState());
  const [status, setStatus] = createSignal<StreamStatus>("idle");
  const [retryIn, setRetryIn] = createSignal<number | undefined>();
  const [server, setServer] = createSignal<ServerTarget>(targetFor(initialBase()));
  const [wellKnown, setWellKnown] = createSignal<WellKnown | null>(null);
  const [serverError, setServerError] = createSignal<string | null>(null);
  const [verify, setVerify] = createStore<Record<number, VerifyResult>>({});
  const [verifySummary, setVerifySummary] = createStore({ verified: 0, bad: 0, unsigned: 0, unknown: 0 });

  let client: StreamClient | null = null;
  let verifier: Verifier | null = null;
  let pending: WireEvent[] = [];
  let flushScheduled = false;
  let caughtUpAt: number | null = null;
  let rest: Api = api(server());
  let gen = 0;

  const gamertags = new Map<string, string>();
  const listeners = new Set<(ev: WireEvent) => void>();

  function flush() {
    flushScheduled = false;
    if (!pending.length) return;
    const evs = pending;
    pending = [];
    const live = caughtUpAt !== null;
    const now = performance.now();
    setState(produce((s) => { for (const ev of evs) applyEvent(s, ev); }));
    if (live) {
      for (const ev of evs) {
        const gid = ev.stream.startsWith("game-") ? ev.stream.slice(5) : null;
        if (!gid) continue;
        if (ev.type === "PlayerKilled" && frameHub.has(gid)) {
          const d = ev.data as PlayerKilled;
          frameHub.get(gid).addDeath(d.victim, d.tick, now);
        } else if (ev.type === "GameEnded") {
          setTimeout(() => frameHub.drop(gid), 10_000);
        }
      }
    }
    for (const ev of evs) for (const l of listeners) l(ev);
  }

  function schedule() {
    if (flushScheduled) return;
    flushScheduled = true;
    // While catching up, fold big bursts into one update; when live, apply
    // on the next frame so kill feeds feel instant.
    if (typeof requestAnimationFrame !== "undefined" && caughtUpAt !== null) requestAnimationFrame(flush);
    else setTimeout(flush, 16);
  }

  function onVerifyResults(rs: VerifyResult[]) {
    batch(() => {
      setVerify(produce((v) => { for (const r of rs) v[r.position] = r; }));
      setVerifySummary(produce((s) => { for (const r of rs) s[r.status] += 1; }));
    });
  }

  function makeVerifier(target: ServerTarget, r: Api): Verifier {
    const keys = new KeyCache({
      serverKeys: async () => (await fetchWellKnown(target.base)).keys ?? [],
      accountKeys: async (author) => {
        // author = "acct:<xuid>@<host>"; keys are filed by gamertag.
        const m = /^acct:([0-9a-fA-F]{16})@/.exec(author);
        if (!m) return null;
        // Filled synchronously as events arrive, so it's ahead of the store.
        const tag = gamertags.get(m[1]!.toLowerCase()) ?? state.accounts[m[1]!]?.gamertag;
        if (!tag) return null;
        return r.accountKeys(tag);
      },
    });
    return new Verifier(keys, onVerifyResults);
  }

  function connect(target: ServerTarget) {
    const my = ++gen;
    client?.stop();
    frameHub.clear();
    pending = [];
    caughtUpAt = null;
    rest = api(target);
    gamertags.clear();
    verifier = makeVerifier(target, rest);
    batch(() => {
      setState(reconcile(initialState()));
      setVerify(reconcile({}));
      setVerifySummary({ verified: 0, bad: 0, unsigned: 0, unknown: 0 });
    });
    const r = rest;
    client = new StreamClient(
      {
        url: (from) => wsUrl(r.streamUrl(from)),
        connect: (url) => new WebSocket(url) as unknown as import("./stream").SocketLike,
      },
      {
        onEvent: (ev) => {
          if (ev.type === "AccountCreated") {
            const d = ev.data as { xuid?: string; gamertag?: string };
            if (d?.xuid && d.gamertag) gamertags.set(d.xuid.toLowerCase(), d.gamertag);
          }
          pending.push(ev);
          verifier?.enqueue(ev);
          schedule();
        },
        onFrame: (f) => frameHub.push(f, performance.now()),
        onCaughtUp: (position) => {
          flush();
          caughtUpAt = performance.now();
          setState("caughtUp", true);
          if (position > state.position) setState("position", position);
        },
        onStatus: (s, d) => {
          if (my !== gen) return;
          setStatus(s);
          setRetryIn(d.retryInMs);
        },
      },
    );
    client.start(null);
  }

  async function useServer(base: string): Promise<void> {
    setServerError(null);
    let wk: WellKnown | null = null;
    try {
      wk = await fetchWellKnown(base);
    } catch (e) {
      // Still usable without discovery (older servers); remember the error.
      setServerError((e as Error).message);
    }
    const target = targetFor(base, wk);
    setWellKnown(wk);
    setServer(target);
    remember(base, wk?.id);
    connect(target);
  }

  function start() {
    const t = server();
    connect(t);
    fetchWellKnown(t.base).then((wk) => {
      setWellKnown(wk);
      remember(t.base, wk.id);
      const t2 = targetFor(t.base, wk);
      if (t2.api !== t.api) { setServer(t2); connect(t2); }
    }, (e) => setServerError((e as Error).message));
  }

  return {
    state,
    status,
    retryIn,
    server,
    wellKnown,
    serverError,
    verify,
    verifySummary,
    useServer,
    start,
    api: () => rest,
    verifyStatus: (position: number): VerifyStatus | null => verify[position]?.status ?? null,
    onEvent(fn: (ev: WireEvent) => void) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

export const live = createRoot(createLive);
export type Live = typeof live;
