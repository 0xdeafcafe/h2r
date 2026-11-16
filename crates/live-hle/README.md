# Xbox Live client HLE

Opt-in public API overrides for Halo 2 retail XONLINES/XNet 5849. `entries.rs`
is the address/name/stack-word inventory from `docs/live-boundary.md`. Sign-in
operations live in `state.rs`; `network.rs` implements host sockets, keys and
address routing, with platform FFI confined to `host.rs`. `client.rs` owns the
HTTP worker, `invites.rs` implements cached friends, invite messages and accepted
joins, `messages.rs` rejects unsupported message attachments, and `ui.rs` provides
the remaining empty menu views. `storage_api.rs` implements the path formatter
and asynchronous title-resource downloads. QoS listener traffic and guest result
allocation live in `qos.rs` and `qos_memory.rs`. Every remaining
boundary is a named runtime stop. No executable bytes or lookup tables are embedded.

`Hle::new(LiveConfig)` reads account identities once and chooses a host LAN
endpoint. `Hle::with_address` supplies an explicit endpoint, while
`Hle::with_network` also selects local discovery peers. Create one Hle per
Runtime, call `register` on its Dispatch before execution, and configure the
kernel's `ConsoleConfig::phy_link` when enabled. Boot's `live_glue::install`
does both and reads the optional host-network environment settings. Disabled
configuration installs nothing.

Calls use checked guest memory and stdcall cleanup. No guest-memory or HLE lock
is retained across socket waits. Socket failures use guest-thread Winsock last
errors; malformed frames and sign-in memory failures use GuestFault. Task
handles and the stable user array occupy a bounded claimed allocation in the
existing shared HLE window. Peer QoS results use a separate reusable allocation.
Unsupported APIs preserve CPU/stack state and emit `Event::Unsupported`.

See `docs/live.md`'s "Client HLE" sections for configuration, channel ports,
System Link's two-instance setup and exact stub behavior. The host transport
is plaintext between HLE clients, without Xbox wire-protocol interoperability.
With a server URL, logon verifies accounts and publishes signed presence on a
background thread. Session advertisements and signed mailbox invitations use that
worker too; acceptance feeds the game's own session lookup and socket path.
Private account keys stay in host memory and are never logged or copied to the
guest. Without a server URL, local sign-in remains available.

Run `cargo test -p live-hle` for synthetic boundary and real loopback tests;
the latter require permission to bind localhost sockets. No XBE is needed.
