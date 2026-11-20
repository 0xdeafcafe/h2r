import { A, useSearchParams } from "@solidjs/router";
import { For, Show, batch, createMemo, createResource, createSignal } from "solid-js";
import { readFilms, readPlayerClips, setFilmSaved, type FilmSummary, type Clip } from "../api/films";
import MapPicture from "../components/MapPicture";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { gameMapTitle } from "../state/maps";
import { Tabs } from "../stats/Layout";
import { filmCard } from "./cards";
import "./theater.css";

const tabs = [
  { id: "recent", label: "Recent" },
  { id: "saved", label: "Saved" },
  { id: "clips", label: "Clips" },
];

type Tab = "recent" | "saved" | "clips";
const noFilms: Record<Tab, string> = {
  recent: "No recent films.",
  saved: "No saved films.",
  clips: "No clips yet.",
};

interface Card {
  film: FilmSummary;
  clip?: Clip;
}

type Loaded = { items: Card[] } | { error: string };

export default function PlayerFilms(props: { xuid: string }) {
  const [params, setParams] = useSearchParams<{ tab?: string }>();
  const tab = (): Tab => params.tab === "saved" || params.tab === "clips" ? params.tab : "recent";
  const source = createMemo(() => ({
    api: live.server().api, player: props.xuid, reader: identity.me()?.xuid, ready: live.loadMode() !== "snapshot",
  }));
  return (
    <Show when={source().ready && source()} keyed fallback={<div class="s-loading">Loading films…</div>}>
      {(account) => <Library player={account.player} api={account.api} tab={tab()} select={(id) => setParams({ tab: id === "recent" ? undefined : id }, { replace: true, scroll: false })} />}
    </Show>
  );
}

function Library(props: { player: string; api: string; tab: Tab; select: (id: string) => void }) {
  const own = () => identity.me()?.xuid.toLowerCase() === props.player.toLowerCase();
  const [pending, setPending] = createSignal<Set<string>>(new Set());
  const [actionError, setActionError] = createSignal("");

  async function load<T>(read: () => Promise<T[]>, card: (item: T) => Card): Promise<Loaded> {
    try {
      return { items: (await read()).map(card) };
    } catch (e) {
      return { error: e instanceof Error ? e.message : String(e) };
    }
  }

  const reader = live.api().readToken;
  const [recent, refreshRecent] = createResource(
    () => props.tab === "recent",
    () => load(() => readFilms(props.api, reader, props.player, "recent"), (film) => ({ film })),
  );
  // On your Recent tab the Saved list also tells each button its state.
  const [saved, refreshSaved] = createResource(
    () => props.tab === "saved" || own(),
    () => load(() => readFilms(props.api, reader, props.player, "saved"), (film) => ({ film })),
  );
  const [clips, refreshClips] = createResource(
    () => props.tab === "clips",
    () => load(() => readPlayerClips(props.api, reader, props.player), ({ clip, film }) => ({ clip, film })),
  );
  const resource = () => props.tab === "clips" ? clips : props.tab === "saved" ? saved : recent;
  const result = () => resource()();
  const items = () => {
    const data = result();
    if (!data || !("items" in data)) return [];
    return data.items;
  };
  const savedIds = createMemo(() => {
    const data = saved();
    return new Set(data && "items" in data ? data.items.map(({ film }) => film.game_id) : []);
  });
  const savedError = () => {
    const data = saved();
    return data && "error" in data ? data.error : undefined;
  };
  const isSaved = (id: string) => savedIds().has(id);

  async function toggle(film: FilmSummary) {
    const id = film.game_id;
    const me = identity.me(), wk = live.wellKnown();
    if (!own() || !me || !wk || pending().has(id)) return;
    const next = !isSaved(id);
    setActionError("");
    setPending((ids) => new Set(ids).add(id));
    try {
      await setFilmSaved({ api: props.api, aud: wk.id, me }, id, next);
      refreshSaved.mutate((data) => {
        if (!data || !("items" in data)) return data;
        const items = data.items.filter((row) => row.film.game_id !== id);
        if (next) items.unshift({ film });
        return { items };
      });
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending((ids) => {
        const rest = new Set(ids);
        rest.delete(id);
        return rest;
      });
    }
  }

  const select = (id: string) => batch(() => { setActionError(""); props.select(id); });
  const retry = () => props.tab === "clips" ? refreshClips.refetch() : props.tab === "saved" ? refreshSaved.refetch() : refreshRecent.refetch();
  return (
    <>
      <Tabs tabs={tabs} label="Films" active={props.tab} onPick={select} small />
      <Show when={actionError()}><p class="s-pad s-bad" role="alert">Couldn't update saved films. {actionError()}</p></Show>
      <Show when={own() && props.tab === "recent" && savedError()}>
        <p class="s-pad s-bad" role="alert">Couldn't read saved films. <button type="button" class="s-btn" onClick={() => void refreshSaved.refetch()}>Retry</button></p>
      </Show>
      <Show when={!resource().loading && result()} fallback={<div class="s-loading">Loading films…</div>}>
        {(loaded) => (
          <Show when={"items" in loaded()} fallback={
            <p class="s-pad s-bad" role="alert">Couldn't load {props.tab === "clips" ? "clips" : "films"}. {(loaded() as { error: string }).error}{" "}
              <button type="button" class="s-btn" onClick={() => void retry()}>Retry</button>
            </p>
          }>
            <Show when={items().length} fallback={<p class="s-empty">{noFilms[props.tab]}</p>}>
              <ul class="film-library">
                <For each={items()}>{({ film, clip }) => {
                  const mapId = () => live.state.games[film.game_id]?.map_id;
                  const card = createMemo(() => filmCard(film, gameMapTitle({ map: film.map, map_id: mapId() }), clip));
                  return (
                    <li class="film-card">
                      <A href={card().href} class="film-card-link">
                        <MapPicture map={film.map} mapId={mapId()} class="film-card-image" fallback={<span class="film-card-image film-card-image--empty" aria-hidden="true">{card().mapName}</span>} />
                        <div class="film-card-body">
                          <strong>{card().title}</strong>
                          <span>{card().gameType}{clip ? ` on ${card().mapName}` : ""}</span>
                          <span>{card().result} · {card().length}</span>
                          <Show when={card().date}>{(date) => <time datetime={film.started_at}>{date()}</time>}</Show>
                        </div>
                      </A>
                      <Show when={own() && !clip}>
                        <div class="film-card-actions">
                          <button class="s-btn" type="button"
                            disabled={pending().has(film.game_id) || !live.wellKnown() || saved.loading || !saved() || !!savedError()}
                            onClick={() => void toggle(film)}>{pending().has(film.game_id) ? "Saving…" : isSaved(film.game_id) ? "Unsave" : "Save"}</button>
                        </div>
                      </Show>
                    </li>
                  );
                }}</For>
              </ul>
            </Show>
          </Show>
        )}
      </Show>
    </>
  );
}
