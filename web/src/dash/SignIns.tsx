// "Signed in": where the signed-in account is signed in (its keys: this
// browser, other browsers, game installs), as a dashboard list screen, with
// signing any other one out, or every other one at once. Each asks first:
// select picks the row, select again (on "confirm") signs it out, back
// keeps it. This browser's own row signs out here.
// src/state/signins.ts is the API.

import { Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { now } from "../lib/now";
import { confirmText, fetchSignIns, OTHERS_KEY, signInMeta, signInRows, signOutOne, signOutOthers, viaText, type SignIn } from "../state/signins";
import { BigPod, Glyph, Plate, Prompts } from "./parts";
import type { Action } from "./input";
import { ListPanel } from "./Dashboard";
import "../styles/dash-signins.css";

export interface SignInsProps {
  go: (href: string, dir: "deeper" | "back") => void;
  back: () => void;
  bind: (fn: ((a: Action) => boolean) | null) => void;
}

export default function SignInsScreen(props: SignInsProps) {
  const [list, setList] = createSignal<SignIn[] | null>(null);
  const [err, setErr] = createSignal<string | null>(null);
  const [note, setNote] = createSignal<string | null>(null);
  const [sel, setSel] = createSignal(0);
  const [confirming, setConfirming] = createSignal(false);
  const [busy, setBusy] = createSignal(false);

  const ctx = () => {
    const me = identity.me(), wk = live.wellKnown();
    return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
  };
  async function load() {
    const c = ctx();
    if (!c) return;
    try {
      const l = await fetchSignIns(c.api, c.aud, c.me);
      if (identity.me()?.xuid !== c.me.xuid) return;
      setList(l); setErr(null);
    } catch (x) {
      setErr(`couldn't read where you're signed in: ${(x as Error).message}`);
    }
  }
  createEffect(on(() => [identity.me()?.xuid, live.wellKnown()?.id], () => { setList(null); void load(); }));

  const rows = createMemo(() => signInRows(list() ?? [], now()));
  createEffect(() => { if (sel() >= rows().length) setSel(Math.max(0, rows().length - 1)); });
  const row = () => rows()[sel()];
  const current = () => list()?.find((s) => s.kid === row()?.key);
  const choose = (i: number) => { if (i !== sel()) setConfirming(false); setSel(i); };

  async function signOut() {
    const r = row(), c = ctx();
    if (!r || !c || busy()) return;
    if (!confirming()) { setConfirming(true); setNote(null); return; }
    setBusy(true); setErr(null);
    try {
      if (current()?.current) { identity.signOut(); props.go("/", "back"); return; }
      const gone = r.key === OTHERS_KEY ? await signOutOthers(c.api, c.aud, c.me) : await signOutOne(c.api, c.aud, c.me, r.key);
      setNote(gone.length === 1 ? "Signed out 1 sign-in." : `Signed out ${gone.length} sign-ins.`);
      setConfirming(false);
      await load();
    } catch (x) {
      setErr(`couldn't sign out: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }

  function act(a: Action): boolean {
    const n = rows().length;
    switch (a) {
      case "down": choose(Math.min(n - 1, sel() + 1)); return true;
      case "up": choose(Math.max(0, sel() - 1)); return true;
      case "pageDown": choose(Math.min(n - 1, sel() + 6)); return true;
      case "pageUp": choose(Math.max(0, sel() - 6)); return true;
      case "select": void signOut(); return true;
      case "back": case "left":
        if (confirming()) setConfirming(false); else props.back();
        return true;
      default: return false;
    }
  }
  onMount(() => props.bind(act));
  onCleanup(() => props.bind(null));

  const glyph = () => (row()?.key === OTHERS_KEY ? "exit" : current()?.kind === "game" ? "disc" : current()?.kind === "browser" ? "globe" : "key");
  const day = (iso: string) => new Date(iso).toLocaleString(undefined, { day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit" });
  return (
    <div class="dx-screen dx-list dx-pages dx-signins">
      <ListPanel
        title="signed in" rows={rows()} sel={sel()} onSel={choose}
        onAct={(i) => { choose(i); void signOut(); }}
        empty={err() ? "can't reach the server" : "looking"}
      />
      <div class="dx-preview"><BigPod><Glyph name={glyph()} /></BigPod></div>
      <Plate small={row()?.key === OTHERS_KEY ? "all but this browser" : current()?.current ? "this browser" : current()?.kind === "game" ? "halo 2" : "sign-in"} title={row()?.label ?? "signed in"}>
        <Show when={!confirming() && current()}>
          {(s) => (
            <dl>
              <Show when={s().name && !s().current}><dt>device</dt><dd>{s().name}</dd></Show>
              <Show when={s().current && s().name}><dt>browser</dt><dd>{s().name}</dd></Show>
              <dt>signed in</dt><dd>{day(s().created_at)}<Show when={viaText(s().via)}> <span>{viaText(s().via)}</span></Show></dd>
              <dt>last used</dt><dd>{s().current ? "now" : s().last_seen ? signInMeta(s(), now()).replace(/^used /, "") : "not since the server started"}</dd>
            </dl>
          )}
        </Show>
        <Show when={confirming() && row()}>
          {(r) => <p class="dx-error" role="alert">{confirmText(r(), current())} Press select to sign out, back to keep it.</p>}
        </Show>
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
        <Show when={note() && !confirming()}><p class="dx-note" role="status">{note()}</p></Show>
        <Show when={!confirming() && !note() && !err()}>
          <p class="dx-note">{row()?.key === OTHERS_KEY ? "Signs out every browser and copy of Halo 2 but this one." : "Signing out stops it at its next request."}</p>
        </Show>
      </Plate>
      <Prompts
        onA={rows().length ? () => void signOut() : undefined}
        onB={() => act("back")}
        aLabel={confirming() ? "confirm" : "sign out"}
        bLabel={confirming() ? "keep" : "back"}
      />
    </div>
  );
}
