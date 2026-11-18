import { For, Show, createMemo, createResource, createSignal, onCleanup } from "solid-js";
import { live } from "../state/live";
import { identity } from "../state/identity";
import type { WireEvent } from "../api/types";
import { eventSummary, eventWindowStart, readEventWindow } from "../lib/eventLog";
import { VerifyMark } from "../components/bits";
import { Crumbs, Panel, Title } from "./Layout";
import { Pills } from "./Side";
import { num } from "./fmt";
import { STATS_EVENT_KINDS, isStatsEvent } from "./eventKinds";

export default function Events() {
  const scope = () => JSON.stringify([live.server().api, identity.me()?.xuid]);
  return (
    <>
      <Crumbs trail={[["Home", "/stats"], ["Event Log"]]} />
      <Show when={scope()} keyed>{(_scope) => <StatsEventLog />}</Show>
    </>
  );
}

function StatsEventLog() {
  const [type, setType] = createSignal("");
  const [stream, setStream] = createSignal("");
  const [frozen, setFrozen] = createSignal<WireEvent[] | null>(null);
  const [open, setOpen] = createSignal<number | null>(null);
  const [end, setEnd] = createSignal<number>();
  let cancel: AbortController | undefined;
  const [history, { refetch }] = createResource(end, async (until) => {
    const abort = new AbortController();
    cancel?.abort();
    cancel = abort;
    try {
      return { events: await readEventWindow(live.api(), until, abort.signal) };
    } catch (error) {
      return { error: (error as Error).message };
    }
  });
  onCleanup(() => cancel?.abort());

  const tail = () => frozen() ?? live.state.tail;
  const source = () => end() === undefined ? tail() : history.loading ? [] : history()?.events ?? [];
  const stats = createMemo(() => source().filter(isStatsEvent));
  const types = createMemo(() => {
    const counts = new Map<string, number>();
    for (const event of stats()) counts.set(event.type, (counts.get(event.type) ?? 0) + 1);
    return counts;
  });
  const verified = createMemo(() => {
    const counts = { verified: 0, bad: 0, unknown: 0, unsigned: 0, redacted: 0 };
    for (const event of stats()) counts[live.verifyStatus(event.position) ?? "unknown"]++;
    return counts;
  });
  const rows = createMemo(() => {
    const t = type(), s = stream().trim();
    return stats().filter((e) => (!t || e.type === t) && (!s || e.stream.includes(s))).reverse();
  });
  const older = () => {
    const until = end();
    return until === undefined ? Math.max(0, (tail()[0]?.position ?? 1) - 1) : eventWindowStart(until);
  };

  return (
    <div class="s-cols">
      <aside class="s-left">
        <Title>Event Log</Title>
        <Pills label="Event log" items={[
          { label: "h2 live Online", href: "/stats" },
          { label: "Players & Leaderboards", href: "/stats/players" },
          { label: "Server Details", href: "/dash/server" },
        ]} />
        <Panel title="Signature Check" sub="game record events on this page">
          <dl class="s-rows">
            <div><dt>Verified</dt><dd>{num(verified().verified)}</dd></div>
            <div><dt>Failed</dt><dd classList={{ warn: verified().bad > 0 }}>{num(verified().bad)}</dd></div>
            <div><dt>Unchecked</dt><dd>{num(verified().unknown)}</dd></div>
            <div><dt>Unsigned</dt><dd>{num(verified().unsigned)}</dd></div>
            <div><dt>Redacted</dt><dd>{num(verified().redacted)}</dd></div>
          </dl>
        </Panel>
        <Panel title="Event Types" sub="game record events on this page">
          <dl class="s-rows">
            <div><dt><button class="s-linkbtn" onClick={() => setType("")}>All Stats Events</button></dt><dd>{num(stats().length)}</dd></div>
            <For each={Object.keys(STATS_EVENT_KINDS)}>
              {(t) => <div><dt><button class="s-linkbtn" classList={{ on: type() === t }} onClick={() => setType(type() === t ? "" : t)}>{t}</button></dt><dd>{num(types().get(t) ?? 0)}</dd></div>}
            </For>
          </dl>
        </Panel>
      </aside>
      <div class="s-right events-page">
        <Title right={<span class="s-dim">{end() !== undefined ? "archive" : frozen() ? "paused" : live.state.caughtUp ? "live" : "catching up"}</span>}>Halo Stats Events</Title>
        <div class="s-matte" style={{ padding: "8px" }}>
          <div class="s-toolbar">
            <label class="sr-only" for="ev-type">Event type</label>
            <select id="ev-type" value={type()} onChange={(e) => setType(e.currentTarget.value)}>
              <option value="">all stats types</option>
              <For each={Object.keys(STATS_EVENT_KINDS)}>{(t) => <option value={t}>{t} ({types().get(t) ?? 0})</option>}</For>
            </select>
            <input placeholder="stream contains…" value={stream()} onInput={(e) => setStream(e.currentTarget.value)} aria-label="Stream filter" />
            <Show when={end() !== undefined} fallback={
              <button class="s-btn" onClick={() => setFrozen(frozen() ? null : [...live.state.tail])}>{frozen() ? "resume" : "pause"}</button>
            }>
              <button class="s-btn" onClick={() => { setEnd(undefined); setFrozen(null); setOpen(null); }}>back to live</button>
            </Show>
            <button class="s-btn" onClick={() => { setEnd(older()); setOpen(null); }} disabled={!older() || history.loading}>{history.loading ? "loading…" : "load older"}</button>
          </div>
          <Show when={end() !== undefined && history()?.error}>
            {(error) => <p class="s-pad" role="alert">{error()} <button class="s-btn" onClick={() => void refetch()}>retry</button></p>}
          </Show>
          <div class="s-events">
            <div class="s-events-head"><span>#</span><span>Type</span><span>Stream</span><span>Summary</span><span>Time</span><span /></div>
            <ol class="event-list">
              <For each={rows()} fallback={<li class="s-empty">{history.loading ? "Loading game events…" : "No matching stats events."}</li>}>
                {(e) => (
                  <li class={`ev ev--${STATS_EVENT_KINDS[e.type]!.category}`} classList={{ "ev--open": open() === e.position }}>
                    <button class="ev__row" aria-expanded={open() === e.position} onClick={() => setOpen(open() === e.position ? null : e.position)}>
                      <span class="ev__pos">{e.position}</span>
                      <span class="ev__type">{e.type}</span>
                      <span class="ev__stream">{e.stream}<span class="muted">@{e.version}</span></span>
                      <span class="ev__summary">{eventSummary(e)}</span>
                      <span class="ev__at">{e.at?.slice(11, 19)}</span>
                      <VerifyMark position={e.position} compact />
                    </button>
                    <Show when={open() === e.position}>
                      <pre class="ev__json">{JSON.stringify({ ...e, jws: e.jws ? e.jws.slice(0, 48) + "…" : undefined }, null, 2)}</pre>
                      <Show when={live.verify[e.position]?.reason}><div class="warn small s-pad">{live.verify[e.position]!.reason}</div></Show>
                    </Show>
                  </li>
                )}
              </For>
            </ol>
          </div>
        </div>
      </div>
    </div>
  );
}
