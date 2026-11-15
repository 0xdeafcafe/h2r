// The message-of-the-day editor: the header string and the message blocks
// the game shows under it (one picked at random every 15 s on the
// matchmaking screen), with a preview in the stats site's style. Saves sign
// a token with the signed-in account's key; the server republishes
// message_of_the_day.dat with the same text in all nine languages.

import { For, Show, createSignal, onMount } from "solid-js";
import { Block, Panel } from "../stats/Layout";
import { getMotd, putMotd } from "./api";
import { MOTD_MAX_MESSAGES, MOTD_MAX_UNITS, motdError, type MotdDoc } from "./validate";

export default function MotdEditor() {
  const [doc, setDoc] = createSignal<MotdDoc | null>(null);
  const [error, setError] = createSignal("");
  const [status, setStatus] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  onMount(() => void load());

  async function load(): Promise<void> {
    setBusy(true);
    setError("");
    try {
      const motd = await getMotd();
      setDoc(motd ?? { header: "", messages: [""] });
      setStatus(motd ? "" : "no message of the day published yet; saving publishes the first one");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const bad = () => (doc() ? motdError(doc()!) : null);

  async function save(): Promise<void> {
    const current = doc();
    if (!current || bad()) return;
    setBusy(true);
    setError("");
    setStatus("");
    try {
      const reply = await putMotd(current);
      setStatus(
        `published ${reply.published.size} bytes (sha256 ${reply.published.sha256.slice(0, 12)}…). ` +
          "The game picks it up at its next Xbox Live sign-in (sign out and back in, or restart the game), not mid-session.",
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Show when={doc()}>
      {(d) => (
        <div class="adm-motd">
          <Panel
            title="Message of the Day"
            sub={`the header shows on the Live main menu; one of up to ${MOTD_MAX_MESSAGES} messages, picked at random, on the matchmaking screen every 15 s`}
            right={
              <button class="s-btn" disabled={busy() || !!bad()} onClick={() => void save()}>
                {busy() ? "…" : "Publish"}
              </button>
            }
          >
            <label class="adm-field">
              <span>Header</span>
              <input
                value={d().header}
                maxLength={MOTD_MAX_UNITS}
                onInput={(e) => setDoc({ header: e.currentTarget.value, messages: d().messages })}
              />
            </label>
            <ul class="adm-messages">
              <For each={d().messages}>
                {(m, i) => (
                  <li>
                    <input
                      value={m}
                      maxLength={MOTD_MAX_UNITS}
                      onInput={(e) => {
                        const messages = [...d().messages];
                        messages[i()] = e.currentTarget.value;
                        setDoc({ header: d().header, messages });
                      }}
                    />
                    <button
                      class="adm-x"
                      disabled={d().messages.length <= 1}
                      aria-label={`Remove message ${i() + 1}`}
                      onClick={() =>
                        setDoc({
                          header: d().header,
                          messages: d().messages.filter((_, j) => j !== i()),
                        })
                      }
                    >
                      ×
                    </button>
                  </li>
                )}
              </For>
            </ul>
            <button
              class="s-btn"
              disabled={d().messages.length >= MOTD_MAX_MESSAGES}
              onClick={() =>
                setDoc({ header: d().header, messages: [...d().messages, ""] })
              }
            >
              Add a message
            </button>
            <Show when={bad()}>
              {(e) => <p class="adm-error">{e()}</p>}
            </Show>
            <Show when={error()}>
              <p class="adm-error">{error()}</p>
            </Show>
            <Show when={status()}>
              <p class="adm-ok">{status()}</p>
            </Show>
          </Panel>
          <Block title="Preview">
            <div class="adm-preview">
              <p class="adm-preview-header">{d().header || "(no header)"}</p>
              <p class="adm-preview-message">
                {d().messages.find((m) => m.trim()) || "(no message)"}
              </p>
              <p class="adm-preview-note">
                the matchmaking screen re-picks from {d().messages.length}{" "}
                {d().messages.length === 1 ? "message" : "messages"} every 15 seconds
              </p>
              <p class="adm-preview-note">
                a published change reaches a console at its next Xbox Live sign-in (sign out and
                back in, or restart the game), not mid-session
              </p>
            </div>
          </Block>
        </div>
      )}
    </Show>
  );
}
