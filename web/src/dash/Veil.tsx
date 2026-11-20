// The app's root, and the veil between its two sides. Leaving the
// dashboard for the stats site, the green swells up through the screen and
// thins out into the stats page's navy; coming back, the page sinks into
// the dark and a green flare closes in on the orb. It covers the old side
// (200 ms), the route changes underneath, then it clears (320 ms). Any key
// or press skips it. With prefers-reduced-motion it doesn't play at all.
//
// On the stats side it also leaves a hairline of dashboard green across
// the very top of the page.

import { Show, createEffect, createSignal, on, onCleanup, onMount, type JSX } from "solid-js";
import { useLocation, useNavigate } from "@solidjs/router";
import { CROSS_IN_MS, CROSS_OUT_MS, crosses, sideOf, type Side } from "./cross";
import "../styles/dash-veil.css";
import { NotificationToast } from "./Notifications";

interface VeilState { to: Side; phase: "in" | "out" }

const [veil, setVeil] = createSignal<VeilState | null>(null);
let navigate: ((href: string, opts?: { replace?: boolean }) => void) | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let pending: (() => void) | null = null;
/** Set when a crossing was skipped: the change of side it causes shouldn't play the veil after all. */
let quiet = false;

const reducedMotion = () => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

/** The veil, if one is playing. */
export const crossing = veil;

function reveal(to: Side) {
  clearTimeout(timer);
  setVeil({ to, phase: "out" });
  timer = setTimeout(() => setVeil(null), CROSS_OUT_MS);
}

/** Go to `href` on the other side, through the veil. */
export function crossTo(href: string, opts?: { replace?: boolean }) {
  if (!navigate) { location.assign(href); return; }
  const nav = navigate;
  if (reducedMotion()) { nav(href, opts); return; }
  if (pending) return;
  const to = sideOf(href);
  clearTimeout(timer);
  setVeil({ to, phase: "in" });
  pending = () => { pending = null; nav(href, opts); reveal(to); };
  timer = setTimeout(() => pending?.(), CROSS_IN_MS);
}

/** Finish at once: change the route if that is still to come, and drop the veil. */
function skip() {
  if (!veil()) return;
  if (pending) { quiet = true; pending(); }
  clearTimeout(timer);
  setVeil(null);
}

export default function Veil(props: { children?: JSX.Element }) {
  const location_ = useLocation();
  navigate = useNavigate();
  const side = () => sideOf(location_.pathname);

  // A change of side that didn't come through crossTo (the browser's back
  // button, a redirect): the route has already changed, so only clear off.
  createEffect(on(side, (to) => { const hush = quiet; quiet = false; if (!hush && !veil() && !reducedMotion()) reveal(to); }, { defer: true }));

  function onClick(e: MouseEvent) {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = (e.target as Element | null)?.closest?.("a[href]");
    if (!(a instanceof HTMLAnchorElement) || a.target || a.hasAttribute("download") || a.origin !== location.origin) return;
    if (!crosses(location.pathname, a.pathname)) return;
    e.preventDefault();
    crossTo(a.pathname + a.search + a.hash);
  }
  onMount(() => {
    document.addEventListener("click", onClick, true);
    window.addEventListener("keydown", skip, true);
    window.addEventListener("pointerdown", skip, true);
  });
  onCleanup(() => {
    document.removeEventListener("click", onClick, true);
    window.removeEventListener("keydown", skip, true);
    window.removeEventListener("pointerdown", skip, true);
    clearTimeout(timer);
    pending = null;
    navigate = null;
  });

  return (
    <>
      <Show when={side() === "stats"}><i class="xf-echo" aria-hidden="true" /></Show>
      {props.children}
      <NotificationToast />
      <Show when={veil()}>{(v) => <div class={`xf xf--to-${v().to} xf--${v().phase}`} aria-hidden="true"><i class="xf-n" /><i class="xf-g" /></div>}</Show>
    </>
  );
}
