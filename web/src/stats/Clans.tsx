// Clans on the stats site: the clan directory (/stats/clans) and a clan's
// page (/stats/clans/:id) with its roster, combined numbers from games its
// members played together, and the signed-in member's actions.

import { gameMapTitle } from "../state/maps";
import { For, Show, createMemo, createSignal } from "solid-js";
import { A, useNavigate, useParams } from "@solidjs/router";
import { live } from "../state/live";
import { identity } from "../state/identity";
import { social, type WriteResult } from "../state/social";
import { teamRecord } from "../state/social-model";
import { OUTCOME_WORDS, campaignOutcome, rankIndex, serviceRecord } from "../state/derive";
import RankIcon from "../components/RankIcon";
import { gameTypeLine } from "../state/gametype";
import {
  MEMBER, OFFICER, PRIV, ROLE_LABEL, TEAM_NAME_MAX, TEAM_TEXT_MAX, can, leaderOf, randomTeamId, roleOf, sortedMembers,
  teamPropsError, teamWrites, type Team, type TeamMember,
} from "../api/social";
import { gamertagError } from "../state/account";
import Emblem from "../components/Emblem";
import { PresenceBadge } from "../components/bits";
import { Crumbs, Derived, Panel, Strip, Title, playerHref } from "./Layout";
import { PeopleFinder, Pills, fireteam, gameTitle } from "./Side";
import { ClanEmblem } from "./clanBits";
import { num, ratio, usDate } from "./fmt";
import "../styles/social.css";

export { clanHref } from "./modes";
import { clanHref } from "./modes";

/** A small status line under a form or button. */
function Note(props: { r: WriteResult | null; ok?: string }) {
  return (
    <Show when={props.r}>
      {(r) => <p class={r().ok ? "s-soc-ok" : "s-soc-err"} role="status">{r().ok ? props.ok ?? "Done." : (r() as { error: string }).error}</p>}
    </Show>
  );
}

// ---------- the directory ----------

export default function Clans() {
  const [q, setQ] = createSignal("");
  const me = () => identity.me();
  const all = createMemo(() => social.teams().slice().sort((a, b) => a.properties.name.localeCompare(b.properties.name)));
  const shown = createMemo(() => {
    const k = q().trim().toLowerCase();
    return all().filter((t) => !k || t.properties.name.toLowerCase().includes(k) || t.properties.motto.toLowerCase().includes(k));
  });
  const mine = () => (me() ? social.teamsOf(me()!.xuid) : []);
  return (
    <>
      <Crumbs trail={[["Home", "/stats"], ["Clans"]]} />
      <Strip kind="players" />
      <div class="s-cols s-cols--over">
        <aside class="s-left">
          <Title>Clans</Title>
          <Pills label="Clans" items={[
            { label: `Clan Directory (${all().length})`, href: "#directory" },
            ...(me() ? [{ label: `My Clans (${mine().length})`, href: "#mine" }, { label: "Start a Clan", href: "#start" }] : [{ label: "Sign Up for h2 live", href: "/dash/signup" }]),
            { label: "Players & Leaderboards", href: "/stats/players" },
          ]} />
          <Show when={me()}>
            <MyTeams />
            <StartTeam />
          </Show>
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
                <thead><tr><th>Clan</th><th class="hide-s">Motto</th><th>Leader</th><th class="num">Members</th><th class="num">Online</th><th class="num hide-s">Founded</th></tr></thead>
                <tbody>
                  <For each={shown()} fallback={<tr><td colspan="6" class="s-dim">{social.teamsError() ? `Couldn't read teams: ${social.teamsError()}` : !social.teamsLoaded() ? "Loading teams…" : all().length ? "No team matches that." : "No teams on this server yet."}</td></tr>}>
                    {(t) => {
                      const lead = () => leaderOf(t);
                      const online = () => Object.keys(t.members).filter((x) => live.state.accounts[x]?.online).length;
                      return (
                        <tr>
                          <td><span class="s-who"><ClanEmblem team={t} size={16} /><A href={clanHref(t.id)}>{t.properties.name}</A></span></td>
                          <td class="hide-s s-soc-motto">{t.properties.motto}</td>
                          <td><Show when={lead()} fallback={<span class="s-dim">none</span>}>{(l) => <A href={playerHref(l().identity.gamertag)}>{l().identity.gamertag}</A>}</Show></td>
                          <td class="num">{Object.keys(t.members).length}</td>
                          <td class="num">{online() || "–"}</td>
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

function MyTeams() {
  const me = () => identity.me()!;
  const list = () => social.teamsOf(me().xuid);
  const [r, setR] = createSignal<WriteResult | null>(null);
  return (
    <Panel title="My Clans" sub="clans you're in or invited to" id="mine">
      <Show when={list().length} fallback={<p class="s-recent-empty">You're not in a clan yet.</p>}>
        <ul class="s-mini-list">
          <For each={list()}>
            {(t) => {
              const role = () => roleOf(t, me().xuid);
              return (
                <li>
                  <ClanEmblem team={t} size={14} /><A href={clanHref(t.id)}>{t.properties.name}</A>
                  <Show when={role() === "recruit"} fallback={<span class="s-dim">{ROLE_LABEL[role() ?? "member"]}</span>}>
                    <span class="s-soc-btns">
                      <button class="s-btn s-btn--go" disabled={social.busy()} onClick={async () => setR(await social.write(teamWrites.answer(t.id, true)))}>Join</button>
                      <button class="s-btn" disabled={social.busy()} onClick={async () => setR(await social.write(teamWrites.answer(t.id, false)))}>Decline</button>
                    </span>
                  </Show>
                </li>
              );
            }}
          </For>
        </ul>
      </Show>
      <Note r={r()} />
    </Panel>
  );
}

function StartTeam() {
  const navigate = useNavigate();
  const [name, setName] = createSignal("");
  const [motto, setMotto] = createSignal("");
  const [r, setR] = createSignal<WriteResult | null>(null);
  async function submit(e: Event) {
    e.preventDefault();
    const p = { name: name().trim(), motto: motto().trim(), description: "", url: "" };
    const bad = teamPropsError(p);
    if (bad) { setR({ ok: false, error: bad }); return; }
    const id = randomTeamId();
    const res = await social.write(teamWrites.create(id, p));
    setR(res);
    if (res.ok) navigate(clanHref(id));
  }
  return (
    <Panel title="Start a Clan" sub="you lead it; recruit by gamertag" id="start">
      <form class="s-soc-form" onSubmit={submit}>
        <label for="s-team-name">Clan name</label>
        <input id="s-team-name" maxLength={TEAM_NAME_MAX} value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        <label for="s-team-motto">Motto</label>
        <input id="s-team-motto" maxLength={TEAM_TEXT_MAX} value={motto()} onInput={(e) => setMotto(e.currentTarget.value)} placeholder="optional" />
        <button type="submit" class="s-btn" disabled={social.busy() || !name().trim()}>create clan</button>
      </form>
      <Note r={r()} />
    </Panel>
  );
}

// ---------- one clan ----------

export function ClanPage() {
  const params = useParams<{ id: string }>();
  const id = () => params.id.toLowerCase();
  const team = () => social.team(id());
  return (
    <Show when={team()} fallback={
      <>
        <Crumbs trail={[["Home", "/stats"], ["Clans", "/stats/clans"], [id()]]} />
        <Show when={social.teamsLoaded()} fallback={<div class="s-loading">Loading team…</div>}>
          <div class="s-full">
            <Title class="s-title--bar">Clan Not Found</Title>
            <p class="s-pad">There's no clan {id()} on this server. It may have been disbanded. <A href="/stats/clans">Browse clans</A></p>
          </div>
        </Show>
      </>
    }>
      {(t) => <TeamBody team={t()} />}
    </Show>
  );
}

function TeamBody(props: { team: Team }) {
  const t = () => props.team;
  const me = () => identity.me();
  const myRole = () => roleOf(t(), me()?.xuid);
  const rec = createMemo(() => teamRecord(live.state, Object.keys(t().members)));
  const members = createMemo(() => sortedMembers(t()));
  const recruits = createMemo(() => Object.values(t().recruits));
  const online = createMemo(() => members().filter((m) => live.state.accounts[m.identity.xuid]?.online));
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
            <dt>Online Now:</dt><dd>{online().length}</dd>
            <dt>Games Together:</dt><dd><Derived note="Games two or more members played on the same side, from the games this page has loaded; derived in your browser.">{num(rec().together)}</Derived></dd>
          </dl>
        </div>
        <Show when={myRole()}>
          {(r) => <div class="s-team-you"><small>You are</small><b>{r() === "recruit" ? "Invited" : ROLE_LABEL[r()]}</b></div>}
        </Show>
      </div>
      <Show when={myRole() === "recruit"}><Invitation team={t()} /></Show>
      <div class="s-cols">
        <aside class="s-left">
          <Title>Clan Record</Title>
          <Pills label="Clan" items={[
            { label: `Roster (${members().length})`, href: "#roster" },
            { label: "Combined Stats", href: "#combined" },
            { label: `Games Together (${rec().together})`, href: "#games" },
            { label: "Clan Directory", href: "/stats/clans" },
          ]} />
          <Show when={can(t(), me()?.xuid, PRIV.recruit)}><Recruit team={t()} /></Show>
          <Show when={can(t(), me()?.xuid, PRIV.modifyTeam)}><EditTeam team={t()} /></Show>
          <Show when={myRole() && myRole() !== "recruit"}><Membership team={t()} /></Show>
          <Panel title="Online Now" sub="members signed in">
            <Show when={online().length} fallback={<p class="s-recent-empty">No members are signed in.</p>}>
              <ul class="s-mini-list">
                <For each={online()}>{(m) => <li><Emblem seed={m.identity.xuid} size={14} /><A href={playerHref(m.identity.gamertag)}>{m.identity.gamertag}</A><span class="s-dim"><PresenceBadge account={live.state.accounts[m.identity.xuid]} /></span></li>}</For>
              </ul>
            </Show>
          </Panel>
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
                <thead><tr><th>Player</th><th>Role</th><th class="hide-s">Rank <Derived>{""}</Derived></th><th class="num">K/D</th><th>Status</th><th class="num hide-s">Joined</th><th class="num" /></tr></thead>
                <tbody>
                  <For each={members()}>{(m) => <RosterRow team={t()} m={m} />}</For>
                </tbody>
              </table>
            </div>
            <Show when={recruits().length}>
              <h4 class="s-soc-sub">Invited</h4>
              <div class="s-grid-wrap">
                <table class="s-grid s-grid--roomy">
                  <thead><tr><th>Player</th><th>Role offered</th><th class="hide-s">Rank <Derived>{""}</Derived></th><th class="num">K/D</th><th>Status</th><th class="num hide-s">Invited</th><th class="num" /></tr></thead>
                  <tbody><For each={recruits()}>{(m) => <RosterRow team={t()} m={m} recruit />}</For></tbody>
                </table>
              </div>
            </Show>
          </div>
          <Title class="s-title--rule">Combined Stats</Title>
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
            <p class="s-soc-foot"><Derived note="Counted from multiplayer games two or more members played together on one side, out of the games this page has loaded; campaign games count apart. The server keeps no clan stats.">Derived from shared games</Derived></p>
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

function RosterRow(props: { team: Team; m: TeamMember; recruit?: boolean }) {
  const me = () => identity.me()?.xuid;
  const x = () => props.m.identity.xuid;
  const role = () => roleOf(props.team, x());
  const [r, setR] = createSignal<WriteResult | null>(null);
  const canRemove = () => !!me() && me() !== x() && x() !== props.team.creator && can(props.team, me(), PRIV.removeMember);
  const canPromote = () => !props.recruit && !!me() && me() !== x() && x() !== props.team.creator && can(props.team, me(), OFFICER);
  // The member's multiplayer rank and K/D, as on their service record.
  const rec = createMemo(() => serviceRecord(live.state, x()));
  const offered = () => (props.m.properties.privileges & (PRIV.removeMember | PRIV.modifyMember) ? "Officer" : "Member");
  return (
    <tr>
      <td><span class="s-who"><Emblem seed={x()} size={16} /><A href={playerHref(props.m.identity.gamertag)}>{props.m.identity.gamertag}</A></span></td>
      <td>{props.recruit ? offered() : <span class={`s-soc-role s-soc-role--${role()}`}>{ROLE_LABEL[role() ?? "member"]}</span>}</td>
      <td class="hide-s"><span class="s-who"><RankIcon rank={rankIndex(rec().exp)} exp={rec().exp} size={16} />{rec().rank}</span></td>
      <td class="num">{rec().games ? ratio(rec().kills, rec().deaths) : "–"}</td>
      <td><PresenceBadge account={live.state.accounts[x()]} /></td>
      <td class="num hide-s">{usDate(props.m.joined_at)}</td>
      <td class="num s-soc-cell">
        <Show when={canPromote()}>
          <button class="s-btn" disabled={social.busy()} title={role() === "officer" ? "Make an ordinary member" : "Make an officer (recruit, remove, edit members)"}
            onClick={async () => setR(await social.write(teamWrites.member(props.team.id, x(), { ...props.m.properties, privileges: role() === "officer" ? MEMBER : OFFICER })))}>
            {role() === "officer" ? "Demote" : "Promote"}
          </button>
        </Show>
        <Show when={canRemove()}>
          <button class="s-btn" disabled={social.busy()} onClick={async () => setR(await social.write(teamWrites.remove(props.team.id, x())))}>{props.recruit ? "Cancel" : "Remove"}</button>
        </Show>
        <Show when={r() && !r()!.ok}><span class="s-soc-err" title={(r() as { error: string }).error}>!</span></Show>
      </td>
    </tr>
  );
}

function Invitation(props: { team: Team }) {
  const [r, setR] = createSignal<WriteResult | null>(null);
  return (
    <div class="s-soc-banner" role="region" aria-label="Clan invitation">
      <span>You've been invited to join <b>{props.team.properties.name}</b>.</span>
      <button class="s-btn s-btn--go" disabled={social.busy()} onClick={async () => setR(await social.write(teamWrites.answer(props.team.id, true)))}>Join Clan</button>
      <button class="s-btn" disabled={social.busy()} onClick={async () => setR(await social.write(teamWrites.answer(props.team.id, false)))}>Decline</button>
      <Note r={r()} />
    </div>
  );
}

function Recruit(props: { team: Team }) {
  const [tag, setTag] = createSignal("");
  const [officer, setOfficer] = createSignal(false);
  const [r, setR] = createSignal<WriteResult | null>(null);
  const mayOfficer = () => can(props.team, identity.me()?.xuid, OFFICER);
  async function submit(e: Event) {
    e.preventDefault();
    const t = tag().trim();
    const bad = gamertagError(t);
    if (bad) { setR({ ok: false, error: `Gamertag: ${bad}.` }); return; }
    const who = await social.lookup(t).catch(() => null);
    if (!who) { setR({ ok: false, error: "No account has that gamertag." }); return; }
    const res = await social.write(teamWrites.recruit(props.team.id, who, officer() && mayOfficer() ? OFFICER : MEMBER));
    setR(res);
    if (res.ok) setTag("");
  }
  return (
    <Panel title="Recruit" sub="invite a player by gamertag">
      <form class="s-soc-form" onSubmit={submit}>
        <label for="s-recruit">Gamertag</label>
        <input id="s-recruit" maxLength={15} value={tag()} onInput={(e) => setTag(e.currentTarget.value)} list="s-finder-list" />
        <Show when={mayOfficer()}>
          <label class="s-soc-check"><input type="checkbox" checked={officer()} onChange={(e) => setOfficer(e.currentTarget.checked)} /> as an officer</label>
        </Show>
        <button type="submit" class="s-btn" disabled={social.busy() || !tag().trim()}>send invitation</button>
      </form>
      <Note r={r()} ok="Invitation sent." />
    </Panel>
  );
}

function EditTeam(props: { team: Team }) {
  const p = () => props.team.properties;
  const [name, setName] = createSignal(p().name);
  const [motto, setMotto] = createSignal(p().motto);
  const [desc, setDesc] = createSignal(p().description);
  const [url, setUrl] = createSignal(p().url);
  const [r, setR] = createSignal<WriteResult | null>(null);
  const [sure, setSure] = createSignal(false);
  const navigate = useNavigate();
  async function submit(e: Event) {
    e.preventDefault();
    const next = { ...p(), name: name().trim(), motto: motto().trim(), description: desc().trim(), url: url().trim() };
    const bad = teamPropsError(next);
    if (bad) { setR({ ok: false, error: bad }); return; }
    setR(await social.write(teamWrites.properties(props.team.id, next)));
  }
  const leader = () => can(props.team, identity.me()?.xuid, PRIV.delete);
  return (
    <Panel title="Clan Settings" sub="name, motto and description">
      <form class="s-soc-form" onSubmit={submit}>
        <label for="s-edit-name">Clan name</label>
        <input id="s-edit-name" maxLength={TEAM_NAME_MAX} value={name()} onInput={(e) => setName(e.currentTarget.value)} />
        <label for="s-edit-motto">Motto</label>
        <input id="s-edit-motto" maxLength={TEAM_TEXT_MAX} value={motto()} onInput={(e) => setMotto(e.currentTarget.value)} />
        <label for="s-edit-desc">Description</label>
        <textarea id="s-edit-desc" maxLength={TEAM_TEXT_MAX} rows={3} value={desc()} onInput={(e) => setDesc(e.currentTarget.value)} />
        <label for="s-edit-url">Web site</label>
        <input id="s-edit-url" maxLength={TEAM_TEXT_MAX} value={url()} onInput={(e) => setUrl(e.currentTarget.value)} />
        <button type="submit" class="s-btn" disabled={social.busy()}>save changes</button>
      </form>
      <Note r={r()} ok="Saved." />
      <Show when={leader()}>
        <div class="s-soc-danger">
          <Show when={sure()} fallback={<button class="s-btn" onClick={() => setSure(true)}>Disband clan…</button>}>
            <span>Disband {p().name} for everyone?</span>
            <button class="s-btn s-btn--bad" disabled={social.busy()} onClick={async () => { const res = await social.write(teamWrites.delete(props.team.id)); if (res.ok) navigate("/stats/clans"); else setR(res); }}>Disband</button>
            <button class="s-btn" onClick={() => setSure(false)}>Keep</button>
          </Show>
        </div>
      </Show>
    </Panel>
  );
}

function Membership(props: { team: Team }) {
  const role = () => roleOf(props.team, identity.me()?.xuid);
  const [r, setR] = createSignal<WriteResult | null>(null);
  const [sure, setSure] = createSignal(false);
  return (
    <Panel title="Your Membership" sub={`you are ${role() === "leader" ? "the leader" : `a${role() === "officer" ? "n officer" : " member"}`}`}>
      <div class="s-soc-pad">
        <Show when={role() !== "leader"} fallback={<p class="s-dim">The leader can't leave a clan; disband it instead.</p>}>
          <Show when={sure()} fallback={<button class="s-btn" onClick={() => setSure(true)}>Leave clan…</button>}>
            <span>Leave {props.team.properties.name}?</span>{" "}
            <button class="s-btn s-btn--bad" disabled={social.busy()} onClick={async () => setR(await social.write(teamWrites.remove(props.team.id, identity.me()!.xuid)))}>Leave</button>{" "}
            <button class="s-btn" onClick={() => setSure(false)}>Stay</button>
          </Show>
        </Show>
        <Note r={r()} ok="You left the clan." />
      </div>
    </Panel>
  );
}
