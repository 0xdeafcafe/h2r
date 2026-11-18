// The dense dashboard ("deck"): the dashboard's look for screens that carry
// a lot of numbers, the admin and ops screens (docs/live/web-boundary.md §3).
// Same fonts, greens and bevelled cut-corner panels as the console screens,
// but laid out as a scrolling page of panels instead of one scaled stage:
// metric tiles with sparklines, time charts with a crosshair, ranked bars,
// thin segmented bars and sortable tables. Every chart can swap for its
// table. The shaping is in ./chart.ts, the colours in ./palette.ts.

import { For, Index, Show, createMemo, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { A, useNavigate } from "@solidjs/router";
import {
  areaPath, columnPath, linePath, nearestIndex, niceTicks, peak, scale, shares, shortDay, sortRows, stack,
  type Day, type Delta, type Series, type SortDir,
} from "./chart";
import { ACCENT, BASELINE, GRID, QUIET, WELL, seriesColor } from "./palette";
import "../styles/deck.css";

// ---------- page and panels ----------

/** A deck page: the crumb trail and title, the way back (b / Escape), one
 *  filter row scoping everything under it, then the panel grid. */
export function DeckPage(props: { trail: [string, string?][]; title: string; back: string; filters?: JSX.Element; children: JSX.Element }) {
  const navigate = useNavigate();
  onMount(() => {
    const key = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
      if (e.key === "Escape" || e.key === "Backspace" || e.key === "b") { e.preventDefault(); navigate(props.back); }
    };
    addEventListener("keydown", key);
    onCleanup(() => removeEventListener("keydown", key));
  });
  return (
    <div class="dk">
      <header class="dk-head">
        <nav class="dk-trail" aria-label="Where you are">
          <For each={props.trail}>{([label, href]) => (href ? <A href={href}>{label}</A> : <span>{label}</span>)}</For>
        </nav>
        <h1>{props.title}</h1>
        <A class="dk-back" href={props.back} aria-label="Back"><i aria-hidden="true">b</i><span>back</span></A>
      </header>
      <Show when={props.filters}><div class="dk-filters" role="toolbar" aria-label="Filters">{props.filters}</div></Show>
      <main class="dk-grid">{props.children}</main>
    </div>
  );
}

export type Span = 2 | 3 | 4 | 6 | 8 | 12;

/** A bevelled panel. With `table`, a toggle in its header swaps the chart for its table. */
export function Panel(props: { title: string; meta?: string; span?: Span; table?: () => JSX.Element; children: JSX.Element }) {
  const [asTable, setAsTable] = createSignal(false);
  return (
    <section class={`dk-panel dk-span-${props.span ?? 12}`}>
      <div class="dk-face">
        <header class="dk-panel-head">
          <h2>{props.title}</h2>
          <Show when={props.meta}><span class="dk-meta">{props.meta}</span></Show>
          <Show when={props.table}>
            <button type="button" class="dk-pill dk-pill--small" aria-pressed={asTable()} onClick={() => setAsTable(!asTable())}>
              {asTable() ? "chart" : "table"}
            </button>
          </Show>
        </header>
        <Show when={asTable() && props.table} fallback={props.children}>{props.table!()}</Show>
      </div>
    </section>
  );
}

/** One row of presets (the filter row's date range). */
export function Presets<T extends string | number>(props: { label: string; options: readonly { value: T; label: string }[]; value: T; onPick: (v: T) => void }) {
  return (
    <div class="dk-presets" role="radiogroup" aria-label={props.label}>
      <span class="dk-presets-label">{props.label}</span>
      <For each={props.options}>
        {(o) => (
          <button type="button" role="radio" class="dk-pill" aria-checked={props.value === o.value} onClick={() => props.onPick(o.value)}>{o.label}</button>
        )}
      </For>
    </div>
  );
}

// ---------- figures ----------

/** A metric tile: label, value, change against the period before, a sparkline. */
export function StatTile(props: { label: string; value: string; sub?: string; delta?: Delta; upIsGood?: boolean; trend?: readonly number[]; span?: Span }) {
  const tone = () => {
    const d = props.delta;
    if (!d || d.dir === "flat") return "flat";
    return (d.dir === "up") === (props.upIsGood ?? true) ? "good" : "bad";
  };
  return (
    <section class={`dk-panel dk-tile dk-span-${props.span ?? 2}`}>
      <div class="dk-face">
        <h2 class="dk-tile-label">{props.label}</h2>
        <p class="dk-tile-value">{props.value}</p>
        <div class="dk-tile-foot">
          <Show when={props.delta}>
            {(d) => (
              <span class={`dk-delta dk-delta--${tone()}`}>
                <i aria-hidden="true">{d().dir === "up" ? "▲" : d().dir === "down" ? "▼" : "■"}</i>
                {d().text}<span class="sr-only"> {tone() === "good" ? "better" : tone() === "bad" ? "worse" : "unchanged"}</span>
              </span>
            )}
          </Show>
          <Show when={props.sub}><span class="dk-tile-sub">{props.sub}</span></Show>
        </div>
        <Show when={props.trend && props.trend.length > 1}><Sparkline values={props.trend!} label={`${props.label} trend`} /></Show>
      </div>
    </section>
  );
}

/** A 2px trend line in the quiet hue, its last point in the accent. */
export function Sparkline(props: { values: readonly number[]; label: string; width?: number; height?: number }) {
  const w = () => props.width ?? 120, h = () => props.height ?? 28;
  const pts = createMemo(() => {
    const v = props.values, m = Math.max(1, ...v);
    const sx = scale(0, Math.max(1, v.length - 1), 3, w() - 5), sy = scale(0, m, h() - 4, 4);
    return v.map((y, i) => [sx(i), sy(y)] as const);
  });
  const last = () => pts()[pts().length - 1];
  return (
    <svg class="dk-spark" viewBox={`0 0 ${w()} ${h()}`} width={w()} height={h()} role="img" aria-label={props.label}>
      <path d={linePath(pts())} fill="none" stroke={QUIET} stroke-width="2" stroke-linejoin="round" stroke-linecap="round" />
      <Show when={last()}>{(p) => <circle cx={p()[0]} cy={p()[1]} r="3" fill={ACCENT} stroke={WELL} stroke-width="2" />}</Show>
    </svg>
  );
}

// ---------- legend ----------

export interface Key { label: string; color: string; mark?: "rect" | "line" }

export function Legend(props: { items: readonly Key[] }) {
  return (
    <ul class="dk-legend">
      <For each={props.items}>
        {(k) => <li><i class={`dk-key dk-key--${k.mark ?? "rect"}`} style={{ background: k.color }} aria-hidden="true" />{k.label}</li>}
      </For>
    </ul>
  );
}

// ---------- time chart ----------

const M = { top: 10, right: 10, bottom: 22, left: 48 };

/** Width of an element, followed as it resizes (a fixed guess on the server). */
function useWidth(fallback: number): [() => number, (el: HTMLElement) => void] {
  const [w, setW] = createSignal(fallback);
  const bind = (el: HTMLElement) => {
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver((es) => { const cw = es[0]?.contentRect.width; if (cw) setW(Math.round(cw)); });
    ro.observe(el);
    onCleanup(() => ro.disconnect());
  };
  return [w, bind];
}

/** Days across, one axis up. `line` for one to five series, `area` for one
 *  (a 10% wash under its line), `stack` for parts of a daily whole (columns,
 *  2px gaps, the rounded end on top). Hover or arrow keys move a crosshair
 *  that snaps to a day; the tooltip lists every series there. */
export function TimeChart(props: {
  days: readonly Day[]; series: readonly Series[]; kind: "line" | "area" | "stack";
  format: (n: number) => string; base?: 10 | 1024; height?: number; label: string;
}) {
  const [width, bind] = useWidth(640);
  const [at, setAt] = createSignal<number | null>(null);
  const h = () => props.height ?? 180;
  const stacked = () => props.kind === "stack";
  const single = () => props.series.length === 1;
  const color = (i: number) => (single() && !stacked() ? ACCENT : seriesColor(i));
  const ticks = createMemo(() => niceTicks(peak(props.series, stacked()), 4, props.base ?? 10));
  const top = () => ticks()[ticks().length - 1]!;
  const n = () => props.days.length;
  const plotW = () => Math.max(40, width() - M.left - M.right);
  const slot = () => plotW() / Math.max(1, n());
  // Columns sit in the middle of their day's slot; line points span the plot.
  const x0 = () => M.left + (stacked() ? slot() / 2 : 0);
  const x1 = () => M.left + plotW() - (stacked() ? slot() / 2 : 0);
  const xAt = (i: number) => scale(0, Math.max(1, n() - 1), x0(), x1())(i);
  const y = (v: number) => scale(0, top(), M.top + h(), M.top)(v);
  const colW = () => Math.max(2, Math.min(24, slot() - 2));
  const stacks = createMemo(() => stack(props.series));
  const labelEvery = () => Math.max(1, Math.ceil(n() / Math.max(2, Math.floor(plotW() / 70))));

  function move(e: PointerEvent) {
    const r = (e.currentTarget as SVGElement).getBoundingClientRect();
    setAt(nearestIndex(e.clientX - r.left, x0(), x1(), n()));
  }
  function key(e: KeyboardEvent) {
    const i = at() ?? n() - 1;
    if (e.key === "ArrowLeft") { setAt(Math.max(0, i - 1)); e.preventDefault(); }
    else if (e.key === "ArrowRight") { setAt(Math.min(n() - 1, i + 1)); e.preventDefault(); }
    else if (e.key === "Home") { setAt(0); e.preventDefault(); }
    else if (e.key === "End") { setAt(n() - 1); e.preventDefault(); }
  }
  const tipLeft = () => { const i = at(); return i === null ? 0 : xAt(i); };

  return (
    <div class="dk-chart">
      <Show when={props.series.length > 1}>
        <Legend items={props.series.map((s, i) => ({ label: s.label, color: color(i), mark: stacked() ? "rect" : "line" }))} />
      </Show>
      <div class="dk-well" ref={bind}>
        <svg
          width={width()} height={h() + M.top + M.bottom} role="img" tabindex="0"
          aria-label={`${props.label}: ${n()} days. Arrow keys step through the days; the table view has every value.`}
          onPointerMove={move} onPointerLeave={() => setAt(null)} onKeyDown={key} onBlur={() => setAt(null)}
        >
          <Index each={ticks()}>
            {(t) => (
              <g>
                <line x1={M.left} x2={M.left + plotW()} y1={y(t())} y2={y(t())} stroke={t() === 0 ? BASELINE : GRID} stroke-width="1" shape-rendering="crispEdges" />
                <text class="dk-tick" x={M.left - 6} y={y(t()) + 4} text-anchor="end">{props.format(t())}</text>
              </g>
            )}
          </Index>
          <Index each={props.days}>
            {(d, i) => (
              <Show when={i % labelEvery() === (n() - 1) % labelEvery()}>
                <text class="dk-tick" x={xAt(i)} y={M.top + h() + 16} text-anchor="middle">{shortDay(d())}</text>
              </Show>
            )}
          </Index>
          <Show when={stacked()}>
            <Index each={stacks()}>
              {(s, si) => (
                <Index each={s().spans}>
                  {(span, i) => {
                    // The top segment of a column carries the rounded end; the rest are square,
                    // each 2px short of the one under it (the surface gap).
                    const isTop = () => stacks().slice(si + 1).every((o) => o.spans[i]![1] - o.spans[i]![0] <= 0);
                    const gap = () => (span()[0] > 0 ? 2 : 0);
                    const yTop = () => y(span()[1]);
                    const hh = () => Math.max(0, y(span()[0]) - yTop() - gap());
                    const x = () => xAt(i) - colW() / 2;
                    return (
                      <Show when={span()[1] > span()[0] && hh() > 0}>
                        <Show when={isTop()} fallback={<rect x={x()} y={yTop()} width={colW()} height={hh()} fill={color(si)} />}>
                          <path d={columnPath(x(), yTop(), colW(), hh(), 4)} fill={color(si)} />
                        </Show>
                      </Show>
                    );
                  }}
                </Index>
              )}
            </Index>
          </Show>
          <Show when={!stacked()}>
            <Index each={props.series}>
              {(s, si) => {
                const pts = () => s().values.map((v, i) => [xAt(i), y(v)] as const);
                return (
                  <g>
                    <Show when={props.kind === "area" && single()}><path d={areaPath(pts(), y(0))} fill={color(si)} fill-opacity="0.1" /></Show>
                    <path d={linePath(pts())} fill="none" stroke={color(si)} stroke-width="2" stroke-linejoin="round" stroke-linecap="round" />
                    <Show when={pts().length}><circle cx={pts()[pts().length - 1]![0]} cy={pts()[pts().length - 1]![1]} r="4" fill={color(si)} stroke={WELL} stroke-width="2" /></Show>
                  </g>
                );
              }}
            </Index>
          </Show>
          <Show when={at() !== null}>
            <line class="dk-crosshair" x1={xAt(at()!)} x2={xAt(at()!)} y1={M.top} y2={M.top + h()} />
            <Show when={!stacked()}>
              <Index each={props.series}>
                {(s, si) => <circle cx={xAt(at()!)} cy={y(s().values[at()!] ?? 0)} r="4" fill={color(si)} stroke={WELL} stroke-width="2" />}
              </Index>
            </Show>
          </Show>
        </svg>
        <Show when={at() !== null}>
          <div class="dk-tip" role="status" style={{ left: `${tipLeft()}px`, transform: `translateX(${tipLeft() > width() / 2 ? "calc(-100% - 10px)" : "10px"})` }}>
            <b class="dk-tip-day">{shortDay(props.days[at()!]!)}</b>
            <For each={props.series}>
              {(s, si) => (
                <span class="dk-tip-row"><i class="dk-key dk-key--line" style={{ background: color(si()) }} /><b>{props.format(s.values[at()!] ?? 0)}</b><span>{s.label}</span></span>
              )}
            </For>
            <Show when={stacked() && props.series.length > 1}>
              <span class="dk-tip-row dk-tip-total"><i class="dk-key" /><b>{props.format(props.series.reduce((a, s) => a + (s.values[at()!] ?? 0), 0))}</b><span>total</span></span>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}

/** A time chart's table twin: a row a day, newest first, a column a series. */
export function SeriesTable(props: { days: readonly Day[]; series: readonly Series[]; format: (n: number) => string; label: string }) {
  type R = { day: Day; i: number };
  const rows = () => props.days.map((day, i) => ({ day, i })).reverse();
  return (
    <DenseTable<R>
      label={props.label} rows={rows()} rowKey={(r) => r.day} initialSort={{ col: "day", dir: "desc" }}
      cols={[
        { key: "day", label: "day", value: (r) => r.day, render: (r) => shortDay(r.day) },
        ...props.series.map((s) => ({ key: s.key, label: s.label, num: true, value: (r: R) => s.values[r.i] ?? 0, render: (r: R) => props.format(s.values[r.i] ?? 0) })),
        ...(props.series.length > 1
          ? [{ key: "_total", label: "total", num: true, value: (r: R) => props.series.reduce((a, s) => a + (s.values[r.i] ?? 0), 0), render: (r: R) => props.format(props.series.reduce((a, s) => a + (s.values[r.i] ?? 0), 0)) }]
          : []),
      ]}
    />
  );
}

// ---------- bars ----------

export interface RankRow { key: string; label: string; value: number; other?: boolean; href?: string }

/** Ranked horizontal bars, one hue, largest first, the value at each tip;
 *  an "other" row in the quiet hue. Bars share one baseline and one scale. */
export function RankBars(props: { rows: readonly RankRow[]; format: (n: number) => string; label: string }) {
  const max = () => Math.max(1, ...props.rows.map((r) => r.value));
  return (
    <ol class="dk-rank" aria-label={props.label}>
      <For each={props.rows}>
        {(r) => (
          <li title={`${r.label}: ${props.format(r.value)}`}>
            <span class="dk-rank-name">{r.href ? <A href={r.href}>{r.label}</A> : r.label}</span>
            <span class="dk-rank-track">
              <i class="dk-rank-bar" style={{ width: `${(r.value / max()) * 100}%`, background: r.other ? QUIET : ACCENT }} />
            </span>
            <b class="dk-rank-value">{props.format(r.value)}</b>
          </li>
        )}
      </For>
    </ol>
  );
}

/** One whole split into parts, as a thin bar with no text (the text beside it
 *  carries the figures). A part at zero draws nothing; 2px gaps between parts. */
export function SegBar(props: { parts: readonly { label: string; value: number }[]; format: (n: number) => string; label: string; colors?: readonly string[] }) {
  const sh = () => shares(props.parts.map((p) => p.value));
  const name = () => `${props.label}: ${props.parts.map((p, i) => `${p.label} ${props.format(p.value)} (${Math.round(sh()[i]! * 100)}%)`).join(", ")}`;
  return (
    <span class="dk-seg" role="img" aria-label={name()} title={name()}>
      <Index each={props.parts}>
        {(p, i) => <Show when={p().value > 0}><i style={{ "flex-grow": sh()[i], background: props.colors?.[i] ?? seriesColor(i) }} /></Show>}
      </Index>
    </span>
  );
}

// ---------- table ----------

export interface Col<T> {
  key: string;
  label: string;
  num?: boolean;
  value: (r: T) => number | string;
  render?: (r: T) => JSX.Element;
  /** An inline bar under the number, scaled to the column's largest. */
  bar?: boolean;
  class?: string;
}

/** A dense, sortable table: tabular figures, numbers right-aligned, a
 *  header click (or Enter on it) sorts, and again reverses. */
export function DenseTable<T>(props: { rows: readonly T[]; cols: readonly Col<T>[]; rowKey: (r: T) => string; label: string; initialSort?: { col: string; dir: SortDir } }) {
  const [sort, setSort] = createSignal(props.initialSort ?? { col: props.cols[0]!.key, dir: "asc" as SortDir });
  const col = () => props.cols.find((c) => c.key === sort().col) ?? props.cols[0]!;
  const rows = createMemo(() => sortRows(props.rows, col().value, sort().dir));
  const maxOf = createMemo(() => Object.fromEntries(props.cols.filter((c) => c.bar).map((c) => [c.key, Math.max(1, ...props.rows.map((r) => Number(c.value(r)) || 0))])));
  const pick = (c: Col<T>) => setSort((s) => (s.col === c.key ? { col: c.key, dir: s.dir === "asc" ? "desc" : "asc" } : { col: c.key, dir: c.num ? "desc" : "asc" }));
  return (
    <div class="dk-table-wrap">
      <table class="dk-table">
        <caption class="sr-only">{props.label}</caption>
        <thead>
          <tr>
            <For each={props.cols}>
              {(c) => (
                <th scope="col" classList={{ num: !!c.num, [c.class ?? ""]: !!c.class }} aria-sort={sort().col === c.key ? (sort().dir === "asc" ? "ascending" : "descending") : "none"}>
                  <button type="button" onClick={() => pick(c)}>
                    {c.label}<i aria-hidden="true">{sort().col === c.key ? (sort().dir === "asc" ? "▲" : "▼") : ""}</i>
                  </button>
                </th>
              )}
            </For>
          </tr>
        </thead>
        <tbody>
          <For each={rows()}>
            {(r) => (
              <tr data-key={props.rowKey(r)}>
                <For each={props.cols}>
                  {(c) => (
                    <td classList={{ num: !!c.num, [c.class ?? ""]: !!c.class }}>
                      {c.render ? c.render(r) : String(c.value(r))}
                      <Show when={c.bar}><i class="dk-cellbar" style={{ width: `${((Number(c.value(r)) || 0) / maxOf()[c.key]!) * 100}%` }} aria-hidden="true" /></Show>
                    </td>
                  )}
                </For>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}
