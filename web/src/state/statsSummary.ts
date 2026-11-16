// The server's stats summary for the stats home, asked again every
// half-minute while the tab shows and whenever the window or server
// changes. A server without the endpoint (404) is `unsupported`; the page
// still shows what the live state knows.

import { createEffect, createSignal, onCleanup, untrack, type Accessor } from "solid-js";
import { ApiError } from "../api/http";
import { parseSummary, type StatsSummary, type StatsWindow } from "../lib/statsHome";
import { live } from "./live";

export const SUMMARY_REFRESH_MS = 30_000;

export type SummaryStatus = "loading" | "ok" | "unsupported" | "error";

/** `window` null asks nothing (another hook already has that window). */
export function useStatsSummary(window: Accessor<StatsWindow | null>, everyMs = SUMMARY_REFRESH_MS):
  { summary: Accessor<StatsSummary | null>; status: Accessor<SummaryStatus> } {
  const [summary, setSummary] = createSignal<StatsSummary | null>(null);
  const [status, setStatus] = createSignal<SummaryStatus>("loading");
  createEffect(() => {
    const w = window();
    if (!w) return;
    live.server().api; // a different server: start over
    const ac = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    setStatus(untrack(summary)?.window === w ? "ok" : "loading");
    const run = async () => {
      clearTimeout(timer);
      if (ac.signal.aborted) return;
      if (!document.hidden) {
        try {
          setSummary(parseSummary(await live.api().statsSummary(w, ac.signal)));
          setStatus("ok");
        } catch (e) {
          if (ac.signal.aborted) return;
          if (e instanceof ApiError && e.status === 404) { setSummary(null); setStatus("unsupported"); return; }
          setStatus("error");
        }
      }
      if (!ac.signal.aborted) timer = setTimeout(run, everyMs);
    };
    const seen = () => { if (!document.hidden) void run(); };
    document.addEventListener("visibilitychange", seen);
    void run();
    onCleanup(() => { ac.abort(); clearTimeout(timer); document.removeEventListener("visibilitychange", seen); });
  });
  return { summary, status };
}
