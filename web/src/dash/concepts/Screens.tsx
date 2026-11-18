import { For, Match, Show, Switch, createSignal, type JSX } from "solid-js";
import { Icon, Mark, RelayLandscape } from "./Art";
import { capacity, history, messages, online, peak, players, roster, sampleTime, sessions, traffic, type Screen, type Section } from "./model";

export function Panel(props: { title: string; meta?: string; children: JSX.Element; class?: string }) {
  return <section class={`cc-panel ${props.class ?? ""}`}>
    <div class="cc-face">
      <header class="cc-panel-head"><span class="cc-twin" aria-hidden="true"/><h2>{props.title}</h2><small>{props.meta}</small></header>
      {props.children}
    </div>
  </section>;
}

export function Presence(props: { state: string }) {
  return <span class="cc-presence" data-state={props.state}><i/>{props.state}</span>;
}

export function Bulletin() {
  return <Panel class="cc-bulletin" title="Message of the day" meta="18 NOV">
    <RelayLandscape/>
    <div class="cc-bulletin-copy"><h3>Night shift doubles</h3><p>The relay opens for two-player teams at 20:00 UTC. Meet in the Slipway lobby.</p></div>
  </Panel>;
}

export function Preview(props: { selected: Section; tag: string; hint: string }) {
  const previews = {
    friends: { title: "Friends online", meta: online.length + " / " + roster.length, rows: online.map(f => [f.tag, f.state]) },
    messages: { title: "Inbox", meta: "02", rows: messages.map(m => [m.from, m.time]) },
    sessions: { title: "Open sessions", meta: "04", rows: sessions.slice(0,3).map(s => [s.map, s.slots]) },
    account: { title: "Live ID", meta: "LOCAL", rows: [["Relay", "Northline"], ["Access", "Player"]] },
    server: { title: "Relay status", meta: "ONLINE", rows: [["Players", players+" / "+capacity], ["Peak today", String(peak)], ["Latency", "24 ms"]] },
  };
  const card = () => previews[props.selected];
  return <Panel class="cc-preview" title={card().title} meta={card().meta}>
    <Show when={props.selected === "account"}><div class="cc-id-preview">{props.tag}</div></Show>
    <ul><For each={card().rows}>{row => <li><span>{row[0]}</span><small>{row[1]}</small></li>}</For></ul><p class="cc-help">{props.hint}</p>
  </Panel>;
}

export function Counters() {
  return <dl class="cc-counters">
    <For each={[[ "messages", "Invitations", 1 ], [ "friends", "Friends online", online.length ], [ "account", "Friend requests", 2 ]] as const}>{([icon,label,n]) =>
      <div><Icon name={icon}/><dd>{n}</dd><dt>{label}</dt></div>
    }</For>
  </dl>;
}

function Friends() {
  const [selected, set] = createSignal(roster[0]!);
  return <div class="cc-split">
    <Panel title="Friends list" meta={online.length+" ONLINE"}>
      <div class="cc-list"><For each={roster}>{(friend,i) =>
        <button data-nav data-initial={i() === 0 ? "" : undefined} aria-pressed={selected() === friend} onClick={() => set(friend)}>
          <b>{friend.tag}</b><Presence state={friend.state}/></button>
      }</For></div>
    </Panel>
    <Panel title="Presence" meta="LIVE"><div class="cc-info" aria-live="polite"><div class="cc-medallion"><Icon name="friends"/></div><h3>{selected().tag}</h3><p>{selected().location}</p></div></Panel>
  </div>;
}

function Messages() {
  const [selected, set] = createSignal(messages[0]!);
  return <div class="cc-split">
    <Panel title="Inbox" meta="02"><div class="cc-list"><For each={messages}>{(m,i) =>
      <button data-nav data-initial={i() === 0 ? "" : undefined} aria-pressed={selected() === m} onClick={() => set(m)}><b>{m.from}</b><small>{m.time}</small></button>
    }</For></div></Panel>
    <Panel title="Transmission"><div class="cc-info" aria-live="polite"><h3>{selected().subject}</h3><p>{selected().body}</p></div></Panel>
  </div>;
}

function Sessions() {
  const [selected, set] = createSignal(sessions[0]!);
  return <div class="cc-split">
    <Panel title="Session browser" meta="04 OPEN"><div class="cc-list"><For each={sessions}>{(s,i) =>
      <button data-nav data-initial={i() === 0 ? "" : undefined} aria-pressed={selected() === s} onClick={() => set(s)}><b>{s.map}</b><small>{s.slots}</small></button>
    }</For></div></Panel>
    <Panel title="Lobby details"><div class="cc-info" aria-live="polite"><div class="cc-medallion"><Icon name="sessions"/></div><h3>{selected().mode}</h3><p>Hosted by {selected().host}<br/>{selected().slots} players</p></div></Panel>
  </div>;
}

function Account(props: { tag: string; save: (tag: string) => void }) {
  const [draft, setDraft] = createSignal(props.tag);
  const [notice, setNotice] = createSignal("");
  function submit(e: SubmitEvent) {
    e.preventDefault();
    const name = draft().trim();
    if (!name) { setNotice("Enter a gamertag."); return; }
    props.save(name);
    setDraft(name);
    setNotice("Live ID saved for this preview.");
  }
  return <div class="cc-split">
    <Panel title="Live ID" meta="PLAYER PROFILE">
      <form class="cc-form" onSubmit={submit}><label for="cc-tag">Gamertag</label>
        <input data-nav data-initial id="cc-tag" aria-describedby="cc-field-help" maxlength="16" required autocomplete="off" spellcheck={false} value={draft()} onInput={e => { setDraft(e.currentTarget.value); setNotice(""); }}/>
        <p id="cc-field-help">Up to 16 characters. Visible to friends on Northline.</p>
        <button data-nav class="cc-key">Save Live ID</button><p class="cc-notice" role="status">{notice()}</p>
      </form>
    </Panel>
    <Panel title="Identity card" meta="NL / 017"><div class="cc-info"><div class="cc-medallion"><Mark/></div><h3>{props.tag}</h3><p>Northline relay · Local account</p></div></Panel>
  </div>;
}

function Metrics() {
  return <dl class="cc-metrics"><For each={[[ "Players online", String(players), "/ "+capacity ],[ "Open sessions", String(sessions.length).padStart(2,"0"), "lobbies" ],[ "Relay latency", "24", "ms" ]]}>
    {([title, value, unit]) => <div><dt>{title}</dt><dd>{value} <small>{unit}</small></dd></div>}
  </For></dl>;
}

function Traffic() {
  const [asTable, set] = createSignal(false);
  const x = (i: number) => 4 + i/(traffic.length-1)*592;
  const y = (n: number) => 83 - n/capacity*78;
  const line = traffic.map((v,i) => x(i).toFixed(1)+","+y(v).toFixed(1)).join(" ");
  return <Panel class="cc-traffic" title="Concurrent players" meta={"PEAK "+peak}>
    <div class="cc-chart-head"><span>Northline · 00:00–18:00 UTC</span><button data-nav data-initial class="cc-key" aria-pressed={asTable()} onClick={() => set(!asTable())}>{asTable() ? "Show chart" : "Show values"}</button></div>
    <Show when={!asTable()} fallback={<div class="cc-samples" data-nav data-scroll tabindex="0" role="region" aria-label="Player samples by time"><table><caption class="sr-only">Concurrent players by time</caption><thead><tr><th>UTC</th><th>Players</th></tr></thead><tbody><For each={traffic}>{(v,i) => <tr><td>{sampleTime(i())}</td><td>{v}</td></tr>}</For></tbody></table></div>}>
      <div class="cc-plot"><div class="cc-y"><span>64</span><span>32</span><span>0</span></div><svg viewBox="0 0 600 86" preserveAspectRatio="none" role="img" aria-label={`Concurrent players, peaking at ${peak} at 15:30 UTC and ending at ${players}.`}>
        <defs><linearGradient id="cc-area" x2="0" y2="1"><stop stop-color="#b6f43e" stop-opacity=".54"/><stop offset="1" stop-color="#9ac73a" stop-opacity=".04"/></linearGradient></defs>
        <path d="M4 5h592M4 44h592M4 83h592M4 5v78m148-78v78m148-78v78m148-78v78m148-78v78" fill="none" stroke="#56713e" stroke-dasharray="2 4"/>
        <polygon points={"4,83 "+line+" 596,83"} fill="url(#cc-area)"/><polyline points={line} fill="none" stroke="#c7f76e" stroke-width="2.4" vector-effect="non-scaling-stroke"/>
      </svg></div><div class="cc-times"><span>00:00</span><span>06:00</span><span>12:00</span><span>18:00</span></div>
    </Show>
  </Panel>;
}

export function Server() {
  return <div class="cc-server"><Metrics/><Traffic/>
    <Panel class="cc-history" title="Game history" meta="LATEST 06"><div class="cc-table-scroll" data-nav data-scroll tabindex="0" role="region" aria-label="Game history"><table>
      <caption class="sr-only">Six most recent completed games on Northline</caption>
      <thead><tr><th>UTC</th><th>Map</th><th>Playlist</th><th>Players</th><th>Duration</th></tr></thead>
      <tbody><For each={history}>{row => <tr><For each={row}>{value => <td>{value}</td>}</For></tr>}</For></tbody>
    </table></div></Panel>
  </div>;
}

export function ScreenContent(props: { screen: Screen; tag: string; save: (value: string) => void }) {
  return <Switch>
    <Match when={props.screen === "friends"}><Friends/></Match>
    <Match when={props.screen === "messages"}><Messages/></Match>
    <Match when={props.screen === "sessions"}><Sessions/></Match>
    <Match when={props.screen === "account"}><Account tag={props.tag} save={props.save}/></Match>
    <Match when={props.screen === "server" || props.screen === "home"}><Server/></Match>
  </Switch>;
}
