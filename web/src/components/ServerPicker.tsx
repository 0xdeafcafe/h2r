import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { live } from "../state/live";
import { defaultBase, fetchWellKnown, forget, normalizeBase, recentServers, type RecentServer } from "../api/server";

/** Choose which live server to watch, like picking a server in a social app. */
export default function ServerPicker() {
  const [open, setOpen] = createSignal(false);
  const [input, setInput] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  const [recent, setRecent] = createSignal<RecentServer[]>(recentServers());
  let root!: HTMLDivElement;

  const onDoc = (e: MouseEvent) => { if (open() && !root.contains(e.target as Node)) setOpen(false); };
  onMount(() => document.addEventListener("mousedown", onDoc));
  onCleanup(() => document.removeEventListener("mousedown", onDoc));

  async function choose(raw: string) {
    setErr(null);
    let base: string;
    try {
      base = normalizeBase(raw, location.protocol);
    } catch (e) {
      setErr((e as Error).message);
      return;
    }
    setBusy(true);
    try {
      await fetchWellKnown(base);
    } catch (e) {
      setBusy(false);
      setErr(`Couldn't reach that one: ${(e as Error).message}`);
      return;
    }
    await live.useServer(base);
    setBusy(false);
    setRecent(recentServers());
    setInput("");
    setOpen(false);
    const u = new URL(location.href);
    if (base === defaultBase()) u.searchParams.delete("server"); else u.searchParams.set("server", base);
    history.replaceState(history.state, "", u);
  }

  const label = () => live.wellKnown()?.id ?? new URL(live.server().base).host;

  return (
    <div class="server-picker" ref={root}>
      <button class="server-pill" onClick={() => { setRecent(recentServers()); setOpen(!open()); }} aria-expanded={open()}>
        <span class={`conn-dot conn-dot--${live.status()}`} />
        <span class="server-pill__name">{label()}</span>
      </button>
      <Show when={open()}>
        <div class="server-pop" role="dialog" aria-label="Choose a server">
          <h4>Which server shall we peek at?</h4>
          <div class="server-current">
            <div><b>{label()}</b></div>
            <div class="muted">{live.server().base}</div>
            <Show when={live.wellKnown()?.software}><div class="muted">{live.wellKnown()!.software}</div></Show>
            <Show when={live.serverError()}><div class="warn">{live.serverError()}</div></Show>
          </div>
          <form onSubmit={(e) => { e.preventDefault(); void choose(input()); }}>
            <input
              placeholder="live.example.net or http://127.0.0.1:8074"
              value={input()}
              onInput={(e) => setInput(e.currentTarget.value)}
              aria-label="Server address"
            />
            <button type="submit" disabled={busy() || !input().trim()}>{busy() ? "Knocking…" : "Go"}</button>
          </form>
          <Show when={err()}><div class="warn">{err()}</div></Show>
          <ul class="server-recent">
            <Show when={live.server().base !== defaultBase()}>
              <li><button class="linkish" onClick={() => void choose(defaultBase())}>This site ({new URL(defaultBase()).host})</button></li>
            </Show>
            <For each={recent().filter((r) => r.base !== live.server().base)}>
              {(r) => (
                <li>
                  <button class="linkish" onClick={() => void choose(r.base)}>{r.id ?? r.base}</button>
                  <small class="muted"> {r.id ? r.base : ""}</small>
                  <button class="x" title="Forget" onClick={() => { forget(r.base); setRecent(recentServers()); }}>×</button>
                </li>
              )}
            </For>
          </ul>
        </div>
      </Show>
    </div>
  );
}
