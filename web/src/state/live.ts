// The app's one live connection: a single /v1/stream socket feeding the
// reducer (reactive, batched per tick), the frame hub (not reactive), and
// the signature verifier (async, batched).

import { batch, createRoot, createSignal } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import type { WireEvent, PlayerKilled, GameSummary } from "../api/types";
import { api, type Api } from "../api/http";
import { fetchWellKnown, initialBase, remember, targetFor, wsUrl, type ServerTarget, type WellKnown } from "../api/server";
import {
  addGamePositions, applyAccountsSnapshot, applyEvent, applyGameSnapshot, applyPresenceSnapshot, initialState, seedTail, type LiveState,
} from "./reducer";
import { confirmGames, loadSnapshot } from "./snapshot";
import { FrameHub } from "./frames";
import { StreamClient, type StreamStatus } from "./stream";
import { KeyCache, Verifier, authorXuid, type VerifyResult, type VerifyStatus } from "./verify";

export const frameHub = new FrameHub();

function createLive() {
  const [state, setState] = createStore<LiveState>(initialState());
  const [status, setStatus] = createSignal<StreamStatus>("idle");
  const [retryIn, setRetryIn] = createSignal<number | undefined>();
  const [server, setServer] = createSignal<ServerTarget>(targetFor(initialBase()));
  const [wellKnown, setWellKnown] = createSignal<WellKnown | null>(null);
  const [serverError, setServerError] = createSignal<string | null>(null);
  const [verify, setVerify] = createStore<Record<number, VerifyResult>>({});
  const [verifySummary, setVerifySummary] = createStore({ verified: 0, bad: 0, unsigned: 0, redacted: 0, unknown: 0 });
  /** "snapshot" while loading REST snapshots, then "stream" (or "replay" when
   *  the server gave no positions and we replay the whole log). */
  const [loadMode, setLoadMode] = createSignal<"snapshot" | "stream" | "replay">("snapshot");

  let client: StreamClient | null = null;
  let verifier: Verifier | null = null;
  let pending: WireEvent[] = [];
  let flushScheduled = false;
  let caughtUpAt: number | null = null;
  let rest: Api = api(server(), () => reader());
  /** A one-use `read` token for the signed-in account (state/identity.ts
   *  sets it), or null: reads that depend on the reader use it. */
  let reader: () => Promise<string | null> = async () => null;
  const asReader = async (id: string) => rest.game(id, undefined, await reader().catch(() => null));

  /** Games the stream held back as maybe private: each is read as the
   *  reader, and added only when the server shows it. */
  function confirmHeld() {
    const ids = [...state.held];
    if (!ids.length) return;
    setState("held", []);
    const my = gen;
    void confirmGames(ids, asReader).then((list) => {
      if (my !== gen || !list.length) return;
      setState(produce((s) => { for (const d of list) applyGameSnapshot(s, d); }));
    });
  }
  let gen = 0;

  const listeners = new Set<(ev: WireEvent) => void>();

  function flush() {
    flushScheduled = false;
    if (!pending.length) return;
    const evs = pending;
    pending = [];
    const live = caughtUpAt !== null;
    const now = performance.now();
    setState(produce((s) => { for (const ev of evs) applyEvent(s, ev); }));
    confirmHeld();
    if (live) {
      for (const ev of evs) {
        const gid = ev.stream.startsWith("game-") ? ev.stream.slice(5) : null;
        if (!gid) continue;
        if (ev.type === "PlayerKilled" && frameHub.has(gid)) {
          const d = ev.data as PlayerKilled;
          frameHub.get(gid).addDeath(d.victim, d.tick, now, d.pos);
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
      // Discovery already read them; ask again only if it did not.
      serverKeys: async () => {
        const wk = wellKnown();
        return (wk && server().base === target.base ? wk : await fetchWellKnown(target.base)).keys ?? [];
      },
      accountKeys: async (author) => {
        const xuid = authorXuid(author);
        return xuid ? r.accountKeys(xuid) : null;
      },
    });
    return new Verifier(keys, onVerifyResults);
  }

  let abort: AbortController | null = null;

  function connect(target: ServerTarget) {
    const my = ++gen;
    abort?.abort();
    abort = new AbortController();
    const signal = abort.signal;
    client?.stop();
    client = null;
    frameHub.clear();
    pending = [];
    caughtUpAt = null;
    rest = api(target, () => reader());
    verifier = makeVerifier(target, rest);
    batch(() => {
      setState(reconcile(initialState()));
      setVerify(reconcile({}));
      setVerifySummary({ verified: 0, bad: 0, unsigned: 0, redacted: 0, unknown: 0 });
    });
    const r = rest;
    setLoadMode("snapshot");
    setStatus("connecting");
    void loadSnapshot(r, signal).then(
      (snap) => {
        if (my !== gen) return;
        if (!snap) { setLoadMode("replay"); openStream(my, r, null); return; }
        setState(produce((s) => {
          applyAccountsSnapshot(s, snap.accounts);
          applyPresenceSnapshot(s, snap.presence);
          for (const g of snap.games) applyGameSnapshot(s, g);
          seedTail(s, snap.tail);
          s.position = snap.position;
        }));
        for (const ev of snap.tail) verifier?.enqueue(ev);
        setLoadMode("stream");
        openStream(my, r, snap.position);
      },
      () => {
        if (my !== gen || signal.aborted) return;
        // No snapshot endpoints (or a hiccup): replay the log instead.
        setLoadMode("replay");
        openStream(my, r, null);
      },
    );
  }

  function openStream(my: number, r: Api, from: number | null) {
    client = new StreamClient(
      {
        url: (from, auth) => wsUrl(r.streamUrl(from, auth)),
        auth: () => r.readToken(),
        connect: (url) => {
          const ws = new WebSocket(url);
          // Frames come packed, as binary messages.
          ws.binaryType = "arraybuffer";
          return ws as unknown as import("./stream").SocketLike;
        },
      },
      {
        onEvent: (ev) => {
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
    client.start(from);
  }

  /** Loads one game's detail into the store (deep links, profiles). */
  const loading = new Map<string, Promise<boolean>>();
  function ensureGame(id: string): Promise<boolean> {
    // Known well enough: live (the stream fills it in), or ended with its end
    // known (from GameEnded or a detail read). A game known only from a
    // summary (`GET /v1/games`, an account's recent games) has players but no
    // scoreboard, kills or end reason yet, so it is read in full.
    const g = state.games[id];
    if (g && (g.status === "live" || g.end_reason !== null)) return Promise.resolve(true);
    const key = server().api + "|" + id;
    let p = loading.get(key);
    if (!p) {
      const my = gen;
      p = asReader(id).then((d) => {
        if (my !== gen) return false;
        setState(produce((s) => applyGameSnapshot(s, d)));
        return true;
      }, () => false);
      loading.set(key, p);
      void p.then((ok) => { if (!ok) loading.delete(key); });
    }
    return p;
  }

  /** Adds games read from `GET /v1/games` (a filtered list, say) to the store. */
  function ingestGames(list: GameSummary[]) {
    if (!list.length) return;
    setState(produce((s) => { for (const g of list) applyGameSnapshot(s, g); }));
  }

  /** Checks the signatures of a game's events read over REST (a replay
   *  loads the whole game stream), and counts them in its summary. Events
   *  the stream already queued are not checked twice. */
  function checkGameEvents(id: string, evs: WireEvent[]) {
    let added: number[] = [];
    setState(produce((s) => { added = addGamePositions(s, id, evs); }));
    const fresh = new Set(added);
    for (const ev of evs) if (fresh.has(ev.position) && !verify[ev.position]) verifier?.enqueue(ev);
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
    const base = server().base;
    // Discover first so a relative or moved `api` is honoured from the start.
    fetchWellKnown(base).then((wk) => {
      setWellKnown(wk);
      remember(base, wk.id);
      const t = targetFor(base, wk);
      setServer(t);
      connect(t);
    }, (e) => {
      setServerError((e as Error).message);
      connect(server());
    });
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
    loadMode,
    ensureGame,
    ingestGames,
    checkGameEvents,
    useServer,
    /** Starts the connection over on the same server: the snapshot and the
     *  stream again, as whoever reads now (a sign-in or sign-out). */
    reload: () => connect(server()),
    start,
    api: () => rest,
    setReader(fn: () => Promise<string | null>) { reader = fn; },
    verifyStatus: (position: number): VerifyStatus | null => verify[position]?.status ?? null,
    onEvent(fn: (ev: WireEvent) => void) {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  };
}

export const live = createRoot(createLive);
export type Live = typeof live;
