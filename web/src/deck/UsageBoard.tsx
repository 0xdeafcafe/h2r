// The usage screen's panels, the same for the Live admin's real numbers
// (/dash/admin/usage) and the style page's fixed ones (/dash/admin/style):
// six tiles, uploads by kind per day, what is stored, events, messages and
// players per day, events by type, and the accounts using the most.

import { Show } from "solid-js";
import { A } from "@solidjs/router";
import { DenseTable, Legend, Panel, RankBars, SegBar, SeriesTable, StatTile, TimeChart } from "./Deck";
import { bytes, compact, shortDay } from "./chart";
import { seriesColor } from "./palette";
import { sortCol, tileValue, type UsageView } from "./usageView";
import { UPLOAD_KINDS, type AccountUsage } from "../admin/usage";
import { dashboardPlayerHref } from "../routes";

export default function UsageBoard(props: { view: UsageView; range: number; accounts: readonly AccountUsage[]; accountsNote: string; sort: string }) {
  const v = () => props.view;
  const name = (a: AccountUsage) => a.gamertag ?? a.xuid;
  return (
    <>
      {v().tiles.map((t) => (
        <StatTile label={t.label} value={tileValue(t, bytes)} delta={t.delta ?? undefined} upIsGood={t.upIsGood} trend={t.trend}
          sub={t.delta ? `vs previous ${props.range} days` : `last ${props.range} days`} />
      ))}

      <Panel title="data uploaded" meta="per day, by kind" span={8}
        table={() => <SeriesTable days={v().days} series={v().upload} format={bytes} label="Data uploaded per day by kind" />}>
        <TimeChart days={v().days} series={v().upload} kind="stack" format={bytes} base={1024} label="Data uploaded per day by kind" />
      </Panel>

      <Panel title="stored" meta={bytes(v().storedTotal)} span={4}>
        <RankBars rows={v().stored} format={bytes} label="Bytes stored by kind" />
        <p class="dk-note">kept now, over every account. Live frames are not kept; the 1 Hz record of them is in the events.</p>
      </Panel>

      <Panel title="events written" meta="per day" span={4}
        table={() => <SeriesTable days={v().days} series={v().events} format={compact} label="Events written per day" />}>
        <TimeChart days={v().days} series={v().events} kind="area" format={compact} height={120} label="Events written per day" />
      </Panel>

      <Panel title="messages sent" meta="per day" span={4}
        table={() => <SeriesTable days={v().days} series={v().messages} format={compact} label="Messages sent per day by kind" />}>
        <TimeChart days={v().days} series={v().messages} kind="line" format={compact} height={120} label="Messages sent per day by kind" />
      </Panel>

      <Panel title="players" meta="per day" span={4}
        table={() => <SeriesTable days={v().days} series={v().people} format={compact} label="Active accounts and sign-ins per day" />}>
        <TimeChart days={v().days} series={v().people} kind="line" format={compact} height={120} label="Active accounts and sign-ins per day" />
      </Panel>

      <Panel title="events by type" meta={`last ${props.range} days`} span={4}>
        <RankBars rows={v().eventTypes} format={compact} label="Events written by type" />
      </Panel>

      <Panel title="top accounts" meta={props.accountsNote} span={8}>
        <Show when={props.accounts.length} fallback={<p class="dx-page-empty dk-empty">No account usage in this range.</p>}>
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
              { key: "stored", label: "stored", num: true, value: (a) => a.stored_total, render: (a) => bytes(a.stored_total) },
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
