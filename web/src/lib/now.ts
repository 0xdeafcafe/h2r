import { createRoot, createSignal } from "solid-js";

/** Wall clock that ticks once a second, for elapsed times and "ago"s. */
export const now = createRoot(() => {
  const [t, set] = createSignal(Date.now());
  setInterval(() => set(Date.now()), 1000);
  return t;
});
