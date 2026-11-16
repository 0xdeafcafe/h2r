// `/signin#once=…`: signs this browser in from the link `h2 play --live-local`
// prints (./redeem.ts). The fragment is cleared before anything else runs.

import { Match, Switch, createSignal, onMount } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import StatsLayout, { Panel, Title } from "../stats/Layout";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { fetchWellKnown, targetFor } from "../api/server";
import type { LocalAccount } from "../state/account";
import { adminState } from "../admin/state";
import { clearOnce, redeem } from "./redeem";
import "./signin.css";

type Step =
  | { s: "working" }
  | { s: "none" }
  | { s: "error"; error: string }
  | { s: "switch"; account: LocalAccount; admin: boolean; current: string };

export default function SignIn() {
  // Before the first render: the token leaves the address bar and history.
  const once = typeof location !== "undefined" ? clearOnce(location, history) : null;
  const navigate = useNavigate();
  const [step, setStep] = createSignal<Step>(once ? { s: "working" } : { s: "none" });

  async function keep(account: LocalAccount, admin: boolean): Promise<void> {
    await identity.add(account);
    adminState.reset();
    navigate(admin ? "/stats/admin" : "/", { replace: true });
  }

  onMount(async () => {
    if (!once) return;
    try {
      // The link names this server: talk to it, not a remembered other one.
      const base = location.origin;
      const wk = await fetchWellKnown(base);
      if (live.server().base !== base) await live.useServer(base);
      const r = await redeem(once, { base, api: targetFor(base, wk).api, serverId: wk.id });
      if (!r.ok) {
        setStep({ s: "error", error: r.error });
        return;
      }
      const me = identity.me();
      if (me && me.xuid !== r.account.xuid) {
        setStep({ s: "switch", account: r.account, admin: r.admin, current: me.gamertag });
        return;
      }
      await keep(r.account, r.admin);
    } catch (e) {
      setStep({ s: "error", error: e instanceof Error ? e.message : String(e) });
    }
  });

  return (
    <StatsLayout>
      <div class="s-full">
        <Title class="s-title--bar">Sign In</Title>
        <div class="s-pad">
          <Panel title="Sign in from h2 play" sub="a one-use link, for this machine only">
            <div class="si-body">
              <Switch>
                <Match when={step().s === "working"}>
                  <p>Signing you in…</p>
                </Match>
                <Match when={step().s === "none"}>
                  <p>
                    There's no sign-in link here. <code>h2 play --live-local</code> prints one each time it
                    starts; open it on this machine.
                  </p>
                  <p>
                    On a new device? <A href="/dash/signup/recover">Recover your account</A> with your gamertag and a recovery code.
                  </p>
                </Match>
                <Match when={(() => { const v = step(); return v.s === "error" ? v : null; })()}>
                  {(v) => (
                    <>
                      <p class="si-error">{v().error}</p>
                      <p><A href="/">Dashboard</A> · <A href="/dash/signup/recover">Recover an account with a code</A></p>
                    </>
                  )}
                </Match>
                <Match when={(() => { const v = step(); return v.s === "switch" ? v : null; })()}>
                  {(v) => (
                    <>
                      <p>
                        This browser is signed in as <b>{v().current}</b>. Switch to{" "}
                        <b>{v().account.gamertag}</b>, the account the game is playing as?
                        Its private key is stored in this browser, readable by anyone with access to
                        its profile, as with sign-up. {v().current} stays in this browser's accounts.
                      </p>
                      <p class="si-actions">
                        <button class="s-btn" onClick={() => void keep(v().account, v().admin)}>
                          Switch to {v().account.gamertag}
                        </button>
                        <button class="s-btn" onClick={() => navigate("/", { replace: true })}>
                          Stay {v().current}
                        </button>
                      </p>
                    </>
                  )}
                </Match>
              </Switch>
            </div>
          </Panel>
        </div>
      </div>
    </StatsLayout>
  );
}
