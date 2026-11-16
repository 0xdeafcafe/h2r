// Small shared pieces: presence badge, verify marks.

import { Show } from "solid-js";
import { A } from "@solidjs/router";
import type { AccountState } from "../state/reducer";
import { countResults, summaryLine, type VerifyResult } from "../state/verify";
import { live } from "../state/live";

export type PresenceKind = "playing" | "online" | "away" | "offline";

export function presenceKind(a: AccountState | undefined): PresenceKind {
  if (!a || !a.online) return "offline";
  if (a.presence?.game_id) return "playing";
  const s = (a.presence?.state ?? "").toLowerCase();
  if (s.includes("away") || s.includes("idle") || s.includes("busy")) return "away";
  if (s.includes("game") || s.includes("play")) return "playing";
  return "online";
}

const PRESENCE_LABEL: Record<PresenceKind, string> = {
  playing: "in a game",
  online: "online",
  away: "away",
  offline: "offline",
};

export function PresenceBadge(props: { account: AccountState | undefined }) {
  const kind = () => presenceKind(props.account);
  return (
    <span class={`presence presence--${kind()}`}>
      <span class="presence__dot" />
      <Show when={kind() === "playing" && props.account?.presence?.game_id} fallback={PRESENCE_LABEL[kind()]}>
        {(gid) => <A href={`/stats/games/${gid()}`}>{PRESENCE_LABEL.playing}</A>}
      </Show>
      <Show when={kind() !== "offline" && props.account?.presence?.activity}>{(a) => <span class="presence__activity"> ({a()})</span>}</Show>
    </span>
  );
}

export function VerifyMark(props: { position: number; compact?: boolean }) {
  const r = (): VerifyResult | undefined => live.verify[props.position];
  const label = () => {
    const x = r();
    if (!x) return "checking signature";
    switch (x.status) {
      case "verified": return "signature verified";
      case "bad": return `bad signature: ${x.reason ?? "does not verify"}`;
      case "unsigned": return "not signed";
      default: return `could not check: ${x.reason ?? "unknown"}`;
    }
  };
  return (
    <span class={`verify verify--${r()?.status ?? "pending"}`} title={label()} aria-label={label()}>
      <span class="verify__icon" aria-hidden="true">
        {{ verified: "✓", bad: "!", unsigned: "–", unknown: "?" }[r()?.status ?? "unknown"] ?? "…"}
      </span>
      <Show when={!props.compact}>
        <span class="verify__text">{{ verified: "verified", bad: "tampered?", unsigned: "unsigned", unknown: "unchecked" }[r()?.status ?? "unknown"] ?? "checking"}</span>
      </Show>
    </span>
  );
}

/** Summary of signature checks over a set of positions. */
export function VerifySummary(props: { positions: () => number[] }) {
  const counts = () => countResults(props.positions(), live.verify);
  return (
    <span class="verify-summary" classList={{ "verify-summary--bad": counts().bad > 0 }}>
      <span class="verify__icon">{counts().bad ? "!" : counts().verified ? "✓" : "·"}</span> {summaryLine(counts())}
    </span>
  );
}
