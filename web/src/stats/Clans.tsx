import { gameMapTitle } from "../state/maps";
import { For, Show, createMemo, createSignal } from "solid-js";
import { A, useParams } from "@solidjs/router";
import { live } from "../state/live";
import { social } from "../state/social";
import { teamRecord } from "../state/social-model";
import { OUTCOME_WORDS, campaignOutcome, rankIndex, serviceRecord } from "../state/derive";
import RankIcon from "../components/RankIcon";
import { gameTypeLine } from "../state/gametype";
import { ROLE_LABEL, leaderOf, roleOf, sortedMembers, type Team, type TeamMember } from "../api/social";
import Emblem from "../components/Emblem";
import MedalIcon from "../components/MedalIcon";
import { Crumbs, Derived, Strip, Title, playerHref } from "./Layout";
import { PeopleFinder, Pills, fireteam, gameTitle } from "./Side";
import { ClanEmblem } from "./clanBits";
import { num, ratio, usDate } from "./fmt";
import "../styles/social.css";

import { clanHref } from "./modes";

export default function Clans() {
  const [q, setQ] = createSignal("");
  const all = createMemo(() => social.teams().slice().sort((a, b) => a.properties.name.localeCompare(b.properties.name)));
  const shown = createMemo(() => {
    const k = q().trim().toLowerCase();
    return all().filter((t) => !k || t.properties.name.toLowerCase().includes(k) || t.properties.motto.toLowerCase().includes(k));
  });
  return (
    <>
      <Crumbs trail={[["Home", "/stats"], ["Clans"]]} />
      <Strip kind="players" />
      <div class="s-cols s-cols--over">
        <aside class="s-left">
          <Title>Clans</Title>
          <Pills label="Clans" items={[
            { label: `Clan Directory (${all().length})`, href: "#directory" },
            { label: "Manage Xbox Live Teams", href: "/dash/teams" },
            { label: "Players & Leaderboards", href: "/stats/players" },
          ]} />
          <PeopleFinder />
        </aside>
        <div class="s-right">
          <Title class="s-title--cap" help="Clans are kept by this server; anyone can read them. Membership is signed events on each clan's stream (the protocol calls them teams).">Clan Directory</Title>
          <div id="directory" class="s-matte" style={{ padding: "8px" }}>
            <form class="s-toolbar" onSubmit={(e) => e.preventDefault()}>
              <label for="s-team-q">find a clan</label>
              <input id="s-team-q" value={q()} onInput={(e) => setQ(e.currentTarget.value)} />
              <span class="s-toolbar-note">{shown().length} of {all().length}</span>
            </form>
            <div class="s-grid-wrap">
              <table class="s-grid s-grid--roomy">
                <thead><tr><th>Clan</th><th class="hide-s">Motto</th><th>Leader</th><th class="num">Members</th><th class="num hide-s">Founded</th></tr></thead>
                <tbody>
                  <For each={shown()} fallback={<tr><td colspan="5" class="s-dim">{social.teamsError() ? `Couldn't read clans: ${social.teamsError()}` : !social.teamsLoaded() ? "Loading clans…" : all().length ? "No clan matches that." : "No clans on this server yet."}</td></tr>}>
                    {(t) => {
                      const lead = () => leaderOf(t);
                      return (
                        <tr>
                          <td><span class="s-who"><ClanEmblem team={t} size={16} /><A href={clanHref(t.id)}>{t.properties.name}</A></span></td>
                          <td class="hide-s s-soc-motto">{t.properties.motto}</td>
                          <td><Show when={lead()} fallback={<span class="s-dim">none</span>}>{(l) => <A href={playerHref(l().identity.gamertag)}>{l().identity.gamertag}</A>}</Show></td>
                          <td class="num">{Object.keys(t.members).length}</td>
                          <td class="num hide-s">{usDate(t.created_at)}</td>
                        </tr>
                      );
                    }}
                  </For>
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </div>
    </>
  );
}

export function ClanPage() {
  const params = useParams<{ id: string }>();
  const id = () => params.id.toLowerCase();
  const team = () => social.team(id());
  return (
    <Show when={team()} fallback={
      <>
        <Crumbs trail={[["Home", "/stats"], ["Clans", "/stats/clans"], [id()]]} />
        <Show when={social.teamsLoaded()} fallback={<div class="s-loading">Loading clan…</div>}>
          <div class="s-full">
            <Title class="s-title--bar">Clan Not Found</Title>
            <p class="s-pad">There's no clan {id()} on this server. It may have been disbanded. <A href="/stats/clans">Browse clans</A></p>
          </div>
        </Show>
      </>
    }>
      {(t) => <ClanBody team={t()} />}
    </Show>
  );
}

function ClanBody(props: { team: Team }) {
  const t = () => props.team;
  const rec = createMemo(() => teamRecord(live.state, Object.keys(t().members)));
  const members = createMemo(() => sortedMembers(t()));
  const medals = createMemo(() => [...rec().medals].sort((a, b) => b[1] - a[1]));
  const lead = () => leaderOf(t());
  return (
    <>
      <Crumbs trail={[["Home", "/stats"], ["Clans", "/stats/clans"], [t().properties.name]]} />
      <div class="s-id s-team-id">
        <span class="s-id-emblem s-team-emblem"><ClanEmblem team={t()} size={68} /></span>
        <div class="s-id-name s-team-name">
          <h2>{t().properties.name}</h2>
          <div class="s-soc-motto">{t().properties.motto || "No motto yet."}</div>
          <div>Led by <Show when={lead()} fallback="nobody">{(l) => <A href={playerHref(l().identity.gamertag)}>{l().identity.gamertag}</A>}</Show> · Founded {usDate(t().created_at)}</div>
        </div>
        <div class="s-id-nums">
          <dl>
            <dt>Members:</dt><dd>{members().length} of {t().max_members}</dd>
            <dt>Officers:</dt><dd>{members().filter((m) => roleOf(t(), m.identity.xuid) === "officer").length}</dd>
            <dt>Games Together:</dt><dd><Derived note="Games two or more members played on the same side, from the games this page has loaded; derived in your browser.">{num(rec().together)}</Derived></dd>
          </dl>
        </div>
      </div>
      <div class="s-cols">
        <aside class="s-left">
          <Title>Clan Record</Title>
          <Pills label="Clan" items={[
            { label: `Roster (${members().length})`, href: "#roster" },
            { label: "Clan Service Record", href: "#combined" },
            { label: `Games Together (${rec().together})`, href: "#games" },
            { label: "Clan Directory", href: "/stats/clans" },
            { label: "Manage Xbox Live Team", href: `/dash/teams/${t().id}` },
          ]} />
          <PeopleFinder />
        </aside>
        <div class="s-right">
          <Title class="s-title--rule">About the Clan</Title>
          <div class="s-matte s-soc-about">
            <p>{t().properties.description || "The leader hasn't written a description yet."}</p>
            <Show when={safeUrl(t().properties.url)}>{(u) => <p><a href={u()} target="_blank" rel="noopener noreferrer nofollow">{t().properties.url}</a></p>}</Show>
          </div>
          <Title class="s-title--tabs">Roster</Title>
          <div id="roster" class="s-matte" style={{ padding: "8px" }}>
            <div class="s-grid-wrap">
              <table class="s-grid s-grid--roomy">
                <thead><tr><th>Player</th><th>Role</th><th class="hide-s">Rank <Derived>{""}</Derived></th><th class="num">K/D</th><th class="num hide-s">Joined</th></tr></thead>
                <tbody>
                  <For each={members()}>{(m) => <RosterRow team={t()} m={m} />}</For>
                </tbody>
              </table>
            </div>
          </div>
          <Title class="s-title--rule">Clan Service Record</Title>
          <div id="combined" class="s-matte s-soc-combined">
            <dl class="s-facts">
              <div><dt>Games Together</dt><dd>{num(rec().together)}</dd></div>
              <div><dt>Wins</dt><dd>{num(rec().wins)}</dd></div>
              <div><dt>Win Rate</dt><dd>{rec().together ? `${Math.round((100 * rec().wins) / rec().together)}%` : "–"}</dd></div>
              <div><dt>Campaign Together</dt><dd>{num(rec().campaign.length)}</dd></div>
            </dl>
            <dl class="s-facts">
              <div><dt>Kills</dt><dd>{num(rec().kills)}</dd></div>
              <div><dt>Deaths</dt><dd>{num(rec().deaths)}</dd></div>
              <div><dt>K/D Ratio</dt><dd>{ratio(rec().kills, rec().deaths)}</dd></div>
            </dl>
            <div class="s-soc-medals">
              <h4 class="s-soc-sub">Medals Earned Together</h4>
              <div class="s-preview-medals">
                <For each={medals()} fallback={<span class="s-dim">No recorded medals in finished shared games.</span>}>
                  {([id, count]) => <span><MedalIcon id={id} size={28} /><small>{num(count)}</small></span>}
                </For>
              </div>
            </div>
            <p class="s-soc-foot"><Derived note="Counted from the loaded multiplayer games where at least two current members played together. Medals come from recorded kills in finished games. Campaign games count separately.">Derived from loaded shared games</Derived></p>
          </div>
          <Title class="s-title--tabs">Recent Games Together</Title>
          <div id="games" class="s-matte" style={{ padding: "8px" }}>
            <div class="s-grid-wrap">
              <table class="s-grid s-grid--roomy">
                <thead><tr><th>Game</th><th class="hide-s">Date</th><th>Members</th><th class="num">Kills</th><th class="num">Deaths</th><th>Result</th></tr></thead>
                <tbody>
                  <For each={rec().games.slice(0, 15)} fallback={<tr><td colspan="6" class="s-dim">No games with two members on the same side yet.</td></tr>}>
                    {(g) => (
                      <tr>
                        <td><A href={`/stats/games/${g.game.game_id}`}>{gameTitle(g.game)} on {gameMapTitle(g.game)}</A></td>
                        <td class="hide-s">{g.game.status === "live" ? "playing now" : usDate(g.game.started_at)}</td>
                        <td>{g.slots.map((i) => g.game.players[i]?.name).filter(Boolean).join(", ")}</td>
                        <td class="num">{g.kills}</td>
                        <td class="num">{g.deaths}</td>
                        <td>{g.game.status === "live" ? <span class="s-soc-live">Live</span> : g.won ? <b class="s-soc-win">Won</b> : "Lost"}</td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
          </div>
          <Show when={rec().campaign.length}>
            <Title class="s-title--tabs">Campaign Together</Title>
            <div class="s-matte" style={{ padding: "8px" }}>
              <div class="s-grid-wrap">
                <table class="s-grid s-grid--roomy">
                  <thead><tr><th>Level</th><th class="hide-s">Date</th><th>Players</th><th>Result</th></tr></thead>
                  <tbody>
                    <For each={rec().campaign.slice(0, 10)}>
                      {(g) => (
                        <tr>
                          <td><A href={`/stats/games/${g.game_id}`}>{gameMapTitle(g)}</A> <span class="s-dim">{gameTypeLine(g)}</span></td>
                          <td class="hide-s">{g.status === "live" ? "playing now" : usDate(g.started_at)}</td>
                          <td>{fireteam(g)}</td>
                          <td>{g.status === "live" ? <span class="s-soc-live">Live</span> : OUTCOME_WORDS[campaignOutcome(g)]}</td>
                        </tr>
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </div>
          </Show>
        </div>
      </div>
    </>
  );
}

/** Only http(s) links are shown as links. */
export function safeUrl(u: string): string | null {
  if (!u.trim()) return null;
  try {
    const x = new URL(/^[a-z][a-z0-9+.-]*:/i.test(u) ? u : `https://${u}`);
    return x.protocol === "http:" || x.protocol === "https:" ? x.toString() : null;
  } catch { return null; }
}

function RosterRow(props: { team: Team; m: TeamMember }) {
  const x = () => props.m.identity.xuid;
  const role = () => roleOf(props.team, x()) ?? "member";
  const rec = createMemo(() => serviceRecord(live.state, x()));
  return (
    <tr>
      <td><span class="s-who"><Emblem seed={x()} size={16} /><A href={playerHref(props.m.identity.gamertag)}>{props.m.identity.gamertag}</A></span></td>
      <td><span class={`s-soc-role s-soc-role--${role()}`}>{ROLE_LABEL[role()]}</span></td>
      <td class="hide-s"><span class="s-who"><RankIcon rank={rankIndex(rec().exp)} exp={rec().exp} size={16} />{rec().rank}</span></td>
      <td class="num">{rec().games ? ratio(rec().kills, rec().deaths) : "–"}</td>
      <td class="num hide-s">{usDate(props.m.joined_at)}</td>
    </tr>
  );
}
