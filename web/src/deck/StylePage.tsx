// /dash/admin/style: the dense dashboard's components on fixed data, laid
// out as the Live admin's usage screen is (docs/live/web-boundary.md §3).
// Nothing is fetched and nothing needs signing in.

import { createMemo, createSignal } from "solid-js";
import { DeckPage, Presets } from "./Deck";
import { topN } from "./chart";
import { mockOverview, usageMock } from "./mock";
import { usageView } from "./usageView";
import UsageBoard from "./UsageBoard";

const LAST = "2026-11-18";
const DATA = usageMock(LAST, 60);
export const RANGES = [
  { value: 7, label: "7 days" },
  { value: 14, label: "14 days" },
  { value: 30, label: "30 days" },
] as const;

export default function StylePage() {
  const [range, setRange] = createSignal<number>(30);
  const v = createMemo(() => usageView(mockOverview(DATA, range()), mockOverview(DATA, range() * 2), LAST, range()));
  const top = createMemo(() => topN(DATA.accounts, 12, (a) => a.uploaded));
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
      <UsageBoard view={v()} range={range()} accounts={top()} accountsNote="by data uploaded, last 30 days" sort="uploaded" />
    </DeckPage>
  );
}
