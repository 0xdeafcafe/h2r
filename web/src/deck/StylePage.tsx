// /dash/admin/style: the dense dashboard's components on fixed data, laid
// out the way the admin usage screen will be (docs/live/web-boundary.md
// §3). Nothing is fetched and nothing needs signing in.

import { createMemo, createSignal } from "solid-js";
import { DeckPage, DenseTable, Panel, Presets, RankBars, SegBar, SeriesTable, StatTile, TimeChart, Legend } from "./Deck";
import { bytes, compact, shortDay, topN } from "./chart";
import { UPLOAD_KINDS, uploaded, usageMock, type AccountUsage } from "./mock";
import { tileValue, usageView } from "./usageView";
import { seriesColor } from "./palette";

const LAST = "2026-11-18";
const DATA = usageMock(LAST, 60);
const RANGES = [
  { value: 7, label: "7 days" },
  { value: 14, label: "14 days" },
  { value: 30, label: "30 days" },
] as const;

export default function StylePage() {
  const [range, setRange] = createSignal<number>(30);
  const v = createMemo(() => usageView(DATA, LAST, range()));
  const top = createMemo(() => topN(DATA.accounts, 12, uploaded));
  const playerHref = (a: AccountUsage) => `/stats/players/${encodeURIComponent(a.gamertag)}`;
  return (
    <DeckPage
      trail={[["dashboard", "/"], ["server", "/dash/server"], ["admin"], ["style"]]}
      title="usage"
      back="/dash/server"
      filters={<>
        <Presets label="range" options={RANGES} value={range()} onPick={setRange} />
        <span class="dk-filters-note">fixed sample data · not this server</span>
      </>}
    >
      {v().tiles.map((t) => (
        <StatTile label={t.label} value={tileValue(t, bytes)} delta={t.delta} upIsGood={t.upIsGood} trend={t.trend} sub={`vs previous ${range()} days`} />
      ))}

      <Panel title="data uploaded" meta="per day, by kind" span={8}
        table={() => <SeriesTable days={v().days} series={v().upload} format={bytes} label="Data uploaded per day by kind" />}>
        <TimeChart days={v().days} series={v().upload} kind="stack" format={bytes} base={1024} label="Data uploaded per day by kind" />
      </Panel>

      <Panel title="stored" meta={bytes(v().storedTotal)} span={4}>
        <RankBars rows={v().stored} format={bytes} label="Bytes stored by kind" />
        <p class="dk-note">what the server keeps on disk now; co-op relay is never stored.</p>
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

      <Panel title="events by type" meta={`last ${range()} days`} span={4}>
        <RankBars rows={v().eventTypes} format={compact} label="Events written by type" />
      </Panel>

      <Panel title="top accounts" meta="by data uploaded, last 30 days" span={8}>
        <Legend items={UPLOAD_KINDS.map((k, i) => ({ label: k.label, color: seriesColor(i) }))} />
        <DenseTable<AccountUsage>
          label="Accounts using the most" rows={top()} rowKey={(a) => a.xuid} initialSort={{ col: "up", dir: "desc" }}
          cols={[
            { key: "tag", label: "gamertag", value: (a) => a.gamertag, render: (a) => <a href={playerHref(a)}>{a.gamertag}</a> },
            { key: "up", label: "uploaded", num: true, bar: true, value: uploaded, render: (a) => bytes(uploaded(a)) },
            {
              key: "mix", label: "by kind", value: (a) => a.bytes_up.frames / Math.max(1, uploaded(a)), class: "dk-col-seg",
              render: (a) => <SegBar parts={UPLOAD_KINDS.map((k) => ({ label: k.label, value: a.bytes_up[k.key] }))} format={bytes} label={`${a.gamertag} uploads by kind`} />,
            },
            { key: "stored", label: "stored", num: true, value: (a) => a.bytes_stored, render: (a) => bytes(a.bytes_stored) },
            { key: "events", label: "events", num: true, value: (a) => a.events, render: (a) => compact(a.events) },
            { key: "msgs", label: "messages", num: true, value: (a) => a.messages, render: (a) => compact(a.messages) },
            { key: "games", label: "games", num: true, value: (a) => a.games, render: (a) => compact(a.games) },
            { key: "hosted", label: "hosted", num: true, value: (a) => a.sessions_hosted, render: (a) => compact(a.sessions_hosted) },
            { key: "days", label: "active days", num: true, value: (a) => a.active_days },
            { key: "seen", label: "last seen", value: (a) => a.last_seen, render: (a) => shortDay(a.last_seen) },
          ]}
        />
      </Panel>
    </DeckPage>
  );
}
