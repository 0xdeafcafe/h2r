import { beforeAll, describe, expect, test } from "bun:test";
import { plugin } from "bun";
import type { OverlayProps } from "../src/viewer/Overlay.tsx";
import type { ViewPanel } from "../src/viewer/ViewMenu";
import { loadProgress, overlayState } from "../src/viewer/overlayState";

describe("the viewer overlay state", () => {
  const pending = { sceneReady: true, glb: null };
  const geometry = { sceneReady: true, glb: "/art/arena.glb", manifestReady: true };

  test("loading and reveal follow the map through its first drawn frame", () => {
    expect(overlayState({ sceneReady: false })).toBe("boot");
    expect(overlayState(pending)).toBe("loading");
    expect(overlayState({ ...geometry, phase: "fetching" })).toBe("loading");
    expect(overlayState({ ...geometry, phase: "decoding" })).toBe("decoding");
    expect(overlayState({ ...geometry, phase: "ok" })).toBe("decoding");
    expect(overlayState({ ...geometry, phase: "ok", reveal: "started" })).toBe("reveal");
    expect(overlayState({ ...geometry, phase: "ok", reveal: "done" })).toBe("ready");
  });

  test("missing geometry, a failed fetch, and WebGL failure have different states", () => {
    expect(overlayState({ ...pending, manifestReady: true })).toBe("none");
    expect(overlayState({ ...pending, waitExpired: true })).toBe("none");
    expect(overlayState({ ...geometry, phase: "failed" })).toBe("error");
    expect(overlayState({ sceneReady: false, unavailable: true })).toBe("unsupported");
  });
});

describe("geometry load progress", () => {
  test("the first byte lights a segment, full length lights 22, decoding 23, and reveal 24", () => {
    const lit = [0, 1, 87, 50, 100].reduce<number[]>((out, loaded) => {
      out.push(loadProgress("loading", loaded, 100, out.at(-1)).lit);
      return out;
    }, []);
    expect(lit).toEqual([1, 1, 19, 19, 22]);
    expect(loadProgress("loading", 100, 100).percent).toBe(100);
    expect(loadProgress("decoding").lit).toBe(23);
    expect(loadProgress("reveal").lit).toBe(24);
  });

  test("a missing length chases and prints the bytes received", () => {
    expect(loadProgress("boot").chasing).toBe(true);
    expect(loadProgress("loading", 2_400_000)).toMatchObject({ chasing: true, percent: null, size: "2.4 MB" });
    expect(loadProgress("loading", 8200, 0).size).toBe("8.2 KB");
    expect(loadProgress("loading", 820).size).toBe("820 B");
    expect(loadProgress("loading").size).toBeNull();
  });
});

let render: (props: Omit<OverlayProps, "title"> & { title?: string }) => string;
let renderMenu: (panels: readonly ViewPanel[]) => string;

beforeAll(async () => {
  const babel = await import("@babel/core");
  const solid = (await import("babel-preset-solid")).default;
  plugin({
    name: "solid-ssr-overlay",
    setup(build) {
      build.onLoad({ filter: /\/src\/viewer\/(?:Overlay|ViewMenu)\.tsx$/ }, async ({ path }) => {
        const src = await Bun.file(path).text();
        const out = await babel.transformAsync(src, {
          filename: path, babelrc: false, configFile: false,
          parserOpts: { plugins: ["jsx", "typescript"] },
          presets: [[solid, { generate: "ssr", hydratable: false }]],
        });
        return { contents: out!.code!, loader: "ts" };
      });
    },
  });
  const { renderToString } = await import("solid-js/web");
  const { createComponent } = await import("solid-js");
  const { Overlay } = await import("../src/viewer/Overlay.tsx");
  const { ViewMenu } = await import("../src/viewer/ViewMenu");
  render = (props) => renderToString(() => createComponent(Overlay, { title: "Slayer on Orchard", ...props }));
  const noop = () => {};
  renderMenu = (panels) => renderToString(() => createComponent(ViewMenu, {
    panels, open: noop, back: noop, close: noop, trigger: () => undefined,
  }));
});

describe("the rendered viewer overlay", () => {
  test("boot shows the grid and an indeterminate bar, with no controls", () => {
    const html = render({ state: "boot", children: "CAMERA CONTROLS" });
    expect(html).toContain('data-state="boot"');
    expect(html).toContain("vo-grid");
    expect(html).toContain("Map uplink");
    expect(html).toContain("Slayer on Orchard");
    expect(html).toContain('role="progressbar"');
    expect(html).toContain('aria-valuemin="0"');
    expect(html).toContain('aria-valuemax="100"');
    expect(html).not.toContain("aria-valuenow");
    expect(html).toContain('aria-live="polite">Loading viewer');
    expect(html).not.toContain("CAMERA CONTROLS");
  });

  test("loading gives the byte progress an accessible value and decoding pulses segment 23", () => {
    const html = render({
      state: "loading",
      backdrop: "/orchard.png",
      progress: loadProgress("loading", 63, 100),
    });
    expect(html).toContain('src="/orchard.png"');
    expect(html).toContain("Loading map geometry");
    expect(html).toContain('aria-valuenow="63"');
    expect(html).toContain("63%");
    const decoding = render({ state: "decoding" });
    expect(decoding.match(/vo-segment--lit/g)).toHaveLength(23);
    expect(decoding.match(/vo-segment--pulse/g)).toHaveLength(1);
    expect(decoding).toContain("Decoding geometry");
  });

  test("the controls show over the veil during reveal and without it when ready", () => {
    const children = "CAMERA CONTROLS";
    const reveal = render({ state: "reveal", children });
    expect(reveal).toContain("vo-veil--reveal");
    expect(reveal).toContain(children);
    const ready = render({ state: "ready", children });
    expect(ready).not.toContain("vo-veil");
    expect(ready).toContain(children);
    expect(render({ state: "ready", children, bare: true })).not.toContain(children);
  });

  test("a missing map explains the grid and offers the 2D map", () => {
    const html = render({ state: "none", on2D: () => {}, children: "CAMERA CONTROLS" });
    expect(html).toContain('role="status"');
    expect(html).toContain("No 3D geometry for this map");
    expect(html).toContain("The floor grid is fitted to where players have been.");
    expect(html).toContain(">2D map</button>");
    expect(html).toContain("CAMERA CONTROLS");
    expect(html).not.toContain(">Retry</button>");
  });

  test("a failed map load offers retry and 2D, while WebGL failure keeps the veil", () => {
    const error = render({ state: "error", onRetry: () => {}, on2D: () => {}, children: "CAMERA CONTROLS" });
    expect(error).toContain('role="alert"');
    expect(error).toContain("Uplink lost");
    expect(error).toContain(">Retry</button>");
    expect(error).toContain(">2D map</button>");
    expect(error).toContain("CAMERA CONTROLS");
    expect(error).not.toContain("vo-veil");
    const unsupported = render({ state: "unsupported", error: "WebGL is disabled.", on2D: () => {}, children: "CAMERA CONTROLS" });
    expect(unsupported).toContain('role="alert"');
    expect(unsupported).toContain("3D view unavailable");
    expect(unsupported).toContain("WebGL is disabled.");
    expect(unsupported).toContain("vo-veil");
    expect(unsupported).toContain(">2D map</button>");
    expect(unsupported).not.toContain("CAMERA CONTROLS");
    expect(unsupported).not.toContain(">Retry</button>");
  });
});


test("the View menu shows only applicable rows and opens a dialog for a picker", () => {
  const pick = () => {};
  const see: ViewPanel = {
    title: "See-through",
    rows: [{ label: "In focus", selected: true, pick }, { label: "Off", selected: false, pick }],
  };
  const view: ViewPanel = {
    title: "View",
    rows: [
      { label: "Ambient occlusion", checked: true, pick },
      { label: "Lifts & teleporters", show: false, checked: false, pick },
      { label: "See-through", panel: see, value: "In focus" },
    ],
  };
  const menu = renderMenu([view]);
  expect(menu).toContain('role="menu"');
  expect(menu).toContain('role="menuitemcheckbox"');
  expect(menu).toContain("Ambient occlusion");
  expect(menu).not.toContain("teleporters");
  expect(menu).toContain('aria-haspopup="dialog"');
  const picker = renderMenu([view, see]);
  expect(picker).toContain('role="dialog"');
  expect(picker).toContain('aria-label="See-through"');
  expect(picker).toContain('role="menuitemradio"');
  expect(picker).toContain('aria-checked="true"');
});
