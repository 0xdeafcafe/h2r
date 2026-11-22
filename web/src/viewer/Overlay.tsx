import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, type JSX } from "solid-js";
import { LOAD_SEGMENTS, loadProgress, type LoadProgress, type OverlayState } from "./overlayState";
import "./overlay.css";

export interface OverlayProps {
  state: OverlayState;
  title: string;
  backdrop?: string | null;
  progress?: LoadProgress;
  error?: string | null;
  onRetry?: () => void;
  on2D?: () => void;
  bare?: boolean;
  status?: JSX.Element;
  children?: JSX.Element;
}

const segments = Array.from({ length: LOAD_SEGMENTS }, (_, segment) => segment);

function Notice(props: {
  class?: string;
  title: string;
  alert?: boolean;
  onRetry?: () => void;
  on2D?: () => void;
  children: JSX.Element;
}) {
  return (
    <div class={`vo-plate vo-notice ${props.class ?? ""}`} role={props.alert ? "alert" : "status"}>
      <div>
        <div class="vo-title">{props.title}</div>
        <p>{props.children}</p>
        <Show when={props.onRetry || props.on2D}>
          <div class="vo-actions">
            <Show when={props.onRetry}><button class="vo-button" type="button" onClick={props.onRetry}>Retry</button></Show>
            <Show when={props.on2D}><button class="vo-button" type="button" onClick={props.on2D}>2D map</button></Show>
          </div>
        </Show>
      </div>
    </div>
  );
}

export function Overlay(props: OverlayProps) {
  const loading = createMemo(() => props.state === "boot" || props.state === "loading" || props.state === "decoding");
  const grid = createMemo(() => props.state === "none");
  const reveal = () => props.state === "reveal";
  const unsupported = () => props.state === "unsupported";
  const progress = createMemo(() => props.progress ?? loadProgress(props.state));
  const [loadingShown, setLoadingShown] = createSignal(false);
  const [collapsed, setCollapsed] = createSignal(false);
  const [enteredViaReveal, setEnteredViaReveal] = createSignal(false);

  // Do not fade a loading plate in on the way out when it never appeared.
  createEffect(on(loading, (active) => {
    if (!active) return;
    setLoadingShown(false);
    const timer = setTimeout(() => setLoadingShown(true), 150);
    onCleanup(() => clearTimeout(timer));
  }));
  createEffect(on(grid, (active) => {
    setCollapsed(false);
    if (!active) return;
    const timer = setTimeout(() => setCollapsed(true), 6000);
    onCleanup(() => clearTimeout(timer));
  }));
  createEffect(() => {
    if (reveal()) setEnteredViaReveal(true);
    else if (props.state !== "ready") setEnteredViaReveal(false);
  });

  return (
    <div class="vo" classList={{ "vo--bare": props.bare }} data-state={props.state}>
      <Show when={loading() || reveal() || unsupported()}>
        <div class="vo-veil" classList={{ "vo-veil--reveal": reveal() }}>
          <Show when={props.backdrop} fallback={<div class="vo-grid" aria-hidden="true" />}>
            {(src) => <img class="vo-backdrop" alt="" src={src()} />}
          </Show>
          <Show when={loading() || (reveal() && loadingShown())}>
            <div class="vo-plate vo-loading">
              <div>
                <div class="vo-scan" aria-hidden="true" />
                <div class="vo-eyebrow">Map uplink</div>
                <div class="vo-title">{props.title}</div>
                <div
                  class="vo-progress"
                  classList={{ "vo-progress--chase": progress().chasing }}
                  role="progressbar"
                  aria-label={progress().text}
                  aria-valuemin="0"
                  aria-valuemax="100"
                  aria-valuenow={progress().percent ?? undefined}
                >
                  <For each={segments}>
                    {(segment) => (
                      <span classList={{
                        "vo-segment--lit": segment < progress().lit,
                        "vo-segment--pulse": props.state === "decoding" && segment === 22,
                      }} style={{ "--i": String(segment) }} />
                    )}
                  </For>
                </div>
                <div class="vo-load-status">
                  <span aria-live="polite">{progress().text}</span>
                  <span>{progress().percent === null ? progress().size : `${progress().percent}%`}</span>
                </div>
              </div>
            </div>
          </Show>
          <Show when={unsupported()}>
            <Notice title="3D view unavailable" alert on2D={props.on2D}>
              {props.error ?? "This browser could not start the 3D view."}
            </Notice>
          </Show>
        </div>
      </Show>

      <Show when={!props.bare && !loading() && !unsupported()}>
        <div class="vo-ui" classList={{ "vo-ui--reveal": enteredViaReveal() || reveal() }}>
          {props.children}
          <Show when={props.status || (grid() && collapsed())}>
            <div class="vo-status">
              {props.status}
              <Show when={grid() && collapsed()}>
                <div class="vo-note vo-note--warn">No 3D geometry for this map.</div>
              </Show>
            </div>
          </Show>
          <Show when={grid() && !collapsed()}>
            <Notice class="vo-notice--grid" title="No 3D geometry for this map" on2D={props.on2D}>
              The floor grid is fitted to where players have been.
            </Notice>
          </Show>
        </div>
      </Show>

      <Show when={props.state === "error"}>
        <Notice class="vo-notice--error" title="Uplink lost" alert onRetry={props.onRetry} on2D={props.on2D}>
          The map's geometry didn't load. The floor grid stands in for it.
        </Notice>
      </Show>
    </div>
  );
}
