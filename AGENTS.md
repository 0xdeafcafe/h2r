# Notes for coding agents

## Use the agent API

When you need to see, drive or check the running game or the editor, use
the agent API. Don't screen-scrape, read process memory by hand, drive the
UI with synthetic input, or add one-off debug hooks.

- The game: `h2 agent` ([docs/runtime/agent-api.md](docs/runtime/agent-api.md)).
- The editor: `h2-editor`'s socket API ([docs/editor/agent-api.md](docs/editor/agent-api.md)).

`session.methods` lists every method each API offers.

If the API can't do what you need, don't route around it. Decide whether
the API should be expanded to cover it:

- **Expand it** when the need will come up again: a new view of game or
  editor state, a new action, or a new check. Add the method with its
  params, result and tests, and document it in the API's doc. Then use it.
- **Don't expand it** only for a genuine one-off that no later agent or
  test will need. Say in your report why the API wasn't used and why it
  wasn't expanded.

In both cases, record the decision in your report or commit message.
