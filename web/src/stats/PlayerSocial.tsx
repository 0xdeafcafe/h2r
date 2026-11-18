// Public friends and clan membership on a player's service record.

import { For, Show } from "solid-js";
import { A } from "@solidjs/router";
import { identity } from "../state/identity";
import { social } from "../state/social";
import { ROLE_LABEL, roleOf } from "../api/social";
import type { AccountState } from "../state/reducer";
import Emblem from "../components/Emblem";
import { Panel, playerHref } from "./Layout";
import { ClanEmblem } from "./clanBits";
import { clanHref } from "./modes";
import "../styles/social.css";

export function PlayerFriends(props: { account: AccountState }) {
  const doc = () => social.friendsOf(props.account.xuid, props.account.gamertag);
  const list = () => doc()?.friends ?? [];
  const manageHref = () => identity.me()?.xuid === props.account.xuid
    ? "/dash/friends"
    : `/dash/friends/${encodeURIComponent(props.account.gamertag)}`;
  return (
    <Panel title="Friends" right={<A href={manageHref()} class="s-soc-link">dashboard</A>} class="s-soc-box">
      <Show when={doc()} fallback={<p class="s-recent-empty">Loading friends…</p>}>
        <Show when={list().length} fallback={<p class="s-recent-empty">No friends yet.</p>}>
          <ul class="s-mini-list">
            <For each={list().slice(0, 10)}>
              {(f) => <li><Emblem seed={f.xuid} size={14} /><A href={playerHref(f.gamertag)}>{f.gamertag}</A></li>}
            </For>
          </ul>
          <Show when={list().length > 10}><p class="s-soc-more">and {list().length - 10} more</p></Show>
        </Show>
      </Show>
    </Panel>
  );
}

export function PlayerClans(props: { account: AccountState }) {
  const teams = () => social.teamsOf(props.account.xuid).filter((t) => !!t.members[props.account.xuid]);
  return (
    <Panel title="Clans" sub="clans they're in" right={<A href="/stats/clans" class="s-soc-link">all clans</A>} class="s-soc-box">
      <Show when={teams().length} fallback={<p class="s-recent-empty">{social.teamsLoaded() ? "Not in a clan." : "Loading clans…"}</p>}>
        <ul class="s-mini-list">
          <For each={teams()}>
            {(t) => {
              const role = () => roleOf(t, props.account.xuid);
              return (
                <li>
                  <ClanEmblem team={t} size={14} /><A href={clanHref(t.id)}>{t.properties.name}</A>
                  <span class="s-dim">{ROLE_LABEL[role() ?? "member"]}</span>
                </li>
              );
            }}
          </For>
        </ul>
      </Show>
    </Panel>
  );
}

/** Both boxes, for the service record's menu column. */
export default function PlayerSocial(props: { account: AccountState }) {
  return (
    <>
      <PlayerClans account={props.account} />
      <PlayerFriends account={props.account} />
    </>
  );
}
