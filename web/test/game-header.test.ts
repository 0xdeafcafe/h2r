// The game page's pre-rendered header (src/stats/gameHeader.ts): the model
// derived from a game, the markup both the servers and the client write
// from it (the fixtures under fixtures/ are the contract — the Rust side,
// crates/live/live-server/src/page_header.rs, renders the same model into the
// same bytes), and the emblem's fixed size.

import { describe, expect, test } from "bun:test";
import {
  adoptGameHeader, escapeHtml, gameHeaderHtml, gameHeaderJson, gameHeaderModel, showGameHeader, teamName,
} from "../src/stats/gameHeader";
import { packEmblem } from "../src/lib/art";
import { emblemPath } from "../src/lib/emblem";
import { fixtureGame } from "./fixtures/game-header-game";
import type { GamePlayerState } from "../src/state/reducer";

const MODEL_JSON = await Bun.file(new URL("fixtures/game-header.json", import.meta.url)).text();
const GOLDEN_HTML = await Bun.file(new URL("fixtures/game-header.html", import.meta.url)).text();
const model = JSON.parse(MODEL_JSON);

describe("the game header's model", () => {
  test("derived from the fixture game, it is the fixture model", () => {
    expect(gameHeaderJson(gameHeaderModel(fixtureGame()))).toBe(MODEL_JSON.trim());
  });

  test("variant titles preserve spaced names and title-case bare keys", () => {
    const game = fixtureGame();
    expect(gameHeaderModel({ ...game, variant: "team slayer" }).title).toBe("team slayer");
    expect(gameHeaderModel({ ...game, variant: "team_slayer" }).title).toBe("Team Slayer");
    expect(gameHeaderModel({ ...game, variant: "Team Slayer" }).title).toBe("Team Slayer");
  });

  test("players stand by score then index, teams by score then team, each with its emblem", () => {
    // Blue 20 ahead of Red 18; within Blue, Sneaky Pete's 12 over the
    // Clanker's 8; a player without an account is picked from their name.
    expect(model.teams.map((t: { name: string; score: number }) => [t.name, t.score])).toEqual([["Blue", 20], ["Red", 18]]);
    expect(model.teams[0].players.map((p: { name: string; seed: string }) => [p.name, p.seed]))
      .toEqual([["Sneaky Pete", "0009000000000043"], ["Clanker-7", "Clanker-7"]]);
    expect(model.teams[0].players[1].bot).toBe(true);
    expect(model.teams[0].players[1].emblem).toBe(packEmblem("Clanker-7"));
  });

  test("a free-for-all game lists players with their own colours and no teams", () => {
    const g = fixtureGame();
    g.team_play = false;
    g.teams = {};
    g.winner = { player: 1 };
    const m = gameHeaderModel(g);
    expect(m.teams).toBeUndefined();
    expect(m.players?.length).toBe(4);
    expect(m.result).toBe("Sneaky Pete placed 1st");
    expect(new Set(m.players?.map((p) => p.colour)).size).toBe(4);
  });

  test("a campaign game has the mission for its title, its players without scores", () => {
    const g = fixtureGame();
    g.mode = "campaign";
    g.kind = "campaign";
    g.variant = null;
    g.difficulty = "heroic";
    g.online = false;
    g.team_play = false;
    g.teams = {};
    const m = gameHeaderModel(g);
    expect(m.campaign).toBe(true);
    expect(m.title).toBe("Orchard");
    expect(m.type).toBe("Campaign Co-op · Heroic");
    expect(m.players?.every((p) => p.score === undefined)).toBe(true);
  });

  test("a live game says so, with no result yet", () => {
    const g = fixtureGame();
    g.status = "live";
    g.ended_at = null;
    g.winner = null;
    const m = gameHeaderModel(g);
    expect([m.live, m.status, m.result]).toEqual([true, "LIVE", null]);
  });

  test("the emblem pick is the one the site's Emblem component makes", () => {
    // components/Emblem.tsx picks from the seed with the same FNV-1a hash;
    // pinned here so the header and the tables agree on a player's badge.
    expect(packEmblem("0009000000000042")).toBe(16);
    expect(packEmblem("guest")).toBe(packEmblem("guest"));
    for (let i = 0; i < 40; i++) {
      const pick = packEmblem(`seed-${i}`);
      expect(pick).toBeGreaterThanOrEqual(0);
      expect(pick).toBeLessThan(24);
    }
  });

  test("the header uses the game's appearance and follows a remote server selection", () => {
    const game = fixtureGame();
    const host = game.players[0]!;
    const player = gameHeaderModel(game, "https://another.example").teams?.[1]?.players[0];
    expect(player?.emblemSrc).toBe("https://another.example" + emblemPath(host.xuid!, host.appearance));
    expect(player?.emblemSrc).toBe("https://another.example/art/emblem/30-7-3-6-8-2.png");
  });

  test("team names are the eight the site uses", () => {
    expect(teamName(0)).toBe("Red");
    expect(teamName(7)).toBe("Brown");
    expect(teamName(8)).toBe("Team 9");
    expect(teamName(-1)).toBe("Team 0");
  });
});

describe("the game header's markup", () => {
  test("the fixture model renders the golden markup, byte for byte", () => {
    expect(gameHeaderHtml(model)).toBe(GOLDEN_HTML.trim());
  });

  test("the pre-rendered header holds the game, its standings and its emblem", () => {
    const html = gameHeaderHtml(model);
    expect(html).toContain('<div class="s-gamehead" data-game="0123abcd">');
    expect(html).toContain("<b>Team Slayer</b> on Orchard");
    expect(html).toContain("<i class=\"s-gamehead-flag\">FINAL</i> <span>Blue Team won</span>");
    expect(html).toContain("<header><span>Blue</span> <b>20</b></header>");
    // A name that needs escaping, escaped.
    expect(html).toContain("<span>Rock &amp; Roll</span>");
    expect(html).toContain('<img class="emblem emblem--game" src="/art/emblem/30-7-3-6-8-2.png"');
    expect(html).toContain('data-emblem-fallback="/art/emblems/17-half-disc-stack.webp"');
  });

  test("scores from the wire are rendered as text", () => {
    const wire = JSON.parse(JSON.stringify(model));
    wire.teams[0].score = "<team>";
    wire.teams[0].players[0].score = "<player>";
    const html = gameHeaderHtml(wire);
    expect(html).toContain("&lt;team&gt;");
    expect(html).toContain("&lt;player&gt;");
  });

  test("every emblem element has fixed dimensions, so nothing shifts as it loads", () => {
    const imgs = gameHeaderHtml(model).match(/<img [^>]*>/g) ?? [];
    expect(imgs.length).toBeGreaterThan(0);
    for (const img of imgs) {
      expect(img).toContain('width="16"');
      expect(img).toContain('height="16"');
    }
  });

  test("text is escaped as the servers escape it", () => {
    expect(escapeHtml(`<b>&"'</b>`)).toBe("&lt;b&gt;&amp;&quot;&#39;&lt;/b&gt;");
    expect(escapeHtml("a\u0001b")).toBe("a b");
  });

  test("the model as the page carries it cannot close its script tag", () => {
    const g = fixtureGame();
    (g.players[4] as GamePlayerState) = {
      index: 4, name: "</script><script>alert(1)</script>", xuid: null, team: 0, local: false,
      game_name: null, left: false, appearance: null,
    };
    const json = gameHeaderJson(gameHeaderModel(g));
    expect(json).not.toContain("</script>");
    expect(() => JSON.parse(json)).not.toThrow();
  });

  test("a live game's header carries the live flag, not the final one", () => {
    const g = fixtureGame();
    g.status = "live";
    g.winner = null;
    const html = gameHeaderHtml(gameHeaderModel(g));
    expect(html).toContain('s-gamehead-flag--live">LIVE<');
    expect(html).not.toContain("FINAL");
  });
});

describe("the Game Details hero", () => {
  function heroSlot() {
    const model = { textContent: MODEL_JSON.trim() };
    const sourceHeader = { dataset: { game: "0123abcd" }, querySelector: () => model };
    const source = { querySelector: () => sourceHeader };
    const slot = {
      dataset: {} as Record<string, string>,
      firstElementChild: null as typeof sourceHeader | null,
      innerHTML: "",
      appendChild(header: typeof sourceHeader) { this.firstElementChild = header; },
      replaceChildren() { this.firstElementChild = null; this.innerHTML = ""; },
    };
    return { slot, element: slot as unknown as HTMLElement, source: source as unknown as ParentNode, sourceHeader };
  }

  test("only the matching game can adopt the staged hero and its existing nodes", () => {
    const { slot, element, source, sourceHeader } = heroSlot();
    adoptGameHeader(element, source, "other");
    expect(slot.firstElementChild).toBeNull();

    adoptGameHeader(element, source, "0123abcd");
    showGameHeader(element, gameHeaderModel(fixtureGame()));
    expect(slot.firstElementChild).toBe(sourceHeader);
    expect(slot.innerHTML).toBe("");
  });

  test("a changed game updates the same slot, and a missing game clears it", () => {
    const { slot, element, source } = heroSlot();
    const g = fixtureGame();
    adoptGameHeader(element, source, "0123abcd");
    g.scores[0]!.score = 16;
    g.teams[0] = 19;
    showGameHeader(element, gameHeaderModel(g));
    expect(slot.innerHTML).toContain("<b>19</b>");
    expect(slot.innerHTML).toContain('s-gamehead-pscore">16</b>');
    showGameHeader(element, null);
    expect(slot.firstElementChild).toBeNull();
    expect(slot.innerHTML).toBe("");
  });

  test("the initial HTML stages the hero and Game Details places it below the site header and breadcrumbs", async () => {
    const read = (path: string) => Bun.file(new URL(path, import.meta.url)).text();
    const [html, app, layout, game] = await Promise.all([
      read("../index.html"), read("../src/index.tsx"), read("../src/stats/Layout.tsx"), read("../src/stats/Game.tsx"),
    ]);
    expect(html).toMatch(/<template id="game-header-source">\s*<!-- game-header -->\s*<!-- \/game-header -->\s*<\/template>/);
    expect(app).toMatch(/path=\{STATS_ROOT\} component=\{StatsLayout\}>[\s\S]*?path="\/games\/:id" component=\{Game\}/);
    expect(layout).toMatch(/<header class="s-top">[\s\S]*?<main class="s-main">\{props.children\}<\/main>/);
    expect(game).toMatch(/<Crumbs trail=\{trail\(\)\} \/>\s*<div class="s-full s-game">\s*<div ref=\{hero\} class="s-game-hero" \/>/);
  });
});
