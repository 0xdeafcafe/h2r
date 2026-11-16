// The dashboard's recovery screens (src/state/recovery.ts is the API):
//   - /dash/signup/recovery: the signed-in account's recovery code, counting
//     down, its authenticator setup (secret and QR code), and set up / reset.
//     Right after sign-up it opens here once, with the setup showing;
//   - /dash/signup/recover: a new device takes an account over with its
//     gamertag and a code.
// Dashboard.tsx owns the layout (big pod, small pods, prompts); these are
// the plates, the code in the big pod and their state.

import { Show, createEffect, createMemo, createSignal, on, type Accessor } from "solid-js";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { now } from "../lib/now";
import { qrEncode, qrPath } from "../lib/qr";
import { fetchWellKnown } from "../api/server";
import {
  codeError, fetchRecovery, groupSecret, normalizeCode, recoverAccount, RecoverError, secondsLeft, setupRecovery,
  type Recovery,
} from "../state/recovery";
import { gamertagError, type LocalAccount } from "../state/account";
import { adminState } from "../admin/state";
import { copyText } from "./Server";
import { Plate } from "./parts";

export const NEW_DEVICE_NOTE = "On a new device, enter your gamertag and the code shown here or in your authenticator app.";

/** The recovery setup a sign-up just got, shown once on the next screen. */
const [fresh, setFresh] = createSignal<{ xuid: string; rec: Recovery } | null>(null);
export const keepFreshRecovery = (xuid: string, rec: Recovery) => setFresh({ xuid, rec });

export interface RecoveryState {
  rec: Accessor<Recovery | null>;
  phase: Accessor<"loading" | "none" | "ready" | "error">;
  error: Accessor<string | null>;
  /** Just signed up: this is the first look at it. */
  isNew: Accessor<boolean>;
  showSetup: Accessor<boolean>;
  toggleSetup(): void;
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
      setError(`couldn't read your recovery setup: ${(x as Error).message}`);
      if (!quiet || !rec()) setPhase("error");
    } finally { fetching = false; }
  }

  createEffect(on(() => [active(), identity.me()?.xuid, live.wellKnown()?.id], () => {
    if (!active()) return;
    setConfirming(false);
    const me = identity.me(), f = fresh();
    if (me && f && f.xuid === me.xuid) {
      setFresh(null);
      setRec(f.rec); setPhase("ready"); setIsNew(true); setShowSetup(true);
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
      setError(`couldn't set up recovery: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }

  return {
    rec, phase, error, isNew, showSetup, left, busy, confirming,
    toggleSetup: () => setShowSetup(!showSetup()),
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
    <div class="dx-rcode" aria-live="polite">
      <svg class="dx-rcode-ring" viewBox="0 0 100 100" aria-hidden="true">
        <circle cx="50" cy="50" r={R} class="dx-rcode-track" />
        <circle cx="50" cy="50" r={R} class="dx-rcode-left" style={{ "stroke-dasharray": `${C}`, "stroke-dashoffset": `${C * (1 - frac())}` }} />
      </svg>
      <b>{s.rec()!.code}</b>
      <small>{s.left()}s</small>
    </div>
  );
}

/** The recovery plate: the code's words, the authenticator setup, set up / reset. */
export function RecoveryPlate(props: { state: RecoveryState; onDone: () => void }) {
  const s = props.state;
  const [copied, setCopied] = createSignal<"code" | "secret" | null>(null);
  const flash = (what: "code" | "secret") => { setCopied(what); setTimeout(() => setCopied(null), 1500); };
  const tag = () => identity.me()?.gamertag ?? "";
  return (
    <Plate small={s.isNew() ? "new account" : tag()} title={s.isNew() ? "set up recovery" : "recovery"} main>
      <div class="dx-recovery-body">
        <Show when={s.phase() === "loading"}><p class="dx-note">Loading…</p></Show>
        <Show when={s.phase() === "error"}><p class="dx-error" role="alert">{s.error()}</p></Show>
        <Show when={s.phase() === "none"}>
          <p class="dx-signup-intro">No recovery yet. Set it up so you can sign in on a new device.</p>
          <p class="dx-recovery-actions">
            <button type="button" class="dx-go" disabled={s.busy()} onClick={() => s.setupOrReset()}>{s.busy() ? "setting up" : "set up recovery"}</button>
          </p>
          <Show when={s.error()}><p class="dx-error" role="alert">{s.error()}</p></Show>
        </Show>
        <Show when={s.phase() === "ready" && s.rec()}>
          {(r) => (
            <>
              <Show when={s.isNew()}><p class="dx-signup-intro">Save this now. It's how you get back in if you lose this browser.</p></Show>
              <div class="dx-recovery-grid" classList={{ "dx-recovery-grid--qr": s.showSetup() && !!r().otpauth }}>
                <dl>
                  <dt>code</dt>
                  <dd>
                    <span class="dx-copy">
                      <code class="dx-rcode-inline">{r().code}</code>
                      <button type="button" class="dx-forget" onClick={() => void s.copyCode().then((ok) => ok && flash("code"))}>{copied() === "code" ? "copied" : "copy"}</button>
                    </span>
                  </dd>
                  <Show when={s.showSetup()}>
                    <dt>secret</dt>
                    <dd>
                      <span class="dx-copy">
                        <code class="dx-secret">{groupSecret(r().secret)}</code>
                        <button type="button" class="dx-forget" onClick={() => void copyText(r().secret).then((ok) => ok && flash("secret"))}>{copied() === "secret" ? "copied" : "copy"}</button>
                      </span>
                    </dd>
                    <Show when={r().otpauth}>
                      <dt>app</dt>
                      <dd><a class="dx-forget" href={r().otpauth}>open in authenticator</a></dd>
                    </Show>
                  </Show>
                </dl>
                <Show when={s.showSetup() && r().otpauth}>
                  <QrCode text={r().otpauth} label={`authenticator setup for ${tag()}`} />
                </Show>
              </div>
              <p class="dx-note">{NEW_DEVICE_NOTE}</p>
              <Show when={s.showSetup()}>
                <p class="dx-note">Scan the code with an authenticator app, or type the secret into it.</p>
              </Show>
              <Show when={s.error()}><p class="dx-error" role="alert">{s.error()}</p></Show>
              <Show
                when={s.confirming()}
                fallback={
                  <p class="dx-recovery-actions">
                    <Show when={s.isNew()}><button type="button" class="dx-go" onClick={() => props.onDone()}>done</button></Show>
                    <button type="button" class="dx-forget" onClick={() => s.toggleSetup()}>{s.showSetup() ? "hide authenticator setup" : "show authenticator setup"}</button>
                    <Show when={!s.isNew()}><button type="button" class="dx-forget" onClick={() => s.setupOrReset()}>reset recovery</button></Show>
                  </p>
                }
              >
                <p class="dx-error">Reset recovery? The current secret stops working, in every authenticator app it's in.</p>
                <p class="dx-recovery-actions">
                  <button type="button" class="dx-go" disabled={s.busy()} onClick={() => s.setupOrReset()}>{s.busy() ? "resetting" : "reset"}</button>
                  <button type="button" class="dx-forget" onClick={() => s.cancelReset()}>cancel</button>
                </p>
              </Show>
            </>
          )}
        </Show>
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
      setErr(x instanceof RecoverError ? x.message : `couldn't recover: ${(x as Error).message}`);
    } finally { setBusy(false); }
  }
  return (
    <Plate small="new device" title="recover account" main>
      <p class="dx-signup-intro">Your gamertag and a recovery code.</p>
      <form id="dash-recover" class="dx-form" onSubmit={submit}>
        <label for="dash-recover-tag">gamertag</label>
        <input id="dash-recover-tag" value={tag()} maxLength={15} onInput={(e) => setTag(e.currentTarget.value)} autocomplete="username" spellcheck={false} />
        <label for="dash-recover-code">code</label>
        <input id="dash-recover-code" value={code()} maxLength={9} inputmode="numeric" autocomplete="one-time-code" onInput={(e) => setCode(e.currentTarget.value)} placeholder="123-456" />
        <button type="submit" class="dx-go" disabled={busy()}>{busy() ? "recovering" : "recover"}</button>
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
        <p class="dx-note">
          The code is on the recovery screen of a device that's still signed in, or in your authenticator app.
          This browser gets a new key for the account; to play, choose “download for h2” again afterwards.
        </p>
      </form>
    </Plate>
  );
}
