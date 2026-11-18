// The mute list on the dashboard (docs/live/live-api.md "Mute list"), over
// ../admin/mutes.ts: on the signed-in player's own screen, the players they
// muted with Unmute on each; on another player's screen, Mute or Unmute.
// The same list the game reads at sign-in, so a change here holds in the
// game's voice from the next sign-in, both ways.

import { For, Show, createEffect, createSignal, on } from "solid-js";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { fetchMutes, setMute } from "../admin/mutes";
import type { Who } from "../admin/reports";

const ctx = () => {
  const me = identity.me(), wk = live.wellKnown();
  return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
};

/** The signed-in player's mute list, loaded once per account and server. */
function createMutes() {
  const [list, setList] = createSignal<Who[] | null>(null);
  const [err, setErr] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  async function load() {
    const c = ctx();
    if (!c) return;
    try { setList(await fetchMutes(c.api, c.aud, c.me)); setErr(null); } catch (x) { setErr((x as Error).message); }
  }
  createEffect(on(() => [identity.me()?.xuid, live.server().base, live.wellKnown()?.id], () => { setList(null); void load(); }));
  async function change(target: Who, mute: boolean) {
    const c = ctx();
    if (!c || busy()) return;
    setBusy(true); setErr(null);
    try { await setMute(c.api, c.aud, c.me, target.xuid, mute); await load(); } catch (x) { setErr(`couldn't ${mute ? "mute" : "unmute"}: ${(x as Error).message}`); } finally { setBusy(false); }
  }
  return { list, err, busy, change };
}

/** The players the signed-in player muted. Nothing while it is empty. */
export function MutedList() {
  const m = createMutes();
  return (
    <>
      <Show when={m.list()?.length}>
        <div class="dx-mutes">
          <p class="dx-note">muted in voice (both ways):</p>
          <ul>
            <For each={m.list()!}>
              {(w) => (
                <li>
                  <b class="dx-blade-name">{w.gamertag}</b>{" "}
                  <button type="button" class="dx-forget" disabled={m.busy()} onClick={() => void m.change(w, false)}>unmute</button>
                </li>
              )}
            </For>
          </ul>
        </div>
      </Show>
      <Show when={m.err()}><p class="dx-error" role="alert">{m.err()}</p></Show>
    </>
  );
}

/** Mute or unmute `target` for the signed-in player. */
export function MuteButton(props: { target: Who }) {
  const m = createMutes();
  const muted = () => m.list()?.some((w) => w.xuid === props.target.xuid) ?? false;
  return (
    <Show when={m.list()}>
      <p class="dx-note dx-mute">
        {muted() ? "muted in voice" : "voice"}{" "}
        <button type="button" class="dx-forget" disabled={m.busy()} onClick={() => void m.change(props.target, !muted())}>{muted() ? "unmute" : "mute"}</button>
      </p>
      <Show when={m.err()}><p class="dx-error" role="alert">{m.err()}</p></Show>
    </Show>
  );
}
