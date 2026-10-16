//! Member work sync. Shell, on events and every ~60 s:
//! `WorkSync::new(home, team).nudges(state, eligible, running, now)` -> for each `Nudge` queue a
//! `WakeOrder` (reason = `nudge.reason`, prompt = `wake::build_prompt`). Nothing else spawns.

pub mod agenda;
pub mod diagnostics;
pub mod fingerprint;
pub mod nudge;
pub mod reconcile;
pub mod store;

pub use agenda::{build_agenda, Agenda, AgendaItem};
pub use diagnostics::{diagnose, MemberDiagnostic};
pub use nudge::{Nudge, WorkSync};
pub use reconcile::Decision;
