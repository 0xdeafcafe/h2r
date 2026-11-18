import { For, Show, createMemo, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { ago } from "../lib/format";
import { now } from "../lib/now";
import { formatBytes } from "../lib/maps";
import { blocks } from "../api/cloudsaves";
import { serviceRecord } from "../state/derive";
import { gameTypeLine } from "../state/gametype";
import { gameImage } from "../state/gameart";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { gameMapTitle } from "../state/maps";
import { endedGames } from "../state/reducer";
import { createCloudSaves, type SaveRow } from "../state/cloudsaves";
import { ListPanel } from "./Dashboard";
import { BigPod, Plate, Prompts } from "./parts";
import type { Action } from "./input";
import "../styles/dash-games.css";
import { STATS_NAME } from "../brand";

/** The disc's own pictures (crates/cli/src/live_art.rs `TITLE_IMAGES`): the
 *  game's tile and the picture of a save without one of its own. Null
 *  without the server's game art; the screen then draws its own. */
export const TITLE_IMAGE = "title/image";
export const TITLE_SAVE_IMAGE = "title/save-image";
const titleImage = () => gameImage(TITLE_IMAGE)?.url ?? null;
const saveImage = () => gameImage(TITLE_SAVE_IMAGE)?.url ?? null;

/** The game's tile: the disc's title image, else `fallback` (a drawn tile). */
function TitleTile(props: { class: string; fallback?: JSX.Element }) {
  return (
    <Show when={titleImage()} fallback={props.fallback ?? <i class={`${props.class} dx-title-tile--drawn`} aria-hidden="true" />}>
      {(u) => <img class={props.class} src={u()} alt="" decoding="async" draggable={false} />}
    </Show>
  );
}

/** "18 Nov 2026" in the reader's own words. */
const day = (ms: number) => new Date(ms).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });

export default function GamesScreen(props: {
  back: () => void;
  bind: (fn: ((action: Action) => boolean) | null) => void;
}) {
  let plate!: HTMLDivElement;
  let library!: HTMLDivElement;
  const saves = createCloudSaves();
  const titles = [{ key: "halo2", label: "Halo 2", icon: <TitleTile class="dx-title-tile" /> }];
  const [focus, setFocus] = createSignal<"list" | "stats" | string>("list");
  const [armed, setArmed] = createSignal<string | null>(null);
  const stops = () => [...plate.querySelectorAll<HTMLElement>("[data-stop]")];
  const stats = () => plate.querySelector<HTMLAnchorElement>(".dx-game-stats")!;
  const move = (by: number) => {
    const all = stops(), at = all.indexOf(document.activeElement as HTMLElement);
    all[Math.max(0, Math.min(all.length - 1, (at < 0 ? all.length - 1 : at) + by))]?.focus();
  };
  const save = (dir: string) => saves.rows().find((s) => s.dir === dir);
  // The deleted row goes away under the focus: it moves to the stats link.
  const del = async (s: SaveRow) => {
    setArmed(null);
    await saves.remove(s);
    if (focus() === s.dir && !saves.rows().some((r) => r.dir === s.dir)) stats().focus();
  };
  const select = () => {
    const f = focus();
    if (f === "list") return stats().focus();
    if (f === "stats") return stats().click();
    const s = save(f);
    if (!s || saves.busy()) return;
    if (armed() === f) void del(s); else setArmed(f);
  };
  const back = () => {
    if (armed()) setArmed(null);
    else if (focus() !== "list") library.querySelector<HTMLElement>('[role="listbox"]')?.focus();
    else props.back();
  };
  onMount(() => props.bind(action => {
    switch (action) {
      case "right": if (focus() === "list") stats().focus(); return true;
      case "up": if (focus() === "list") return false; move(-1); return true;
      case "down": if (focus() === "list") return false; move(1); return true;
      case "select": select(); return true;
      case "left": case "back": back(); return true;
      default: return false;
    }
  }));
  onCleanup(() => props.bind(null));
  const aLabel = () => {
    const f = focus();
    if (f === "list") return "details";
    if (f === "stats") return `open ${STATS_NAME}`;
    return armed() === f ? "delete it" : "delete…";
  };

  return (
    <div class="dx-screen dx-list dx-pages dx-games">
      <div class="dx-preview" aria-hidden="true"><BigPod><TitleTile class="dx-game-pod-tile" fallback={<i class="dx-game-disc" />} /></BigPod></div>
      <div ref={library} onFocusIn={() => { setFocus("list"); setArmed(null); }}>
        <ListPanel title="Games" rows={titles} sel={0} onSel={() => {}} onAct={() => stats().focus()} empty="" hint="→ details" />
      </div>
      <div ref={plate}>
        <Halo2Plate
          saves={saves}
          armed={armed()}
          onFocus={(f) => { setFocus(f); if (armed() !== f) setArmed(null); }}
          onArm={(dir) => { setFocus(dir); setArmed(dir); }}
          onDelete={(s) => void del(s)}
          onKeep={() => setArmed(null)}
        />
      </div>
      <Prompts onB={back} onA={select} aLabel={aLabel()} bLabel={armed() ? "keep it" : "back"} />
    </div>
  );
}

type Saves = ReturnType<typeof createCloudSaves>;

function Halo2Plate(props: {
  saves: Saves;
  armed: string | null;
  onFocus: (f: string) => void;
  onArm: (dir: string) => void;
  onDelete: (s: SaveRow) => void;
  onKeep: () => void;
}) {
  const recent = createMemo(() => endedGames(live.state, 3));
  return (
    <Plate small="game details" title="Halo 2">
      <TitleTile class="dx-game-tile" />
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
          <div class="dx-game-record-row">
            <dl class="dx-game-record">
              <div><dt>Played</dt><dd>{record().games}</dd></div>
              <div><dt>Wins</dt><dd>{record().wins}</dd></div>
              <div><dt>Kills</dt><dd>{record().kills}</dd></div>
            </dl>
            <StatsLink onFocus={props.onFocus} />
          </div>
          <SavedGames {...props} />
        </>;
      }}</Show>
      <Show when={!identity.me()}><StatsLink onFocus={props.onFocus} /></Show>
    </Plate>
  );
}

function StatsLink(props: { onFocus: (f: string) => void }) {
  return <a data-stop onFocus={() => props.onFocus("stats")} class="dx-chip dx-chip--link dx-game-stats" href="/stats">{STATS_NAME}</a>;
}

/** The player's cloud saves for Halo 2, as the original dashboard's Memory
 *  screen listed a game's saves: picture, name, date and size, and the
 *  space they take. */
function SavedGames(props: Parameters<typeof Halo2Plate>[0]) {
  const s = props.saves;
  const used = () => s.list()?.used_bytes ?? 0;
  const quota = () => s.list()?.quota_bytes ?? 0;
  return (
    <Show when={s.state() !== "none"}>
      <h3>
        Saved games
        <span>
          <Show when={s.state() === "ready"} fallback={s.state() === "error" ? "unavailable" : "loading"}>
            {s.rows().length} {s.rows().length === 1 ? "save" : "saves"}, {formatBytes(used())}
            <Show when={quota()}> of {formatBytes(quota())}</Show>
          </Show>
        </span>
      </h3>
      <Show when={s.state() === "ready" && quota()}>
        <div class="dx-game-quota" role="meter" aria-label="cloud storage used" aria-valuemin={0} aria-valuemax={quota()} aria-valuenow={used()}>
          <i style={{ width: `${Math.min(100, (used() / Math.max(1, quota())) * 100)}%` }} />
        </div>
      </Show>
      <Show when={s.state() === "ready"}>
        <Show when={s.rows().length} fallback={<p>No saves on this server yet.</p>}>
          <ul class="dx-game-saves">
            <For each={s.rows()}>{save => {
              const on = () => props.armed === save.dir;
              const pic = () => save.picture ?? saveImage();
              return (
                <li classList={{ armed: on(), busy: s.busy() === save.dir }}>
                  <button type="button" data-stop onFocus={() => props.onFocus(save.dir)} onClick={() => props.onArm(save.dir)} aria-label={`${save.title ?? "saved game"}, ${day(save.modified)}, ${blocks(save.size)} blocks`}>
                    <Show when={pic()} fallback={<i class="dx-save-pic dx-save-pic--drawn" aria-hidden="true" />}>
                      {(u) => <img class="dx-save-pic" src={u()} alt="" decoding="async" draggable={false} />}
                    </Show>
                    <span>
                      <strong>{save.title ?? (save.meta ? "…" : save.dir)}</strong>
                      <small>{day(save.modified)}</small>
                    </span>
                    <em>{blocks(save.size)}<small>{blocks(save.size) === 1 ? "block" : "blocks"}</small></em>
                  </button>
                  <Show when={on()}>
                    <div class="dx-save-ask" role="group" aria-label="delete this save?">
                      <span>Delete this save?</span>
                      <button type="button" onClick={() => props.onDelete(save)}>delete</button>
                      <button type="button" onClick={() => props.onKeep()}>keep</button>
                    </div>
                  </Show>
                </li>
              );
            }}</For>
          </ul>
        </Show>
      </Show>
      <Show when={s.err()}><p class="dx-error" role="alert">{s.err()}</p></Show>
    </Show>
  );
}
