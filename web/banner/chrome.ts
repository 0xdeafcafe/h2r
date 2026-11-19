// Headless Chrome for the scripts that draw pictures (`bun run banner`,
// `bun run brand`): finding it, starting it, and a minimal DevTools
// protocol client.

import { existsSync } from "node:fs";

export function defaultChrome(): string {
  if (process.env.CHROME) return process.env.CHROME;
  const found = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
  ].find((p) => existsSync(p));
  if (!found) throw new Error("no Chrome found: pass --chrome PATH or set $CHROME");
  return found;
}

/** A minimal DevTools protocol client over one WebSocket. */
export class Cdp {
  private id = 0;
  private waiting = new Map<number, { ok: (v: any) => void; fail: (e: Error) => void }>();
  private listeners = new Set<(m: any) => void>();
  private constructor(private ws: WebSocket) {
    ws.onmessage = (e) => {
      const m = JSON.parse(String(e.data));
      // The page's console, for --verbose.
      if (m.method === "Runtime.consoleAPICalled" && process.argv.includes("--verbose")) console.error("page:", ...m.params.args.map((a: any) => a.value ?? a.description));
      if (m.method) for (const l of this.listeners) l(m);
      const w = m.id !== undefined ? this.waiting.get(m.id) : undefined;
      if (!w) return;
      this.waiting.delete(m.id);
      if (m.error) w.fail(new Error(m.error.message)); else w.ok(m.result);
    };
  }
  static open(url: string): Promise<Cdp> {
    return new Promise((ok, fail) => {
      const ws = new WebSocket(url);
      ws.onopen = () => ok(new Cdp(ws));
      ws.onerror = () => fail(new Error(`could not reach ${url}`));
    });
  }
  send(method: string, params: object = {}, sessionId?: string): Promise<any> {
    const id = ++this.id;
    this.ws.send(JSON.stringify({ id, method, params, sessionId }));
    return new Promise((ok, fail) => this.waiting.set(id, { ok, fail }));
  }
  /** Every event (`method` set), with its `sessionId`, until unlisten. */
  listen(l: (m: any) => void) { this.listeners.add(l); }
  unlisten(l: (m: any) => void) { this.listeners.delete(l); }
  close() { this.ws.close(); }
}

export async function launchChrome(path: string, profile: string): Promise<{ proc: ReturnType<typeof Bun.spawn>; ws: string }> {
  const proc = Bun.spawn([path,
    "--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`,
    "--no-first-run", "--no-default-browser-check", "--ignore-gpu-blocklist", "--enable-webgl",
    "--hide-scrollbars", "--mute-audio", "--window-size=1920,1080", "about:blank"],
  { stdout: "ignore", stderr: "pipe" });
  const reader = proc.stderr.getReader();
  let text = "";
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const { value, done } = await reader.read();
    if (done) break;
    text += new TextDecoder().decode(value);
    const m = /DevTools listening on (ws:\/\/\S+)/.exec(text);
    if (m) { reader.releaseLock(); return { proc, ws: m[1]! }; }
  }
  proc.kill();
  throw new Error(`Chrome did not start:\n${text}`);
}

