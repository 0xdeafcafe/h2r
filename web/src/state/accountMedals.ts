// An account's medals over every game it played (`GET /v1/accounts/{who}/medals`,
// docs/live/live-api.md "Medals"), read again when the account or the server
// changes. Null until read, and for an account the server doesn't know.

import { createEffect, createSignal, on, type Accessor } from "solid-js";
import { parseAccountMedals } from "./derive";
import { live } from "./live";

export function useAccountMedals(who: Accessor<string | null | undefined>): Accessor<Map<string, number> | null> {
  const [medals, setMedals] = createSignal<Map<string, number> | null>(null);
  createEffect(on(() => [who(), live.server().api] as const, ([x]) => {
    setMedals(null);
    if (!x) return;
    live.api().accountMedals(x).then((b) => { if (who() === x) setMedals(parseAccountMedals(b)); }, () => {});
  }));
  return medals;
}
