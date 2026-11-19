// The /v1/stream WebSocket client: reconnects with backoff and resumes from
// the last position it applied. Transport and timers are injected so the
// logic is testable without a socket.

import type { StreamMessage, WireEvent, Frame } from "../api/types";
import { FrameDecoder, PackError } from "../api/pack";

export interface SocketLike {
  onopen: ((ev: unknown) => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: ((ev: unknown) => void) | null;
  onerror: ((ev: unknown) => void) | null;
  close(): void;
  /** Sends a text message (the auth message). */
  send?(data: string): void;
}

export type StreamStatus = "idle" | "connecting" | "catching-up" | "live" | "reconnecting" | "stopped";

export interface StreamHandlers {
  onEvent(ev: WireEvent): void;
  onFrame(frame: Frame): void;
  onCaughtUp(position: number): void;
  onStatus?(status: StreamStatus, detail: { attempt: number; retryInMs?: number }): void;
}

export interface StreamOptions {
  /** Builds the socket URL. `from` is null for a fresh start from the
   *  beginning; `auth` asks the server to wait for an auth message. */
  url(from: number | null, auth?: boolean): string;
  /** A `read` token for each (re)connect, sent as the first message so the
   *  server shows this reader what its account may see (a friends-only
   *  co-op game); null reads anonymously. */
  auth?(): Promise<string | null>;
  connect(url: string): SocketLike;
  setTimer?(fn: () => void, ms: number): unknown;
  clearTimer?(handle: unknown): void;
  random?(): number;
  baseDelayMs?: number;
  maxDelayMs?: number;
}

/** Exponential backoff with jitter in [0.5, 1) of the nominal delay. */
export function backoffDelay(attempt: number, base: number, max: number, random: number): number {
  const nominal = Math.min(max, base * 2 ** attempt);
  return Math.round(nominal * (0.5 + random * 0.5));
}

export class StreamClient {
  /** Highest event position applied; resume point. */
  position: number | null = null;
  status: StreamStatus = "idle";
  attempt = 0;
  private sock: SocketLike | null = null;
  private timer: unknown = null;
  private stopped = false;
  private readonly o: Required<Omit<StreamOptions, "auth">> & Pick<StreamOptions, "auth">;
  /** Packed frame streams (`?frames=packed`); a new socket starts afresh. */
  private frames = new FrameDecoder();

  constructor(opts: StreamOptions, private readonly h: StreamHandlers) {
    this.o = {
      setTimer: (fn, ms) => setTimeout(fn, ms),
      clearTimer: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
      random: Math.random,
      baseDelayMs: 500,
      maxDelayMs: 15000,
      ...opts,
    };
  }

  start(from: number | null = this.position): void {
    this.stopped = false;
    this.position = from;
    this.open();
  }

  stop(): void {
    this.stopped = true;
    if (this.timer !== null) this.o.clearTimer(this.timer);
    this.timer = null;
    const s = this.sock;
    this.sock = null;
    if (s) { s.onclose = null; s.onmessage = null; s.onerror = null; s.onopen = null; s.close(); }
    this.setStatus("stopped");
  }

  /** The URL a (re)connect uses right now. We ask for the last applied
   *  position again (the doc doesn't say whether `from` is inclusive) and
   *  drop anything at or below it. */
  resumeUrl(): string {
    return this.o.url(this.position);
  }

  private setStatus(s: StreamStatus, retryInMs?: number) {
    this.status = s;
    this.h.onStatus?.(s, { attempt: this.attempt, retryInMs });
  }

  private open() {
    this.setStatus(this.attempt === 0 ? "connecting" : "reconnecting");
    const opened = ++this.opens;
    const auth = this.o.auth;
    // Without a reader, connect at once (anonymously).
    if (!auth) { this.connect(null); return; }
    void auth().catch(() => null).then((token) => {
      if (this.stopped || opened !== this.opens) return;
      this.connect(token);
    });
  }

  private opens = 0;

  private connect(token: string | null) {
    let sock: SocketLike;
    try {
      sock = this.o.connect(this.o.url(this.position, token !== null));
    } catch {
      this.scheduleReconnect();
      return;
    }
    this.sock = sock;
    this.frames = new FrameDecoder();
    sock.onopen = () => {
      if (this.sock !== sock) return;
      if (token !== null) sock.send?.(JSON.stringify({ kind: "auth", token }));
      this.setStatus("catching-up");
    };
    sock.onmessage = (m) => { if (this.sock === sock) this.handle(m.data); };
    sock.onerror = () => { /* onclose follows */ };
    sock.onclose = () => {
      if (this.sock !== sock) return;
      this.sock = null;
      if (!this.stopped) this.scheduleReconnect();
    };
  }

  private scheduleReconnect() {
    const delay = backoffDelay(this.attempt, this.o.baseDelayMs, this.o.maxDelayMs, this.o.random());
    this.attempt += 1;
    this.setStatus("reconnecting", delay);
    this.timer = this.o.setTimer(() => { this.timer = null; if (!this.stopped) this.open(); }, delay);
  }

  /** Handles one raw message. Exposed for tests. Binary messages are
   *  packed frame records (docs/live/live-telemetry.md, Packed frames). */
  handle(raw: unknown): void {
    if (raw instanceof ArrayBuffer || ArrayBuffer.isView(raw)) {
      const bytes = raw instanceof ArrayBuffer ? new Uint8Array(raw) : new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength);
      this.handlePacked(bytes);
      return;
    }
    let msg: StreamMessage;
    try {
      msg = (typeof raw === "string" ? JSON.parse(raw) : raw) as StreamMessage;
    } catch {
      return;
    }
    switch (msg.kind) {
      case "event": {
        const ev = msg.event;
        if (!ev || typeof ev.position !== "number") return;
        if (this.position !== null && ev.position <= this.position) return; // duplicate after resume
        this.position = ev.position;
        this.h.onEvent(ev);
        break;
      }
      case "frame":
        if (msg.frame) this.h.onFrame(msg.frame);
        break;
      case "caught_up":
        // A healthy connection: forget past failures.
        this.attempt = 0;
        if (typeof msg.position === "number" && (this.position === null || msg.position > this.position)) {
          // Nothing to replay up to the head; resume from there next time.
          this.position = msg.position;
        }
        this.setStatus("live");
        this.h.onCaughtUp(msg.position);
        break;
    }
  }

  private handlePacked(bytes: Uint8Array) {
    try {
      this.frames.forEachRecord(bytes, (frame) => this.h.onFrame(frame), true);
    } catch (e) {
      if (!(e instanceof PackError)) throw e;
      // A bad message: drop the rest of it.
    }
  }
}
