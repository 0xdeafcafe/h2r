// Render the dashboard controls with the same Solid JSX transform as the app.
import { beforeAll, expect, test } from "bun:test";
import { plugin } from "bun";

let render: typeof import("solid-js/web").renderToString;
let component: typeof import("solid-js").createComponent;
let views: typeof import("../src/dash/InviteGrant");

beforeAll(async () => {
  const babel = await import("@babel/core");
  const solid = (await import("babel-preset-solid")).default;
  plugin({
    name: "invite-grant-ssr",
    setup(build) {
      build.onLoad({ filter: /\/src\/dash\/InviteGrant\.tsx$/ }, async ({ path }) => {
        const out = await babel.transformAsync(await Bun.file(path).text(), {
          filename: path, babelrc: false, configFile: false,
          parserOpts: { plugins: ["jsx", "typescript"] },
          presets: [[solid, { generate: "ssr", hydratable: false }]],
        });
        return { contents: out!.code!, loader: "ts" };
      });
    },
  });
  ({ renderToString: render } = await import("solid-js/web"));
  ({ createComponent: component } = await import("solid-js"));
  views = await import("../src/dash/InviteGrant");
});

test("the player invite grant switch and cap appear only for admins", () => {
  const control = (admin: boolean, grant: { open_cap: number | null } | null) =>
    render(() => component(views.InviteGrantControl, { admin, grant, write: async () => {} }));

  expect(control(false, { open_cap: 4 })).not.toContain('role="switch"');
  expect(control(false, { open_cap: 4 })).not.toContain("open invite cap");
  const off = control(true, null);
  expect(off).toContain('role="switch"');
  expect(off).toContain('aria-checked="false"');
  expect(off).toContain("can invite · off");
  expect(off).toContain('placeholder="no cap"');
  const on = control(true, { open_cap: 4 });
  expect(on).toContain('aria-checked="true"');
  expect(on).toContain("can invite · on");
  expect(on).toContain("open invite cap");
  expect(on).toContain('value="4"');
});

test("the dashboard invite entry appears with a grant, with or without a cap", () => {
  const entry = (grant: { open_cap: number | null } | null) =>
    render(() => component(views.InviteSomeone, { grant, go: () => {} }));
  expect(entry(null)).not.toContain("invite someone");
  for (const cap of [2, null]) {
    const html = entry({ open_cap: cap });
    expect(html).toContain('href="/dash/server/my-invites"');
    expect(html).toContain(">invite someone</a>");
  }
});
