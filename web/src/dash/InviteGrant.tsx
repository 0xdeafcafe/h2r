import { Show, createEffect, createSignal } from "solid-js";
import type { InviteGrant } from "../api/types";

export const MY_INVITES = "/dash/server/my-invites";

export function InviteSomeone(props: { grant: InviteGrant | null; go: () => void }) {
  return (
    <Show when={props.grant}>
      <a class="dx-forget" href={MY_INVITES} onClick={(e) => { e.preventDefault(); props.go(); }}>invite someone</a>
    </Show>
  );
}

export function InviteGrantControl(props: {
  admin: boolean;
  grant: InviteGrant | null;
  write: (grant: InviteGrant | null) => Promise<void>;
}) {
  const [saved, setSaved] = createSignal(props.grant);
  const [draft, setDraft] = createSignal(String(props.grant?.open_cap ?? ""));
  const [busy, setBusy] = createSignal(false);
  const [err, setErr] = createSignal<string | null>(null);
  createEffect(() => {
    setSaved(props.grant);
    setDraft(String(props.grant?.open_cap ?? ""));
  });

  const cap = () => draft().trim() ? Number(draft()) : null;
  const valid = () => {
    const n = cap();
    return n === null || (/^[0-9]+$/.test(draft().trim()) && n >= 1 && n <= 0xffffffff);
  };
  async function save(enabled: boolean) {
    if (busy() || !props.admin || (enabled && !valid())) return;
    const before = props.grant;
    const next = enabled ? { open_cap: cap() } : null;
    setBusy(true); setErr(null);
    try {
      await props.write(next);
      if (props.grant === before) {
        setSaved(next);
        setDraft(String(next?.open_cap ?? ""));
      }
    } catch (e) { setErr((e as Error).message); }
    finally { setBusy(false); }
  }

  return (
    <Show when={props.admin}>
      <div class="dx-invite-grant">
        <button
          type="button" class="dx-forget" role="switch" aria-label="can invite" aria-checked={!!saved()}
          data-dash-native disabled={busy() || (!saved() && !valid())} onClick={() => void save(!saved())}
        >{`can invite · ${saved() ? "on" : "off"}`}</button>
        <form class="dx-report-form" onSubmit={(e) => { e.preventDefault(); void save(true); }}>
          <label for="dash-invite-cap">open invite cap</label>
          <input
            id="dash-invite-cap" inputmode="numeric" placeholder="no cap" aria-invalid={!valid()}
            value={draft()} onInput={(e) => setDraft(e.currentTarget.value)} disabled={busy()}
          />
          <Show when={saved()}>
            <button type="submit" class="dx-forget" data-dash-native disabled={busy() || !valid() || cap() === saved()?.open_cap}>save</button>
          </Show>
        </form>
        <Show when={!valid()}><p class="dx-error" role="alert">Enter a positive whole number or leave the cap blank.</p></Show>
        <Show when={err()}><p class="dx-error" role="alert">{err()}</p></Show>
      </div>
    </Show>
  );
}
