// The dashboard's account screens (src/state/recovery.ts is the API, which
// calls the sign-in code "recovery"):
//   - /dash/account: the signed-in account's sign-in code, hidden until
//     asked for and then counting down, its authenticator setup (secret and
//     QR code), set up / new code, and where it is signed in (its keys;
//     SignIns.tsx lists them). Right after sign-up it opens here once, with
//     the setup and the code showing;
//   - /dash/recover: a new device signs in with its gamertag and a code.
// Dashboard.tsx owns the layout (big pod, small pods, prompts); these are
// the plates, the code in the big pod and their state.

import { Match, Show, Switch, createEffect, createMemo, createSignal, on, onCleanup, type Accessor } from "solid-js";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { now } from "../lib/now";
import { qrEncode, qrPath } from "../lib/qr";
import { fetchWellKnown } from "../api/server";
import { fetchSignIns, signInSummary, type SignIn } from "../state/signins";
import {
  codeError, fetchRecovery, groupSecret, MASKED_CODE, normalizeCode, recoverAccount, RecoverError, REVEAL_MS, secondsLeft, setupRecovery,
  shownCode, type Recovery,
} from "../state/recovery";
import { gamertagError, type LocalAccount } from "../state/account";
import { adminState } from "../admin/state";
import { copyText } from "./Server";
import { Plate } from "./parts";

export const NEW_DEVICE_NOTE = "To sign in on a new device or in Halo 2, enter your gamertag and the sign-in code shown here or in your authenticator app.";

/** The sign-in code setup a sign-up just got, shown once on the next screen. */
const [fresh, setFresh] = createSignal<{ xuid: string; rec: Recovery } | null>(null);
export const keepFreshRecovery = (xuid: string, rec: Recovery) => setFresh({ xuid, rec });

export interface RecoveryState {
  rec: Accessor<Recovery | null>;
  phase: Accessor<"loading" | "none" | "ready" | "error">;
  error: Accessor<string | null>;
  /** Just signed up: this is the first look at it. */
  isNew: Accessor<boolean>;
  showSetup: Accessor<boolean>;
  /** Where the account is signed in; null while loading or when unreadable. */
  signins: Accessor<SignIn[] | null>;
  toggleSetup(): void;
  /** The code in view (it hides itself after REVEAL_MS, and on leaving). */
  revealed: Accessor<boolean>;
  toggleReveal(): void;
  /** The code as shown: itself or masked. */
  shown: Accessor<string>;
  /** Seconds until the code rolls over. */
  left: Accessor<number>;
  busy: Accessor<boolean>;
  confirming: Accessor<boolean>;
  /** Set up (none yet), or ask to reset, or (asked) reset. */
  setupOrReset(): void;
  cancelReset(): void;
  copyCode(): Promise<boolean>;
}

/** The signed-in account's recovery, read when the screen opens and again
 *  each time the code rolls over. */
export function createRecovery(active: Accessor<boolean>): RecoveryState {
  const [rec, setRec] = createSignal<Recovery | null>(null);
  const [phase, setPhase] = createSignal<"loading" | "none" | "ready" | "error">("loading");
  const [error, setError] = createSignal<string | null>(null);
  const [isNew, setIsNew] = createSignal(false);
  const [showSetup, setShowSetup] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [confirming, setConfirming] = createSignal(false);
  const [signins, setSignIns] = createSignal<SignIn[] | null>(null);
  const [revealUntil, setRevealUntil] = createSignal(0);
  const revealed = () => now() < revealUntil();
  const hide = () => setRevealUntil(0);
  // A hidden tab hides the code too.
  const onVisibility = () => { if (document.visibilityState === "hidden") hide(); };
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", onVisibility);
    onCleanup(() => document.removeEventListener("visibilitychange", onVisibility));
  }
  let seq = 0, fetching = false, lastFetch = 0;

  const ctx = () => {
    const me = identity.me(), wk = live.wellKnown();
    return me && wk ? { me, aud: wk.id, api: live.server().api } : null;
  };

  async function load(quiet: boolean) {
    const c = ctx();
    if (!c) return;
    const n = ++seq;
    fetching = true;
    lastFetch = Date.now();
    if (!quiet) setPhase("loading");
    try {
      const r = await fetchRecovery(c.api, c.aud, c.me);
      if (n !== seq) return;
      setRec(r);
      setPhase(r ? "ready" : "none");
      setError(null);
    } catch (x) {
      if (n !== seq) return;
      setError(`couldn't read your sign-in code: ${(x as Error).message}`);
      if (!quiet || !rec()) setPhase("error");
    } finally { fetching = false; }
  }

  createEffect(on(() => [active(), identity.me()?.xuid, live.wellKnown()?.id], () => {
    // Leaving the screen (or switching account) hides the code.
    hide();
    if (!active()) return;
    setConfirming(false);
    setSignIns(null);
    const c = ctx();
    if (c) void fetchSignIns(c.api, c.aud, c.me).then((l) => { if (identity.me()?.xuid === c.me.xuid) setSignIns(l); }, () => {});
    const me = identity.me(), f = fresh();
    if (me && f && f.xuid === me.xuid) {
      setFresh(null);
      // Just made: the code is to be written down, so it shows.
      setRec(f.rec); setPhase("ready"); setIsNew(true); setShowSetup(true); setRevealUntil(Date.now() + REVEAL_MS);
      return;
    }
    setIsNew(false); setShowSetup(false); setRec(null);
    void load(false);
  }));

  const left = createMemo(() => { const r = rec(); return r ? secondsLeft(r, now()) : 0; });
  // A new code each period; not more than one read every two seconds, so a
  // browser clock far off the server's can't spin.
  createEffect(() => {
    if (active() && phase() === "ready" && left() <= 0 && !fetching && now() - lastFetch > 2000) void load(true);
  });

  async function setup() {
    const c = ctx();
    if (!c || busy()) return;
    setBusy(true); setError(null);
    try {
      const r = await setupRecovery(c.api, c.aud, c.me);
      seq++;
      setRec(r); setPhase("ready"); setShowSetup(true); setConfirming(false);
      lastFetch = Date.now();
    } catch (x) {
      setError(`couldn't set up a sign-in code: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }

  return {
    rec, phase, error, isNew, showSetup, left, busy, confirming, signins, revealed,
    toggleSetup: () => setShowSetup(!showSetup()),
    toggleReveal: () => setRevealUntil(revealed() ? 0 : Date.now() + REVEAL_MS),
    shown: () => { const r = rec(); return r ? shownCode(r.code, revealUntil(), now()) : MASKED_CODE; },
    setupOrReset() {
      if (phase() === "ready" && !confirming()) { setConfirming(true); return; }
      void setup();
    },
    cancelReset: () => setConfirming(false),
    copyCode: async () => { const r = rec(); return r ? copyText(normalizeCode(r.code)) : false; },
  };
}

/** An SVG QR code, dark on white with the quiet zone. */
export function QrCode(props: { text: string; label: string }) {
  const qr = createMemo(() => { try { return qrEncode(props.text, "M"); } catch { return null; } });
  return (
    <Show when={qr()}>
      {(q) => (
        <svg class="dx-qr" viewBox={`0 0 ${q().size + 8} ${q().size + 8}`} role="img" aria-label={props.label} shape-rendering="crispEdges">
          <rect width="100%" height="100%" fill="#fff" />
          <path d={qrPath(q(), 4)} fill="#000" />
        </svg>
      )}
    </Show>
  );
}

/** The big pod's face: the code and a ring running down to the next one. */
export function RecoveryCode(props: { state: RecoveryState }) {
  const s = props.state;
  const frac = () => { const r = s.rec(); return r ? s.left() / r.period : 0; };
  const R = 46, C = 2 * Math.PI * R;
  return (
    <div class="dx-rcode" classList={{ "dx-rcode--hidden": !s.revealed() }} aria-live="polite">
      <svg class="dx-rcode-ring" viewBox="0 0 100 100" aria-hidden="true">
        <circle cx="50" cy="50" r={R} class="dx-rcode-track" />
        <circle cx="50" cy="50" r={R} class="dx-rcode-left" style={{ "stroke-dasharray": `${C}`, "stroke-dashoffset": `${C * (1 - frac())}` }} />
      </svg>
      <b aria-label={s.revealed() ? undefined : "sign-in code hidden"}>{s.shown()}</b>
      <small>{s.revealed() ? `${s.left()}s` : "hidden"}</small>
    </div>
  );
}

/** The account plate: plain rows. Its actions are the pods and prompts
 *  round it (Dashboard.tsx), so the plate itself is only read. */
export function AccountPlate(props: { state: RecoveryState }) {
  const s = props.state;
  const me = () => identity.me();
  return (
    <Plate small={s.isNew() ? "new account" : "h2 live"} title={s.isNew() ? "save your sign-in code" : "account"} main>
      <div class="dx-recovery-body">
        <Show when={s.isNew()}><p class="dx-signup-intro">Keep this. It's how you sign in on another device, or get back in if you lose this browser.</p></Show>
        <div class="dx-recovery-grid" classList={{ "dx-recovery-grid--qr": s.showSetup() && !!s.rec()?.otpauth }}>
          <dl>
            <dt>gamertag</dt><dd>{me()?.gamertag}</dd>
            <dt>sign-in code</dt>
            <dd>
              <Switch>
                <Match when={s.phase() === "loading"}>loading</Match>
                <Match when={s.phase() === "none"}>not set up</Match>
                <Match when={s.phase() === "error"}>unavailable</Match>
                <Match when={s.rec()}>
                  <b class="dx-rcode-inline" classList={{ "dx-rcode-inline--hidden": !s.revealed() }}>{s.shown()}</b>{" "}
                  <span>{s.revealed() ? `${s.left()}s` : "hidden"}</span>
                </Match>
              </Switch>
            </dd>
            <Show when={s.showSetup() && s.rec()}>
              {(r) => <><dt>secret</dt><dd class="dx-secret">{groupSecret(r().secret)}</dd></>}
            </Show>
            <dt>signed in</dt>
            <dd class="dx-signins-summary">
              <Show when={s.signins()} fallback={<span>{s.phase() === "error" ? "unavailable" : "loading"}</span>}>
                {(list) => signInSummary(list())}
              </Show>
            </dd>
          </dl>
          <Show when={s.showSetup() && s.rec()?.otpauth}>
            {(uri) => <QrCode text={uri()} label={`authenticator setup for ${me()?.gamertag ?? ""}`} />}
          </Show>
        </div>
        <Show when={s.confirming()}>
          <p class="dx-error" role="alert">Make a new sign-in code? The current one stops working in every authenticator app it's in. Press select to make it, back to keep the old one.</p>
        </Show>
        <Show when={s.error() && s.phase() !== "loading"}><p class="dx-error" role="alert">{s.error()}</p></Show>
        <p class="dx-note">
          {s.phase() === "none" ? "Set up a sign-in code so you can sign in on a new device or in Halo 2." : NEW_DEVICE_NOTE}
          {s.showSetup() ? " Scan the code with an authenticator app, or type the secret into it." : ""}
        </p>
      </div>
    </Plate>
  );
}

/** A new device takes an account over: gamertag and code. */
export function RecoverPlate(props: { onDone: (a: LocalAccount) => void }) {
  const [tag, setTag] = createSignal("");
  const [code, setCode] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  async function submit(e: Event) {
    e.preventDefault();
    if (busy()) return;
    setErr(null);
    const t = tag().trim();
    const bad = gamertagError(t) ?? (codeError(code()) ? `code: ${codeError(code())}` : null);
    if (bad) { setErr(gamertagError(t) ? `gamertag: ${bad}` : bad); return; }
    setBusy(true);
    try {
      const wk = live.wellKnown() ?? (await fetchWellKnown(live.server().base));
      const a = await recoverAccount(live.server().api, live.server().base, wk.id, t, code());
      await identity.add(a);
      adminState.reset();
      props.onDone(a);
    } catch (x) {
      setErr(x instanceof RecoverError ? x.message : `couldn't sign in: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }
  return (
    <Plate small="new device" title="sign in with a code" main>
      <p class="dx-signup-intro">Your gamertag and its sign-in code.</p>
      <form id="dash-recover" class="dx-form" onSubmit={submit}>
        <label for="dash-recover-tag">gamertag</label>
        <input id="dash-recover-tag" value={tag()} maxLength={15} onInput={(e) => setTag(e.currentTarget.value)} autocomplete="username" spellcheck={false} />
        <label for="dash-recover-code">code</label>
        <input id="dash-recover-code" value={code()} maxLength={9} inputmode="numeric" autocomplete="one-time-code" onInput={(e) => setCode(e.currentTarget.value)} placeholder="123-456" />
        <button type="submit" class="dx-go" disabled={busy()}>{busy() ? "signing in" : "sign in"}</button>
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
        <p class="dx-note">
          The sign-in code is on the account screen of a device that's still signed in, or in your authenticator app.
          This browser gets a new key for the account; to play, choose “download for h2” again afterwards.
        </p>
      </form>
    </Plate>
  );
}
