import { ApiError } from "../api/http";
import type { AdminHost, AdminHosts, EndReason, HostState } from "../lib/hosts";
import type { LocalAccount } from "../state/account";
import { adminGet, formatUptime } from "./server";
import { issueToken } from "./token";

export interface HostsAdmin {
  api: string;
  aud: string;
  me: LocalAccount;
}

function nonnegative(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function isHost(row: Partial<AdminHost> | null): row is AdminHost {
  if (!row?.lease) return false;
  const { lease } = row;
  return typeof lease.id === "string" && lease.id.length > 0 &&
    ["local", "cloudflare", "fly"].includes(lease.provider) &&
    (lease.region === null || typeof lease.region === "string") &&
    (lease.class === "small" || lease.class === "large") && nonnegative(lease.max_players) &&
    !!row.state && Object.hasOwn(stateBadges, row.state) && nonnegative(row.cost_usd);
}

export async function fetchHosts({ api, aud, me }: HostsAdmin, fetchFn: typeof fetch = fetch): Promise<AdminHosts> {
  const body = await adminGet(api, "/admin/hosts", aud, me, fetchFn) as Partial<AdminHosts> | null;
  if (!Array.isArray(body?.running) || !Array.isArray(body.recent) ||
    !body.running.every((host) => isHost(host) && nonnegative(host.players) && nonnegative(host.uptime_ms)) ||
    !body.recent.every((host) => isHost(host) && Object.hasOwn(endReasons, host.reason)) ||
    !nonnegative(body.month_to_date_usd) || (body.budget_usd !== null && !nonnegative(body.budget_usd))) {
    throw new Error("The server sent an invalid host inventory.");
  }
  return {
    running: body.running,
    recent: body.recent,
    month_to_date_usd: body.month_to_date_usd,
    budget_usd: body.budget_usd,
  };
}

/** The stop response is 204. Resolve false when the admin cancels the confirmation. */
export async function stopHost(
  { api, aud, me }: HostsAdmin,
  lease: string,
  confirm: (message: string) => boolean,
  fetchFn: typeof fetch = fetch,
): Promise<boolean> {
  if (!confirm(`Stop host ${lease}? This disconnects any players in the match.`)) return false;

  const token = await issueToken(me, aud, "admin.write");
  const r = await fetchFn(`${api}/admin/hosts/${encodeURIComponent(lease)}/stop`, {
    method: "POST",
    headers: { accept: "application/json", authorization: `Bearer ${token}` },
  });
  if (!r.ok) {
    const body = await r.json().catch(() => null) as { error?: string; message?: string } | null;
    throw new ApiError(r.status, body?.error ?? `http_${r.status}`, body?.message || r.statusText || `HTTP ${r.status}`);
  }
  return true;
}

const dollars = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });

export function formatHostCost(usd: number): string {
  if (!nonnegative(usd)) return "—";
  if (usd > 0 && usd < 0.01) return "<$0.01";
  return dollars.format(usd);
}

export function formatHostUptime(ms: number): string {
  return Number.isFinite(ms) ? formatUptime(ms / 1000) : "—";
}

const stateBadges = {
  requested: { label: "Requested", tone: "warning" },
  booting: { label: "Booting", tone: "warning" },
  joinable: { label: "Joinable", tone: "good" },
  playing: { label: "Playing", tone: "good" },
  ended: { label: "Ended", tone: "quiet" },
  failed: { label: "Failed", tone: "critical" },
} as const satisfies Record<HostState, { label: string; tone: string }>;

export function hostStateBadge(state: HostState) {
  return stateBadges[state];
}

const endReasons: Record<EndReason, string> = {
  match_over: "Match over",
  empty: "All players left",
  idle: "Idle",
  max_duration: "Time limit",
  stopped: "Stopped",
  boot_timeout: "Boot timed out",
  crashed: "Crashed",
  budget: "Budget limit",
};

export function hostEndReason(reason: EndReason | null | undefined): string {
  return reason ? endReasons[reason] ?? "—" : "—";
}
