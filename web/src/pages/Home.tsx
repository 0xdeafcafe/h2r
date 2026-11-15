import { For, Show, createMemo } from "solid-js";
import { A } from "@solidjs/router";
import { live } from "../state/live";
import { endedGames, liveGames, standings, teamStandings } from "../state/reducer";
import GameCard from "../components/GameCard";
import { Avatar, Empty, PresenceBadge, presenceKind } from "../components/bits";
import { ago, clock, titleCase } from "../lib/format";
import { elapsedSeconds, teamName } from "../lib/game";
import { now } from "../lib/now";

export default function Home() {
  const games = createMemo(() => liveGames(live.state));
  const recent = createMemo(() => endedGames(live.state, 12));
  const online = createMemo(() =>
    Object.values(live.state.accounts)
      .filter((a) => a.online)
      .sort((a, b) => (presenceKind(a) === "playing" ? 0 : 1) - (presenceKind(b) === "playing" ? 0 : 1) || a.gamertag.localeCompare(b.gamertag)));
  const total = () => Object.keys(live.state.accounts).length;

  return (
    <div class="page home">
      <section class="hero">
        <div>
          <h1>
            <Show when={games().length} fallback="It's quiet in here.">
              {games().length} game{games().length === 1 ? "" : "s"} happening right now
            </Show>
          </h1>
          <p class="muted">
            {online().length} of {total()} player{total() === 1 ? "" : "s"} online
          </p>
        </div>
      </section>

      <section class="section">
        <h2 class="section__title">Live now</h2>
        <Show
          when={live.loadMode() !== "snapshot" || games().length}
          fallback={<div class="loading">Catching up on what you missed…</div>}
        >
          <Show
            when={games().length}
            fallback={
              <Empty title="Nobody's playing right now" mood="sleepy">
                The arenas are napping. Start a game and it'll pop up here the moment it begins.
              </Empty>
            }
          >
            <div class="card-grid">
              <For each={games()}>{(g) => <GameCard game={g} />}</For>
            </div>
          </Show>
        </Show>
      </section>

      <div class="two-col">
        <section class="section">
          <h2 class="section__title">Who's around</h2>
          <Show when={online().length} fallback={<Empty title="Everyone's offline" mood="sleepy">Sign in from the game to say hi.</Empty>}>
            <ul class="people">
              <For each={online()}>
                {(a) => (
                  <li>
                    <A href={`/players/${encodeURIComponent(a.gamertag)}`} class="person">
                      <Avatar name={a.gamertag} online />
                      <span class="person__name">{a.gamertag}</span>
                      <PresenceBadge account={a} />
                      <small class="muted">since {ago(a.presence?.since, now())}</small>
                    </A>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </section>

        <section class="section">
          <h2 class="section__title">Recently finished</h2>
          <Show when={recent().length} fallback={<Empty title="No finished games yet" mood="happy">Replays show up here when a game ends.</Empty>}>
            <ul class="recent-games">
              <For each={recent()}>
                {(g) => {
                  const top = () =>
                    g.team_play
                      ? teamStandings(g).map((t) => `${teamName(t.team)} ${t.score}`).slice(0, 2).join(" – ")
                      : (() => { const s = standings(g)[0]; return s ? `${s.name} won with ${s.score.score}` : ""; })();
                  return (
                    <li>
                      <A href={`/games/${g.game_id}`} class="recent-game">
                        <span class="recent-game__map">{titleCase(g.map)}</span>
                        <span class="muted">{titleCase(g.variant ?? g.mode)} · {clock(elapsedSeconds(g, now()))}</span>
                        <span class="recent-game__top">{top()}</span>
                        <small class="muted">{ago(g.ended_at, now())}</small>
                      </A>
                    </li>
                  );
                }}
              </For>
            </ul>
          </Show>
        </section>
      </div>
    </div>
  );
}
