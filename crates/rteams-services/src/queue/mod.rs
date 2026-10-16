//! Manual message queue: messages the human queues for an agent that is busy/offline.
//! Shell, each tick: `dispatch::take_due(&mut q, idle_members, true, now)` -> send each
//! `Outgoing` as `GuiCmd::Message` (through the ledger with `message_id`), then
//! `dispatch::ack` / `dispatch::nack`. On startup call `dispatch::reconcile`.

pub mod dispatch;
pub mod model;
pub mod store;

pub use dispatch::Outgoing;
pub use model::{QueueError, QueueItem, QueueState};
pub use store::Queue;
