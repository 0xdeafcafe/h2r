//! The memory reads live in agent-proto so the editor shares them
//! without linking the game agent.
pub use agent_proto::memory_serve::{call, snapshot_reply, supports};
