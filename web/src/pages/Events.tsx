import { For, Show, createMemo, createSignal } from "solid-js";
import { live } from "../state/live";
import type { WireEvent } from "../api/types";
import { VerifyMark } from "../components/bits";

const CATEGORY: Record<string, string> = {
  AccountCreated: "account", GamertagReserved: "account", TitleFilePublished: "file",
  SignedIn: "presence", PresenceChanged: "presence", SignedOut: "presence", PresenceExpired: "presence",
  GameStarted: "game", PlayerJoined: "game", PlayerLeft: "game", PlayerKilled: "kill", ScoreChanged: "score",
  GameFrame: "frame", GameEnded: "game",
};

export default function Events() {
  const [type, setType] = createSignal("");
  const [stream, setStream] = createSignal("");
  const [hideFrames, setHideFrames] = createSignal(true);
  const [paused, setPaused] = createSignal(false);
  const [frozen, setFrozen] = createSignal<WireEvent[]>([]);
  const [open, setOpen] = createSignal<number | null>(null);
  const [older, setOlder] = createSignal<WireEvent[]>([]);
  const [loadingOlder, setLoadingOlder] = createSignal(false);

  const types = createMemo(() => Object.entries(live.state.typeCounts).sort((a, b) => b[1] - a[1]));
  const source = () => (paused() ? frozen() : live.state.tail);
  const rows = createMemo(() => {
    const t = type(), s = stream().trim();
    const all = [...older(), ...source()];
    return all
      .filter((e) => (!t || e.type === t) && (!s || e.stream.includes(s)) && (!hideFrames() || e.type !== "GameFrame" || t === "GameFrame"))
      .slice(-300)
      .reverse();
  });

  async function loadOlder() {
    const first = (older()[0] ?? source()[0])?.position;
    if (!first) return;
    setLoadingOlder(true);
    try {
      const from = Math.max(0, first - 201); // exclusive
      const r = await live.api().events({ from, limit: 200, type: type() || undefined, stream: stream().trim() || undefined });
      setOlder([...r.items.filter((e) => e.position < first), ...older()]);
    } finally {
      setLoadingOlder(false);
    }
  }

  return (
    <div class="page events-page">
      <h1>Event log</h1>
      <p class="muted">
        Every change on the server is an event. {live.state.eventCount} seen this session, head at position {live.state.position}.{" "}
        Signatures: {live.verifySummary.verified} verified
        <Show when={live.verifySummary.bad}>, <span class="warn">{live.verifySummary.bad} bad</span></Show>
        <Show when={live.verifySummary.unknown}>, {live.verifySummary.unknown} unchecked</Show>
        <Show when={live.verifySummary.unsigned}>, {live.verifySummary.unsigned} unsigned</Show>.
      </p>
      <div class="toolbar">
        <select value={type()} onChange={(e) => setType(e.currentTarget.value)} aria-label="Event type">
          <option value="">all types</option>
          <For each={types()}>{([t, n]) => <option value={t}>{t} ({n})</option>}</For>
        </select>
        <input placeholder="stream contains… (e.g. game-, presence-)" value={stream()} onInput={(e) => setStream(e.currentTarget.value)} aria-label="Stream filter" />
        <label class="check"><input type="checkbox" checked={hideFrames()} onChange={(e) => setHideFrames(e.currentTarget.checked)} /> hide frames</label>
        <button onClick={() => { if (!paused()) setFrozen([...live.state.tail]); setPaused(!paused()); }}>{paused() ? "resume" : "pause"}</button>
        <button onClick={() => void loadOlder()} disabled={loadingOlder()}>{loadingOlder() ? "loading…" : "load older"}</button>
      </div>
      <ol class="event-list">
        <For each={rows()} fallback={<li class="muted">Nothing matches. The log is listening…</li>}>
          {(e) => (
            <li class={`ev ev--${CATEGORY[e.type] ?? "other"}`} classList={{ "ev--open": open() === e.position }}>
              <button class="ev__row" onClick={() => setOpen(open() === e.position ? null : e.position)}>
                <span class="ev__pos">#{e.position}</span>
                <span class="ev__type">{e.type}</span>
                <span class="ev__stream">{e.stream}<span class="muted">@{e.version}</span></span>
                <span class="ev__summary">{summary(e)}</span>
                <span class="ev__at muted">{e.at?.slice(11, 19)}</span>
                <VerifyMark position={e.position} compact />
              </button>
              <Show when={open() === e.position}>
                <pre class="ev__json">{JSON.stringify({ ...e, jws: e.jws ? e.jws.slice(0, 48) + "…" : undefined }, null, 2)}</pre>
                <Show when={live.verify[e.position]?.reason}><div class="warn small">{live.verify[e.position]!.reason}</div></Show>
              </Show>
            </li>
          )}
        </For>
      </ol>
    </div>
  );
}

function summary(e: WireEvent): string {
  const d = e.data as Record<string, unknown>;
  switch (e.type) {
    case "AccountCreated": return `${d.gamertag} signed up`;
    case "SignedIn": return `signed in (${d.state ?? "online"})`;
    case "SignedOut": return "signed out";
    case "PresenceExpired": return "went quiet";
    case "PresenceChanged": return `${d.state ?? ""}${d.game_id ? " in a game" : ""}`;
    case "GameStarted": return `${d.map} · ${d.variant ?? d.mode} · ${(d.players as unknown[])?.length ?? 0} players`;
    case "PlayerKilled": return `${d.killer ?? "world"} → ${d.victim} (${d.kind}${d.weapon ? ", " + d.weapon : ""})`;
    case "ScoreChanged": return `tick ${d.tick}`;
    case "GameFrame": return `tick ${d.tick}, ${(d.players as unknown[])?.length ?? 0} players`;
    case "GameEnded": {
      const w = d.winner as { team?: number; player?: number } | null | undefined;
      const who = w ? (w.team !== undefined ? `team ${w.team} won` : `slot ${w.player} won`) : "no winner";
      return `${d.reason}, ${who}`;
    }
    default: return "";
  }
}
