import { For, Show, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import { adminState } from "../admin/state";
import { DeckPage, Panel } from "../deck/Deck";
import { eventSummary, EVENT_WINDOW, eventWindowStart, readEventWindow } from "../lib/eventLog";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { keyAction, watchGamepads, type Action } from "./input";
import "../styles/event-log.css";

const BACK = "/dash/settings";

export default function EventLog() {
  const scope = () => {
    const me = identity.me();
    return me && live.wellKnown() && adminState.isMeAdmin() === true && JSON.stringify([live.server().api, me.xuid]);
  };
  const navigate = useNavigate();
  onMount(() => onCleanup(watchGamepads((action) => {
    if (action === "back") navigate(BACK);
  })));

  return (
    <DeckPage title="Event Log" trail={[["dashboard", "/"], ["server", "/dash/server"], ["live admin", "/dash/admin"], ["event log"]]} back={BACK}>
      <Show when={scope()} keyed fallback={
        <Panel title="Admin access">
          <p class="dk-note">
            <Show when={identity.me()} fallback={<><A href="/dash/signin">Sign in</A> with a server admin account to read this log.</>}>
              {adminState.isMeAdmin() === null ? "Checking admin access…" : "This log is available to server admins."}
            </Show>
          </p>
        </Panel>
      }>
        {(_scope) => <AdminEvents />}
      </Show>
    </DeckPage>
  );
}

function AdminEvents() {
  const [end, setEnd] = createSignal<number | null>(null);
  const [head, setHead] = createSignal(0);
  const [filter, setFilter] = createSignal("");
  const [open, setOpen] = createSignal<number | null>(null);
  let panel!: HTMLDivElement;
  let cancel: AbortController | undefined;
  const [page, { refetch }] = createResource(() => ({ end: end() }), async ({ end }) => {
    cancel?.abort();
    const abort = new AbortController();
    cancel = abort;
    const api = live.api();
    try {
      const until = end ?? (await api.events({ limit: 1 }, abort.signal)).position ?? live.state.position;
      const events = await readEventWindow(api, until, abort.signal);
      if (end === null && !abort.signal.aborted) setHead(until);
      return { until, events: events.reverse() };
    } catch (error) {
      return { error: (error as Error).message };
    }
  });
  onCleanup(() => cancel?.abort());
  const from = () => eventWindowStart(page()?.until ?? 0);
  const rows = createMemo(() => {
    const q = filter().trim().toLowerCase();
    return (page()?.events ?? []).filter((e) => !q || e.type.toLowerCase().includes(q) || e.stream.toLowerCase().includes(q));
  });

  function older() {
    if (page.loading || !from()) return;
    setOpen(null);
    setEnd(from());
  }
  function newer() {
    const until = end();
    if (page.loading || until === null || until >= head()) return;
    setOpen(null);
    setEnd(Math.min(head(), until + EVENT_WINDOW));
  }
  function latest() {
    if (page.loading) return;
    setOpen(null);
    if (end() === null) void refetch();
    else setEnd(null);
  }

  function move(action: Action): boolean {
    if (action === "pageUp") { newer(); return true; }
    if (action === "pageDown") { older(); return true; }
    const controls = Array.from(panel.querySelectorAll<HTMLButtonElement | HTMLInputElement>("[data-log-control]"));
    const active = document.activeElement;
    const index = controls.findIndex((element) => element === active);
    if (action === "select") {
      if (active instanceof HTMLButtonElement && panel.contains(active)) { active.click(); return true; }
      if (index < 0) controls[0]?.focus();
      return false;
    }
    if (action !== "up" && action !== "down" && action !== "left" && action !== "right") return false;
    const step = action === "up" || action === "left" ? -1 : 1;
    const next = controls[index < 0 ? 0 : Math.max(0, Math.min(controls.length - 1, index + step))];
    next?.focus();
    return true;
  }

  onMount(() => {
    const key = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey) return;
      const action = keyAction(e.key);
      if (!action || action === "back" || e.key === "Enter") return;
      if (e.target instanceof HTMLInputElement && action !== "up" && action !== "down") return;
      if (move(action)) e.preventDefault();
    };
    window.addEventListener("keydown", key);
    const stopPad = watchGamepads((action) => { move(action); });
    onCleanup(() => { stopPad(); window.removeEventListener("keydown", key); });
  });

  return (
    <Panel title="Server events" meta={page()?.until && !page.loading ? `positions ${from() + 1}–${page()!.until}` : undefined}>
      <div class="dk-event-log" ref={panel}>
        <div class="dk-event-tools" role="group" aria-label="Event log controls">
          <input data-log-control value={filter()} aria-label="Filter this page by event type or stream"
            placeholder="Filter this page by type or stream" onInput={(e) => setFilter(e.currentTarget.value)} />
          <button data-log-control type="button" class="dk-pill" aria-disabled={page.loading} onClick={latest}>Latest</button>
          <button data-log-control type="button" class="dk-pill" aria-disabled={page.loading || (end() ?? head()) >= head()} onClick={newer}>Newer</button>
          <button data-log-control type="button" class="dk-pill" aria-disabled={page.loading || !from()} onClick={older}>Older</button>
        </div>
        <Show when={page.loading}><p class="dk-note" role="status">Reading events…</p></Show>
        <Show when={!page.loading && page()?.error}>
          {(error) => <p class="dk-note" role="alert">{error()} <button data-log-control type="button" class="dk-pill" onClick={() => void refetch()}>Retry</button></p>}
        </Show>
        <Show when={!page.loading && !page()?.error}>
          <div class="dk-event-table">
            <div class="dk-event-columns" aria-hidden="true"><span>Position</span><span>Type</span><span>Stream</span><span>Summary</span><span>Time (UTC)</span></div>
            <ul>
              <For each={rows()} fallback={<li class="dk-note">No matching events in this range.</li>}>
                {(e) => (
                  <li>
                    <button type="button" class="dk-event-row" data-log-control aria-expanded={open() === e.position}
                      onClick={() => setOpen(open() === e.position ? null : e.position)}>
                      <span>#{e.position}</span><b>{e.type}</b><span>{e.stream}</span>
                      <span>{eventSummary(e)}</span><time dateTime={e.at}>{e.at?.slice(11, 19)}</time>
                    </button>
                    <Show when={open() === e.position}><pre>{JSON.stringify(e, null, 2)}</pre></Show>
                  </li>
                )}
              </For>
            </ul>
          </div>
        </Show>
      </div>
    </Panel>
  );
}
