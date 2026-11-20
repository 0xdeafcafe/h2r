import type { Xuid } from "../api/types";

export type ProviderKind = "local" | "cloudflare" | "fly";
export type HostClass = "small" | "large";
export type Transport = "udp" | "tunnel";
export type HostState = "requested" | "booting" | "joinable" | "playing" | "ended" | "failed";
export type EndReason = "match_over" | "empty" | "idle" | "max_duration" | "stopped" | "boot_timeout" | "crashed" | "budget";

export interface HostLease {
  id: string;
  assignment_id: string;
  provider: ProviderKind;
  region: string | null;
  class: HostClass;
  map: string;
  map_id: string | null;
  variant: string | null;
  hopper: number | null;
  max_players: number;
  roster: Xuid[];
  bots: string[];
  transport: Transport;
  idle_timeout_s: number;
  max_match_s: number;
  created_ms: number;
}

export interface HostEndpoint {
  transport: Transport;
  udp: string | null;
  tunnel_url: string | null;
  xnaddr: string;
  session_id: string | null;
}

export interface HostView {
  lease: HostLease;
  state: HostState;
  endpoint: HostEndpoint | null;
}

export interface HostRequested { lease: HostLease }
export interface HostBooting { lease_id: string; provider_ref: string }
export interface HostReady { lease_id: string; endpoint: HostEndpoint; boot_ms: number }
export interface HostPlaying { lease_id: string; players: number }
export interface HostEnded { lease_id: string; reason: EndReason; ran_ms: number; players_peak: number }

export interface AdminHost {
  lease: Pick<HostLease, "id" | "provider" | "region" | "class" | "max_players">;
  state: HostState;
  cost_usd: number;
}

export interface RunningHost extends AdminHost {
  players: number;
  uptime_ms: number;
}

export interface EndedHost extends AdminHost {
  reason: EndReason;
}

export interface AdminHosts {
  running: RunningHost[];
  recent: EndedHost[];
  month_to_date_usd: number;
  budget_usd: number | null;
}

/** Public game and session payloads can omit these fields for peer hosts and older records. */
export interface DedicatedHostFields {
  host_lease?: string | null;
  dedicated_region?: string | null;
}

export function dedicatedHostLabel(value: DedicatedHostFields): string | null {
  if (typeof value.host_lease !== "string" || !value.host_lease.trim()) return null;
  const region = typeof value.dedicated_region === "string" ? value.dedicated_region.trim() : "";
  return region ? `Dedicated (${region})` : "Dedicated";
}
