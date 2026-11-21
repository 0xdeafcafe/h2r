// The usage screen's panels, the same for the Live admin's real numbers
// (/dash/admin/usage) and the style page's fixed ones (/dash/admin/style):
// six tiles, uploads by kind per day, what is stored, events, messages and
// players per day, events by type, and the accounts using the most.

import { Show } from "solid-js";
import { A } from "@solidjs/router";
import { DenseTable, Legend, Panel, RankBars, SegBar, SeriesTable, StatTile, TimeChart } from "./Deck";
import { bytes, compact, shortDay, type Day, type Series } from "./chart";
import { seriesColor } from "./palette";
import { sortCol, tileValue, type UsageView } from "./usageView";
import { UPLOAD_KINDS, type AccountUsage } from "../admin/usage";
import { dashboardPlayerHref } from "../routes";

export default function UsageBoard(props: {
  view: UsageView;
  range: number;
  accounts: readonly AccountUsage[];
  accountsNote: string;
  sort: string;
  showRequestAccounting?: boolean;
  savesStorageMeasured?: boolean;
  accountSavesStorageMeasured?: boolean;
}) {
  const v = () => props.view;
  const name = (a: AccountUsage) => a.gamertag ?? a.xuid;
  const storedRows = () => props.savesStorageMeasured === false ? v().stored.filter((r) => r.key !== "saves") : v().stored;
  const storedTotal = () => storedRows().reduce((sum, r) => sum + r.value, 0);
  const accountStored = (a: AccountUsage) => props.accountSavesStorageMeasured === false ? a.stored.events + a.stored.content : a.stored_total;
  return (
    <>
      <Show when={props.showRequestAccounting}>
        <Panel title="Live API requests" meta={v().requests.total?.toLocaleString("en-US")}
          table={v().requests.days.length ? () => <SeriesTable days={v().requests.days} series={[v().requests.daily]} format={(n) => n.toLocaleString("en-US")} label="Counted Live API requests per UTC day" /> : undefined}>
          <Show when={v().requests.total !== null} fallback={<p class="dk-empty">No request measurements for this date range.</p>}>
            <Show when={v().requests.measuredFrom}>{(day) => <p class="dk-note">Request records begin {day()} (UTC, within retained history).</p>}</Show>
            <p class="dk-note">Counts Live API requests admitted by the rate limiter, including WebSocket upgrades. Cloudflare’s edge totals also cover traffic such as cached responses and static files.</p>
          </Show>
        </Panel>
      </Show>

      <Show when={v().hasActivity} fallback={<p class="dk-span-12 dk-state">No game or account activity recorded for this date range.</p>}>
        {v().tiles.map((t) => (
          <StatTile label={t.label} value={tileValue(t, bytes)} delta={t.delta ?? undefined} upIsGood={t.upIsGood}
            trend={t.trend.some((n) => n > 0) ? t.trend : undefined}
            sub={t.delta ? `vs previous ${props.range} days` : `last ${props.range} days`} />
        ))}
      </Show>

      <UsagePlot title="recorded uploads" meta="per day, by kind" span={8} days={v().days} series={v().upload}
        kind="stack" format={bytes} base={1024} label="Data uploaded per day by kind" empty="No uploads recorded in this range." />

      <Panel title={props.savesStorageMeasured === false ? "measured storage" : "stored"} meta={bytes(storedTotal())} span={4}>
        <RankBars rows={storedRows()} format={bytes} label="Bytes stored by kind" />
        <p class="dk-note">Logical bytes from the event log, published map packages, and cloud saves across all accounts.</p>
        <Show when={props.savesStorageMeasured === false}>
          <p class="dk-note">Cloud save storage could not be measured. This subtotal covers the event log and published map packages.</p>
        </Show>
      </Panel>

      <UsagePlot title="events written" meta="per day" span={4} days={v().days} series={v().events}
        kind="area" format={compact} height={120} label="Events written per day" empty="No events recorded in this range." />
      <UsagePlot title="messages sent" meta="per day" span={4} days={v().days} series={v().messages}
        kind="line" format={compact} height={120} label="Messages sent per day by kind" empty="No friend requests or game invites recorded in this range." />
      <UsagePlot title="players" meta="per day" span={4} days={v().days} series={v().people}
        kind="line" format={compact} height={120} label="Active accounts and sign-ins per day" empty="No account activity or sign-ins recorded in this range." />

      <Panel title="events by type" meta={`last ${props.range} days`} span={4}>
        <Show when={v().eventTypes.length} fallback={<p class="dk-empty">No retained events in this range.</p>}>
          <RankBars rows={v().eventTypes} format={compact} label="Events written by type" />
        </Show>
      </Panel>

      <Panel title="top accounts" meta={props.accountsNote} span={8}>
        <Show when={props.accounts.length} fallback={<p class="dx-page-empty dk-empty">No account usage in this range.</p>}>
          <Show when={props.accountSavesStorageMeasured === false}>
            <p class="dk-note">Stored bytes in this list cover the event log and published map packages. Cloud save storage could not be measured.</p>
          </Show>
          <Legend items={UPLOAD_KINDS.map((k, i) => ({ label: k.label, color: seriesColor(i) }))} />
          <DenseTable<AccountUsage>
            label="Accounts using the most" rows={props.accounts} rowKey={(a) => a.xuid} initialSort={{ col: sortCol(props.sort), dir: "desc" }}
            cols={[
              { key: "tag", label: "gamertag", value: name, render: (a) => (a.gamertag ? <A href={dashboardPlayerHref(a.gamertag)}>{a.gamertag}</A> : <span class="dk-dim">{a.xuid}</span>) },
              { key: "up", label: "uploaded", num: true, bar: true, value: (a) => a.uploaded, render: (a) => bytes(a.uploaded) },
              {
                key: "mix", label: "by kind", value: (a) => a.up.frames / Math.max(1, a.uploaded), class: "dk-col-seg",
                render: (a) => <SegBar parts={UPLOAD_KINDS.map((k) => ({ label: k.label, value: a.up[k.key] }))} format={bytes} label={`${name(a)} uploads by kind`} />,
              },
              { key: "stored", label: "stored", num: true, value: accountStored, render: (a) => bytes(accountStored(a)) },
              { key: "events", label: "events", num: true, value: (a) => a.events, render: (a) => compact(a.events) },
              { key: "msgs", label: "messages", num: true, value: (a) => a.messages_total, render: (a) => compact(a.messages_total) },
              { key: "games", label: "games", num: true, value: (a) => a.games, render: (a) => compact(a.games) },
              { key: "reports", label: "reported", num: true, value: (a) => a.reports_received, render: (a) => compact(a.reports_received) },
              { key: "days", label: "active days", num: true, value: (a) => a.active_days },
              { key: "seen", label: "last active", value: (a) => a.last_active ?? "", render: (a) => (a.last_active ? shortDay(a.last_active) : "—") },
            ]}
          />
        </Show>
      </Panel>
    </>
  );
}

function UsagePlot(props: {
  title: string;
  meta: string;
  span: 4 | 8;
  days: Day[];
  series: Series[];
  kind: "stack" | "area" | "line";
  format: (n: number) => string;
  label: string;
  empty: string;
  height?: number;
  base?: 1024;
}) {
  const measured = () => props.series.some((series) => series.values.some((n) => n > 0));
  return (
    <Panel title={props.title} meta={props.meta} span={props.span}
      table={measured() ? () => <SeriesTable days={props.days} series={props.series} format={props.format} label={props.label} /> : undefined}>
      <Show when={measured()} fallback={<p class="dk-empty">{props.empty}</p>}>
        <TimeChart days={props.days} series={props.series} kind={props.kind} format={props.format}
          base={props.base} height={props.height} label={props.label} />
      </Show>
    </Panel>
  );
}
