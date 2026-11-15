# Xbox Live client HLE

Opt-in public API overrides for Halo 2 retail XONLINES 5849. `entries.rs` is
the address/name/stack-word inventory from `docs/live-boundary.md`; implemented
operations live in `state.rs`, while every other boundary is a named runtime
stop. No executable bytes, credentials or lookup tables are embedded.

`Hle::new(LiveConfig)` reads account identities once and chooses a host LAN
endpoint. `Hle::with_address` supplies an explicit endpoint for tests/embedding.
Create one Hle per Runtime, call `register` on its Dispatch before execution,
and configure the kernel's `ConsoleConfig::phy_link` when enabled. Boot's
`live_glue::install` does both. Disabled configuration installs nothing.

Calls use checked guest memory and stdcall cleanup, with guest-memory then
Live-state lock order. Handles and the stable user array occupy a bounded
claimed allocation in the existing shared HLE window. No guard escapes a call.
Unsupported APIs and unverified argument variants preserve CPU/stack state and
emit `Event::Unsupported`; guest-memory failures propagate as GuestFault.

See `docs/live.md`'s "Client HLE: sign-in" for configuration and exact stub
behavior. This milestone stores `server_url` without making service requests.
Accounts' signing-key fields are ignored, never retained or logged.

Run `cargo test -p live-hle` for synthetic boundary and lifecycle tests.
