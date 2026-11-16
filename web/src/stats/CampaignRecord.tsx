// A player's campaign service record (/stats/players/:tag/campaign): the
// other tab of the service record. Levels played, how far they got, time
// played, deaths, co-op partners. Nothing here counts towards multiplayer.

import { For, Show, createMemo } from "solid-js";
import { A } from "@solidjs/router";
import { live } from "../state/live";
import { gameMapGroupLabel, gameMapTitle } from "../state/maps";
import { OUTCOME_WORDS, campaignHistory, campaignRecord } from "../state/derive";
import { gameTypeLine } from "../state/gametype";
import Emblem from "../components/Emblem";
import SkullIcon from "../components/SkullIcon";
import { skullName, skullTitle } from "../lib/skulls";
import { now } from "../lib/now";
import { Derived, Title, playerHref } from "./Layout";
import { MapPreview, MapThumb, PeopleFinder, Pills, RecentGames } from "./Side";
import { ModeTabs, PlayedWith, PlayerFrame } from "./Player";
import { hms, num, usDate } from "./fmt";

export default function CampaignRecordPage() {
  return (
    <PlayerFrame which="campaign">
      {(a) => {
        const base = () => playerHref(a().gamertag);
        const rec = createMemo(() => campaignRecord(live.state, a().xuid, now()));
        const rows = createMemo(() => campaignHistory(live.state, a().xuid, now()));
        const latest = () => rows()[0];
        return (
          <div class="s-cols">
            <aside class="s-left">
              <Title>h2 live Service Record</Title>
              <Pills label="Campaign record" items={[
                { label: "Multiplayer Record", href: base() },
                { label: `Campaign History (${rec().games})`, href: `${base()}/history?tab=campaign` },
                { label: "Players & Leaderboards", href: "/stats/players" },
              ]} />
              <RecentGames games={rows().slice(0, 12).map((r) => r.game)} title="Recent Campaign Games" empty="No campaign games on record yet." />
              <PlayedWith account={a()} mode="campaign" />
              <PeopleFinder />
            </aside>
            <div class="s-right">
              <ModeTabs gamertag={a().gamertag} mode="campaign" />
              <div class="s-capbar s-capbar--slim">
                <Show when={latest()} fallback={<span class="s-dim">Latest campaign game</span>}>
                  {(r) => (
                    <>
                      <A href={`/stats/games/${r().game.game_id}`}>{gameMapTitle(r().game)}</A>
                      <span class="s-dim">{gameTypeLine(r().game)}</span>
                      <span class="s-capbar-right">{r().outcome === "playing" ? "playing now" : `${OUTCOME_WORDS[r().outcome]}, ${usDate(r().game.started_at)}`}</span>
                    </>
                  )}
                </Show>
              </div>
              <div class="s-shot">
                <Show when={latest()} keyed fallback={<div class="s-shot-empty">No campaign games on record yet.</div>}>
                  {(r) => <MapThumb game={r.game} large />}
                </Show>
              </div>
              <Title class="s-title--rule">Campaign Snapshot</Title>
              <div class="s-snap">
                <div class="s-snap-pic"><Emblem seed={a().xuid} size={112} /></div>
                <dl class="s-facts">
                  <div><dt>Campaign Games</dt><dd>{num(rec().games)}</dd></div>
                  <div><dt>Levels Completed</dt><dd>{num(rec().completed)}</dd></div>
                  <div><dt>Not Finished</dt><dd>{num(rec().unfinished)}</dd></div>
                  <div><dt>Levels Played</dt><dd>{num(rec().levels.length)}</dd></div>
                  <div><dt>Co-op Games</dt><dd>{num(rec().coop)}</dd></div>
                  <div><dt>Time Played</dt><dd>{hms(rec().seconds)}</dd></div>
                </dl>
                <dl class="s-facts">
                  <h4>Combat Report</h4>
                  <div><dt>Deaths</dt><dd>{num(rec().deaths)}</dd></div>
                  <Show when={rec().kills !== null}>
                    <div><dt>AI Kills</dt><dd><Derived note="Kills the game's scoreboard credits in campaign games: the level's AI.">{num(rec().kills!)}</Derived></dd></div>
                  </Show>
                  <div><dt>Deaths per Game</dt><dd>{rec().games ? (rec().deaths / rec().games).toFixed(2) : "–"}</dd></div>
                </dl>
                <div class="s-snap-foot">
                  <span>First Played {rec().firstPlayed ? usDate(rec().firstPlayed) : "never"}</span>
                  <span>Last Played {rec().lastPlayed ? usDate(rec().lastPlayed) : "never"}</span>
                </div>
              </div>
              <Title class="s-title--rule">Skulls Played With</Title>
              <div class="s-skulls-played">
                <Show when={rec().skulls.length} fallback={<p class="s-dim">No skulls on in any campaign game on record.</p>}>
                  <ul class="skulls skulls--names" aria-label="Skulls played with">
                    <For each={rec().skulls}>
                      {(s) => (
                        <li title={skullTitle(s.index)}>
                          <SkullIcon index={s.index} size={22} title={false} />
                          <span>{skullName(s.index)}</span>
                          <small>{num(s.games)} {s.games === 1 ? "game" : "games"}</small>
                        </li>
                      )}
                    </For>
                  </ul>
                </Show>
                <p class="s-note">Counted from the {num(rec().games)} campaign {rec().games === 1 ? "game" : "games"} this page has loaded, the newest the server sent. Older games are not included.</p>
              </div>
              <Title class="s-title--rule">Levels</Title>
              <div class="s-grid-wrap" style={{ margin: "0 8px 8px" }}>
                <table class="s-grid s-grid--roomy">
                  <thead><tr><th>Level</th><th class="num">Played</th><th class="num">Completed</th><th class="num">Deaths</th><th class="num hide-s">Fastest</th><th class="num">Last Played</th></tr></thead>
                  <tbody>
                    <For each={rec().levels} fallback={<tr><td colspan="6" class="s-dim">No levels played yet.</td></tr>}>
                      {(l) => (
                        <tr>
                          <td><span class="s-who"><MapPreview map={l.map} mapId={l.map_id} />{gameMapGroupLabel(l)}</span></td>
                          <td class="num">{num(l.plays)}</td>
                          <td class="num">{num(l.completions)}</td>
                          <td class="num">{num(l.deaths)}</td>
                          <td class="num hide-s">{l.fastest === null ? "–" : hms(l.fastest)}</td>
                          <td class="num">{usDate(l.lastPlayed)}</td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        );
      }}
    </PlayerFrame>
  );
}
