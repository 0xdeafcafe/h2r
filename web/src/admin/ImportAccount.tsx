// Signing the browser in as an account the CLI holds: `h2 live account
// export <gamertag> --file f.json` writes the entry this accepts (or prints
// it for pasting). The key is checked against the server — the same check
// `h2 live account import` makes in the other direction — before it is kept
// in this browser's IndexedDB, where anyone with the profile can read it.

import { Show, createSignal } from "solid-js";
import { Panel } from "../stats/Layout";
import { identity } from "../state/identity";
import { live } from "../state/live";
import { importSeed } from "../state/account";
import { parseImportEntry } from "./validate";
import { adminState } from "./state";

interface KeyEntry {
  kid: string;
  jwk: { x?: string };
  valid_to: number | null;
}

export default function ImportAccount() {
  const [text, setText] = createSignal("");
  const [error, setError] = createSignal("");
  const [status, setStatus] = createSignal("");

  async function import_(): Promise<void> {
    setError("");
    setStatus("");
    const parsed = parseImportEntry(text());
    if (!parsed.ok) {
      setError(parsed.error);
      return;
    }
    try {
      for (const account of parsed.accounts) {
        if (account.server_id && account.server_id !== live.wellKnown()?.id) {
          setError(`${account.gamertag} is an account of server ${account.server_id}, not this one`);
          return;
        }
        // The key must be the account's current key here, exactly as the
        // CLI checks an import against us.
        const r = await fetch(
          `${live.server().api}/accounts/by-xuid/${encodeURIComponent(account.xuid)}/keys`,
          { headers: { accept: "application/json" } },
        );
        if (!r.ok) {
          setError(`the server does not know xuid ${account.xuid}`);
          return;
        }
        const keys = ((await r.json()) as { keys?: KeyEntry[] }).keys ?? [];
        const key = await importSeed(account.key, account.x);
        const current = keys.some(
          (k) => k.valid_to === null && k.kid === key.publicJwk.kid && k.jwk.x === account.x,
        );
        if (!current) {
          setError(`${account.gamertag}'s key is not current on this server`);
          return;
        }
        await identity.add({
          xuid: account.xuid,
          gamertag: account.gamertag,
          id: account.id,
          server: live.server().base,
          server_id: live.wellKnown()?.id ?? "",
          kid: key.publicJwk.kid,
          key: account.key,
          x: account.x,
        });
        setStatus(`signed in as ${account.gamertag}`);
        setText("");
        adminState.reset();
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function pickFile(file: File | undefined): Promise<void> {
    if (!file) return;
    setText(await file.text());
  }

  return (
    <Panel
      title="Import an account"
      sub="the file `h2 live account export` writes (or its text, pasted)"
    >
      <p class="adm-note">
        The private key is stored in this browser, readable by anyone with access to its profile —
        the same warning sign-up gives.
      </p>
      <textarea
        class="adm-paste"
        rows={6}
        placeholder='{"xuid": "0009…", "gamertag": "afr", "key": "…", …}'
        value={text()}
        onInput={(e) => setText(e.currentTarget.value)}
      />
      <div class="adm-row">
        <label class="adm-file">
          or choose the exported file
          <input type="file" accept=".json,application/json" onChange={(e) => void pickFile(e.currentTarget.files?.[0])} />
        </label>
        <button class="s-btn" disabled={!text().trim()} onClick={() => void import_()}>
          Import and sign in
        </button>
      </div>
      <Show when={error()}>
        <p class="adm-error">{error()}</p>
      </Show>
      <Show when={status()}>
        <p class="adm-ok">{status()}</p>
      </Show>
    </Panel>
  );
}
