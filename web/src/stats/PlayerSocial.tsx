// The player pages' friends and teams boxes: the player's friends (with
// presence, live from the stream), their teams and roles, and, when you
// are signed in, the add / accept / cancel friend button.

import { For, Match, Show, Switch, createSignal } from "solid-js";
import { A } from "@solidjs/router";
import { identity } from "../state/identity";
import { social, type WriteResult } from "../state/social";
import { ROLE_LABEL, friendStatus, friendWrites, roleOf } from "../api/social";
import type { AccountState } from "../state/reducer";
import Emblem from "../components/Emblem";
import { Panel, playerHref } from "./Layout";
import { ClanEmblem, friendPresence, presenceLine } from "./clanBits";
import { clanHref } from "./Clans";
import "../styles/social.css";

/** The add-friend button for `account`, in whatever state the two are. */
export function FriendButton(props: { account: AccountState }) {
  const me = () => identity.me();
  const status = () => friendStatus(social.mine(), me()?.xuid ?? null, props.account.xuid);
  const [r, setR] = createSignal<WriteResult | null>(null);
  const send = async (w: ReturnType<typeof friendWrites.request>) => setR(await social.write(w));
  return (
    <Show when={me() && social.mine() && status() !== "self"}>
      <span class="s-soc-btns">
        <Switch>
          <Match when={status() === "none"}><button class="s-btn s-btn--go" disabled={social.busy()} onClick={() => send(friendWrites.request(me()!.xuid, props.account.xuid))}>Add Friend</button></Match>
          <Match when={status() === "incoming"}>
            <button class="s-btn s-btn--go" disabled={social.busy()} onClick={() => send(friendWrites.accept(me()!.xuid, props.account.xuid))}>Accept Friend</button>
            <button class="s-btn" disabled={social.busy()} onClick={() => send(friendWrites.decline(me()!.xuid, props.account.xuid))}>Decline</button>
          </Match>
          <Match when={status() === "outgoing"}><button class="s-btn" disabled={social.busy()} title="Cancel the friend request" onClick={() => send(friendWrites.remove(me()!.xuid, props.account.xuid))}>Request Sent ✕</button></Match>
          <Match when={status() === "friend"}><span class="s-soc-friend">✓ Friends</span></Match>
        </Switch>
        <Show when={r() && !r()!.ok}><span class="s-soc-err" role="alert">{(r() as { error: string }).error}</span></Show>
      </span>
    </Show>
  );
}

export function PlayerFriends(props: { account: AccountState }) {
  const doc = () => social.friendsOf(props.account.xuid, props.account.gamertag);
  const mine = () => identity.me()?.xuid === props.account.xuid;
  const list = () => {
    const d = doc();
    if (!d) return [];
    return [...d.friends].sort((a, b) => Number(friendPresence(b).kind !== "offline") - Number(friendPresence(a).kind !== "offline"));
  };
  return (
    <Panel title="Friends" sub={mine() ? "your friends on this server" : "friends on this server"} right={<FriendButton account={props.account} />} class="s-soc-box">
      <Show when={doc()} fallback={<p class="s-recent-empty">Loading friends…</p>}>
        <Show when={mine() && doc()!.requests_in.length}>
          <p class="s-soc-pending"><A href="/dash/friends">{doc()!.requests_in.length} friend request{doc()!.requests_in.length === 1 ? "" : "s"} waiting</A></p>
        </Show>
        <Show when={list().length} fallback={<p class="s-recent-empty">{mine() ? "No friends yet. Add someone from their service record." : "No friends yet."}</p>}>
          <ul class="s-mini-list">
            <For each={list().slice(0, 10)}>
              {(f) => {
                const p = () => friendPresence(f);
                return (
                  <li>
                    <Emblem seed={f.xuid} size={14} /><A href={playerHref(f.gamertag)}>{f.gamertag}</A>
                    <span class="s-dim">
                      <Show when={p().game_id} fallback={presenceLine(f)}>{(g) => <A href={`/stats/games/${g()}`}>{presenceLine(f)}</A>}</Show>
                    </span>
                    <Show when={p().kind !== "offline"}><span class="s-online-dot" title={presenceLine(f)} /></Show>
                  </li>
                );
              }}
            </For>
          </ul>
          <Show when={list().length > 10}><p class="s-soc-more">and {list().length - 10} more</p></Show>
        </Show>
      </Show>
    </Panel>
  );
}

export function PlayerClans(props: { account: AccountState }) {
  const teams = () => social.teamsOf(props.account.xuid);
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
                  <span class="s-dim">{role() === "recruit" ? "invited" : ROLE_LABEL[role() ?? "member"]}</span>
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
