import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { ago } from "../lib/format";
import { now } from "../lib/now";
import { serviceRecord } from "../state/derive";
import { gameTypeLine } from "../state/gametype";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { gameMapTitle } from "../state/maps";
import { endedGames } from "../state/reducer";
import { ListPanel } from "./Dashboard";
import { BigPod, Plate, Prompts } from "./parts";
import type { Action } from "./input";
import "../styles/dash-games.css";

const titles = [{ key: "halo2", label: "Halo 2" }];

export default function GamesScreen(props: {
  back: () => void;
  bind: (fn: ((action: Action) => boolean) | null) => void;
}) {
  let stats!: HTMLAnchorElement;
  let library!: HTMLDivElement;
  const [details, setDetails] = createSignal(false);
  const select = () => details() ? stats.click() : stats.focus();
  const back = () => {
    if (details()) library.querySelector<HTMLElement>('[role="listbox"]')?.focus();
    else props.back();
  };
  onMount(() => props.bind(action => {
    switch (action) {
      case "right": stats.focus(); return true;
      case "select": select(); return true;
      case "left": case "back": back(); return true;
      default: return false;
    }
  }));
  onCleanup(() => props.bind(null));

  return (
    <div class="dx-screen dx-list dx-pages dx-games">
      <div class="dx-preview" aria-hidden="true"><BigPod><i class="dx-game-disc" /></BigPod></div>
      <div ref={library} onFocusIn={() => setDetails(false)}>
        <ListPanel title="Games" rows={titles} sel={0} onSel={() => {}} onAct={() => stats.focus()} empty="" hint="→ details" />
      </div>
      <Halo2Plate linkRef={link => { stats = link; }} onFocus={() => setDetails(true)} />
      <Prompts onB={back} onA={select} aLabel={details() ? "view stats" : "details"} />
    </div>
  );
}

function Halo2Plate(props: { linkRef: (link: HTMLAnchorElement) => void; onFocus: () => void }) {
  const recent = createMemo(() => endedGames(live.state, 3));
  return (
    <Plate small="game details" title="Halo 2">
      <h3>Recent games</h3>
      <Show when={recent().length} fallback={<p>No finished games yet.</p>}>
        <ul class="dx-game-history">
          <For each={recent()}>{game =>
            <li>
              <div><strong>{gameMapTitle(game)}</strong><small>{gameTypeLine(game)}</small></div>
              <time datetime={game.ended_at ?? undefined}>{ago(game.ended_at, now())}</time>
            </li>
          }</For>
        </ul>
      </Show>
      <Show when={identity.me()}>{me => {
        const record = createMemo(() => serviceRecord(live.state, me().xuid));
        return <>
          <h3>Service record <span>{me().gamertag}</span></h3>
          <dl class="dx-game-record">
            <div><dt>Played</dt><dd>{record().games}</dd></div>
            <div><dt>Wins</dt><dd>{record().wins}</dd></div>
            <div><dt>Kills</dt><dd>{record().kills}</dd></div>
          </dl>
        </>;
      }}</Show>
      <a ref={props.linkRef} onFocus={props.onFocus} class="dx-chip dx-chip--link dx-game-stats" href="/stats">View stats</a>
    </Plate>
  );
}
