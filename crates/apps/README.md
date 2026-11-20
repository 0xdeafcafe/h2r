# crates/apps

What links a binary: the composition root, the desktop shell, the `h2` command line, the editor and the embedded hosts for Apple devices and the Xbox. An app crate may depend on every other domain and nothing depends on it, so this is where the whole game is wired together. The rules are in [crate-structure.md](../../docs/dev/crate-structure.md) and are checked by `scripts/crate-graph.py layers --check`.

| Crate | What it is |
|---|---|
| [`app`](app/) | The host shell: window, presentation, input, audio device and guest-thread lifecycle. |
| [`app-icon`](app-icon/) | The dock icon for the unbundled Mac windows. |
| [`boot`](boot/) | The composition root: runs a built module with the kernel, the hooks and every feature attached. |
| [`cli`](cli/) | The `h2` binary and all its commands. |
| [`h2host`](h2host/) | The Rust half of the tvOS and iOS apps. |
| [`studio`](studio/) | The `h2-editor` binary: the editor suite on Dear ImGui and wgpu. |
| [`studio-proto`](studio-proto/) | The studio agent API's names, short forms and client. |
| [`uwp-host`](uwp-host/) | The Xbox (UWP, Dev Mode) app and the Mac-side packaging helpers. |
