//! Co-op over Live, phase 0 (docs/live/live-coop.md): what two machines need to
//! run the same game in step, with no guest and no game data in it.
//!
//! - [`wire`]: the one datagram the peers exchange. It carries the sender's
//!   pad frames and tick hashes that the other side has not acknowledged
//!   yet, so a lost datagram costs nothing and delivery is in order.
//! - [`link`]: where datagrams go. [`link::UdpLink`] is the direct path (the
//!   XNet base port plus [`link::COOP_PORT_OFFSET`]), [`relay::RelayLink`]
//!   goes through the Live server, [`link::Fallback`] uses the relay while
//!   the direct path is silent, and [`link::sim_pair`] is a simulated link
//!   (latency, jitter, loss, reordering) on a fake clock for tests.
//! - [`schedule`]: the two-port pad schedule. Frame *f* has one pad per
//!   port; a machine's own samples are assigned `delay` frames ahead.
//! - [`peer`]: one machine's end: schedule, link and hash comparison. A
//!   frame may start only when both ports' pads for it are in hand.
//! - [`join`]: join in progress: a newcomer asks, the host holds its tick,
//!   snapshots and sends the state in chunks, and admits the player on the
//!   next tick once the newcomer has restored it.
//! - [`tap`]: the schedule as an `xapi::tap::InputTap`, so every
//!   `XInputGetState` is answered from the agreed frame and never from the
//!   local device.
//!
//! Nothing here reads a clock or a device by itself: the caller says when a
//! frame starts and passes the local pad in, which is what makes it testable.

pub mod join;
pub mod link;
pub mod peer;
pub mod relay;
pub mod schedule;
pub mod tap;
pub mod wire;

pub use peer::{Desync, Hold, Peer, Stats};
pub use schedule::{PAD, Pad, Schedule};
