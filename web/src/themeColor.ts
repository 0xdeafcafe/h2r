// Keeps <meta name="theme-color"> on the side the page is on: the server
// sends the first page's (page_meta.rs); moving between the dashboard and
// the stats site in the app changes it here.
import { createEffect } from "solid-js";
import { useLocation } from "@solidjs/router";
import { THEME } from "./brand";
import { sideOf } from "./dash/cross";

export function ThemeColor() {
  const location = useLocation();
  createEffect(() => {
    const colour = THEME[sideOf(location.pathname)];
    let meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
    if (!meta) {
      meta = document.createElement("meta");
      meta.name = "theme-color";
      document.head.append(meta);
    }
    if (meta.content !== colour) meta.content = colour;
  });
  return null;
}
