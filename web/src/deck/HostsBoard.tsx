import { Show } from "solid-js";
import { formatHostCost, formatHostUptime, hostEndReason, hostStateBadge } from "../admin/hosts";
import type { AdminHost, AdminHosts, EndedHost, HostState, ProviderKind, RunningHost } from "../lib/hosts";
import { DenseTable, Panel, SegBar, type Col } from "./Deck";
import { ACCENT, QUIET, STATUS } from "./palette";
import "../styles/deck-hosts.css";

const providerNames: Record<ProviderKind, string> = { local: "Local", cloudflare: "Cloudflare", fly: "Fly" };

export function StateBadge(props: { state: HostState }) {
  const badge = () => hostStateBadge(props.state);
  return <span class={`dk-host-badge dk-host-badge--${badge().tone}`}>{badge().label}</span>;
}

const baseColumns: readonly Col<AdminHost>[] = [
  { key: "lease", label: "lease", value: (host) => host.lease.id, render: (host) => <code>{host.lease.id}</code> },
  { key: "provider", label: "provider", value: (host) => providerNames[host.lease.provider] },
  { key: "region", label: "region", value: (host) => host.lease.region ?? "—" },
  { key: "class", label: "class", value: (host) => host.lease.class === "small" ? "Small" : "Large" },
  { key: "state", label: "state", value: (host) => host.state, render: (host) => <StateBadge state={host.state} /> },
];
const costColumn: Col<AdminHost> = {
  key: "cost", label: "cost", num: true, value: (host) => host.cost_usd, render: (host) => formatHostCost(host.cost_usd),
};

function Budget(props: { spent: number; limit: number }) {
  const parts = () => [
    { label: "spent", value: Math.min(props.spent, props.limit) },
    { label: "remaining", value: Math.max(0, props.limit - props.spent) },
  ];
  const reached = () => props.spent >= props.limit;
  return (
    <Panel title="Month to date" span={6}>
      <p class="dk-host-budget">
        <strong>{formatHostCost(props.spent)}</strong> of {formatHostCost(props.limit)} budget
        <Show when={reached()}> · {props.spent > props.limit ? "Over budget" : "Budget reached"}</Show>
      </p>
      <Show when={props.limit > 0}>
        <SegBar parts={parts()} colors={[reached() ? STATUS.critical : ACCENT, QUIET]} label="Monthly host budget" format={formatHostCost} />
      </Show>
    </Panel>
  );
}

export default function HostsBoard(props: { data: AdminHosts; busy: boolean; stopping: string | null; onStop: (lease: string) => void }) {
  return (
    <>
      <Budget spent={props.data.month_to_date_usd} limit={props.data.budget_usd} />
      <Panel title="Running hosts" meta={String(props.data.running.length)}>
        <Show when={props.data.running.length} fallback={<p class="dk-note">No hosts running.</p>}>
          <DenseTable<RunningHost>
            label="Running hosts" rows={props.data.running} rowKey={(host) => host.lease.id} initialSort={{ col: "uptime", dir: "desc" }}
            cols={[
              ...baseColumns,
              { key: "players", label: "players", num: true, value: (host) => host.players, render: (host) => `${host.players} / ${host.lease.max_players}` },
              { key: "uptime", label: "uptime", num: true, value: (host) => host.uptime_ms, render: (host) => formatHostUptime(host.uptime_ms) },
              { ...costColumn, label: "cost so far" },
              {
                key: "actions", label: "action", value: () => "",
                render: (host) => (
                  <button
                    type="button" class="dk-pill dk-host-stop" disabled={props.busy}
                    aria-label={`Stop host ${host.lease.id}`} onClick={() => props.onStop(host.lease.id)}
                  >
                    {props.stopping === host.lease.id ? "Stopping…" : "Stop"}
                  </button>
                ),
              },
            ]}
          />
        </Show>
      </Panel>
      <Panel title="Recently ended" meta={String(props.data.recent.length)}>
        <Show when={props.data.recent.length} fallback={<p class="dk-note">No recently ended hosts.</p>}>
          <DenseTable<EndedHost>
            label="Recently ended hosts" rows={props.data.recent} rowKey={(host) => host.lease.id}
            cols={[
              ...baseColumns,
              { key: "reason", label: "reason", value: (host) => hostEndReason(host.reason) },
              costColumn,
            ]}
          />
        </Show>
      </Panel>
    </>
  );
}
