// Small shared pieces: avatar, presence badge, verify mark, mascot, empty state.

import { Show, type JSX } from "solid-js";
import { A } from "@solidjs/router";
import { hueFor, initials } from "../lib/format";
import type { AccountState } from "../state/reducer";
import type { VerifyResult } from "../state/verify";
import { live } from "../state/live";

export function Avatar(props: { name: string; size?: "s" | "m" | "l"; online?: boolean }) {
  return (
    <span
      class={`avatar avatar--${props.size ?? "m"}`}
      classList={{ "avatar--online": !!props.online }}
      style={{ "--avatar-hue": String(hueFor(props.name)) }}
      aria-hidden="true"
    >
      {initials(props.name)}
    </span>
  );
}

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
        {(gid) => <A href={`/games/${gid()}`}>{PRESENCE_LABEL.playing}</A>}
      </Show>
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
  const counts = () => {
    const c = { verified: 0, bad: 0, other: 0, pending: 0 };
    for (const p of props.positions()) {
      const r = live.verify[p];
      if (!r) c.pending++;
      else if (r.status === "verified") c.verified++;
      else if (r.status === "bad") c.bad++;
      else c.other++;
    }
    return c;
  };
  return (
    <span class="verify-summary" classList={{ "verify-summary--bad": counts().bad > 0 }}>
      <Show
        when={counts().bad === 0}
        fallback={<><span class="verify__icon">!</span> {counts().bad} event{counts().bad === 1 ? "" : "s"} failed signature checks</>}
      >
        <span class="verify__icon">✓</span>
        {counts().verified} signed event{counts().verified === 1 ? "" : "s"} verified
        <Show when={counts().pending}> · {counts().pending} checking</Show>
        <Show when={counts().other}> · {counts().other} unchecked</Show>
      </Show>
    </span>
  );
}

/** The mascot: a small round friend. Pure CSS/SVG, ours. */
export function Mascot(props: { mood?: "happy" | "sleepy" | "wow"; size?: number }) {
  const s = () => props.size ?? 96;
  return (
    <svg class={`mascot mascot--${props.mood ?? "happy"}`} width={s()} height={s()} viewBox="0 0 100 100" aria-hidden="true">
      <ellipse cx="50" cy="92" rx="26" ry="5" class="mascot__shadow" />
      <path class="mascot__body" d="M50 12c22 0 36 16 36 38s-14 36-36 36S14 72 14 50 28 12 50 12z" />
      <g class="mascot__eyes">
        {props.mood === "sleepy" ? (
          <>
            <path d="M33 50q5 4 10 0" class="mascot__line" />
            <path d="M57 50q5 4 10 0" class="mascot__line" />
          </>
        ) : (
          <>
            <circle cx="38" cy="48" r="5" class="mascot__eye" />
            <circle cx="62" cy="48" r="5" class="mascot__eye" />
          </>
        )}
      </g>
      <circle cx="30" cy="60" r="5" class="mascot__cheek" />
      <circle cx="70" cy="60" r="5" class="mascot__cheek" />
      {props.mood === "wow"
        ? <ellipse cx="50" cy="64" rx="5" ry="6" class="mascot__eye" />
        : <path d="M43 62q7 6 14 0" class="mascot__line" />}
    </svg>
  );
}

export function Empty(props: { title: string; children?: JSX.Element; mood?: "happy" | "sleepy" | "wow" }) {
  return (
    <div class="empty">
      <Mascot mood={props.mood ?? "sleepy"} />
      <h3>{props.title}</h3>
      <Show when={props.children}><p>{props.children}</p></Show>
    </div>
  );
}
