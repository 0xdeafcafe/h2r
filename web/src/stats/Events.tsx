import { gameMapTitle } from "../state/maps";
import { For, Show, createMemo, createSignal } from "solid-js";
import { live } from "../state/live";
import type { WireEvent } from "../api/types";
import { recordedFrame } from "../api/pack";
import { VerifyMark } from "../components/bits";
import { Crumbs, Panel, Title } from "./Layout";
import { Pills } from "./Side";
import { num } from "./fmt";
import { STATS_NAME } from "../brand";
import { gameTypeLine } from "../state/gametype";

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

  // Counts come from the stream; before it has said anything, count what the snapshot loaded.
  const types = createMemo(() => {
    let c: Record<string, number> = live.state.typeCounts;
    if (!Object.keys(c).length) {
      c = {};
      for (const e of live.state.tail) c[e.type] = (c[e.type] ?? 0) + 1;
    }
    return Object.entries(c).sort((a, b) => b[1] - a[1]);
  });
  const seen = () => Math.max(live.state.eventCount, live.state.tail.length);
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

  const v = () => live.verifySummary;
  return (
    <>
      <Crumbs trail={[["Home", "/stats"], ["Event Log"]]} />
      <div class="s-cols">
        <aside class="s-left">
          <Title>Event Log</Title>
          <Pills label="Event log" items={[
            { label: STATS_NAME, href: "/stats" },
            { label: "Players & Leaderboards", href: "/stats/players" },
            { label: "Server Details", href: "/dash/server" },
          ]} />
          <Panel title="Signature Check" sub="verified in this browser">
            <dl class="s-rows">
              <div><dt>Verified</dt><dd>{num(v().verified)}</dd></div>
              <div><dt>Failed</dt><dd classList={{ warn: v().bad > 0 }}>{num(v().bad)}</dd></div>
              <div><dt>Unchecked</dt><dd>{num(v().unknown)}</dd></div>
              <div><dt>Unsigned</dt><dd>{num(v().unsigned)}</dd></div>
              <div title="addresses hidden, signature checked by the server only"><dt>Redacted</dt><dd>{num(v().redacted)}</dd></div>
            </dl>
          </Panel>
          <Panel title="Event Types" sub="seen this session; pick one to filter">
            <dl class="s-rows">
              <div><dt><button class="s-linkbtn" onClick={() => setType("")}>All Events</button></dt><dd>{num(seen())}</dd></div>
              <For each={types()}>
                {([t, n]) => <div><dt><button class="s-linkbtn" classList={{ on: type() === t }} onClick={() => setType(type() === t ? "" : t)}>{t}</button></dt><dd>{num(n)}</dd></div>}
              </For>
            </dl>
          </Panel>
        </aside>
        <div class="s-right events-page">
          <Title right={<span class="s-dim" style={{ color: "#9d9d9d" }}>{paused() ? "paused" : live.state.caughtUp ? `caught up to #${live.state.position}` : "catching up"}</span>}>Signed Event Stream</Title>
          <div class="s-matte" style={{ padding: "8px" }}>
            <p style={{ padding: "0 2px 8px", "font-size": "11px" }}>
              Every change on the server is an event. {num(seen())} loaded this session, head at position {live.state.position}.
            </p>
            <div class="s-toolbar">
              <label class="sr-only" for="ev-type">Event type</label>
              <select id="ev-type" value={type()} onChange={(e) => setType(e.currentTarget.value)}>
                <option value="">all types</option>
                <For each={types()}>{([t, n]) => <option value={t}>{t} ({n})</option>}</For>
              </select>
              <input placeholder="stream contains… (game-, presence-)" value={stream()} onInput={(e) => setStream(e.currentTarget.value)} aria-label="Stream filter" />
              <label class="check"><input type="checkbox" checked={hideFrames()} onChange={(e) => setHideFrames(e.currentTarget.checked)} /> hide frames</label>
              <button class="s-btn" style={{ "margin-left": "auto" }} onClick={() => { if (!paused()) setFrozen([...live.state.tail]); setPaused(!paused()); }}>{paused() ? "resume" : "pause"}</button>
              <button class="s-btn" onClick={() => void loadOlder()} disabled={loadingOlder()}>{loadingOlder() ? "loading…" : "load older"}</button>
            </div>
            <div class="s-events">
              <div class="s-events-head"><span>#</span><span>Type</span><span>Stream</span><span>Summary</span><span>Time</span><span /></div>
              <ol class="event-list">
                <For each={rows()} fallback={<li class="s-empty">Nothing matches. The log is listening…</li>}>
                  {(e) => (
                    <li class={`ev ev--${CATEGORY[e.type] ?? "other"}`} classList={{ "ev--open": open() === e.position }}>
                      <button class="ev__row" onClick={() => setOpen(open() === e.position ? null : e.position)}>
                        <span class="ev__pos">{e.position}</span>
                        <span class="ev__type">{e.type}</span>
                        <span class="ev__stream">{e.stream}<span class="muted">@{e.version}</span></span>
                        <span class="ev__summary">{summary(e)}</span>
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
    </>
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
    case "GameStarted": {
      const s = (k: string) => (typeof d[k] === "string" ? (d[k] as string) : null);
      const players = Array.isArray(d.players) ? d.players : [];
      const type = gameTypeLine({
        mode: s("mode") ?? "", kind: s("kind"), playlist: s("playlist"), online: d.online === true, players: { ...players },
        variant: s("variant"), difficulty: s("difficulty"),
      });
      return `${gameMapTitle({ map: d.map as string, map_id: typeof d.map_id === "string" ? d.map_id : null })} · ${type} · ${players.length} ${players.length === 1 ? "player" : "players"}`;
    }
    case "PlayerKilled": return `${d.killer ?? "world"} → ${d.victim} (${d.kind}${d.weapon ? ", " + d.weapon : ""})`;
    case "ScoreChanged": return `tick ${d.tick}`;
    case "GameFrame": {
      const f = recordedFrame(d);
      return f ? `tick ${f.tick}, ${f.players.length} players` : `tick ${d.tick}`;
    }
    case "GameEnded": {
      const w = d.winner as { team?: number; player?: number } | null | undefined;
      const who = w ? (w.team !== undefined ? `team ${w.team} won` : `slot ${w.player} won`) : "no winner";
      return `${d.reason}, ${who}`;
    }
    default: return "";
  }
}
