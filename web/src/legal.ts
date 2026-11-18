// The Microsoft Game Content Usage Rules notice, in one place for both
// sides (the dashboard's menu and the stats footer). The rules ask for this
// wording on a non-commercial fan work that uses Microsoft's game assets.
// Keep the sentence as written: it is the licence the project relies on.

/** The rules themselves. */
export const RULES_URL = "https://www.xbox.com/en-US/developers/rules";

/** The words linked to [`RULES_URL`]. */
export const RULES_NAME = "Game Content Usage Rules";

/** The notice as three runs, so the middle one can be a link:
 *  `before` + (open quote) `RULES_NAME` (close quote) + `after`. */
export function noticeParts(name: string): { before: string; link: string; after: string } {
  return {
    before: `${name} was created under Microsoft's "`,
    link: RULES_NAME,
    after: `" using assets from Halo 2 and Halo: Combat Evolved, and it is not endorsed by or affiliated with Microsoft.`,
  };
}

/** The other two lines the notice comes with. */
export const COPYRIGHT = "Halo © Microsoft Corporation.";
export const FAN_PROJECT = "This is a free, non-commercial fan project.";

/** The whole notice as plain text, for a README or a test. */
export function noticeText(name: string): string {
  const p = noticeParts(name);
  return `${p.before}${p.link}${p.after} ${COPYRIGHT} ${FAN_PROJECT}`;
}
