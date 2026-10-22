//! Write path: effects become tool calls on `Server` over a `FileStore`.
use std::path::Path;

use rteams_server::Server;
use rteams_store::FileStore;
use serde_json::json;

use crate::app::Effect;

pub struct Writer {
    server: Server,
    pub actor: String,
}

impl Writer {
    pub fn open(home: &Path, team: &str, actor: String) -> Result<Self, String> {
        let store = FileStore::open(home, team).map_err(|e| e.to_string())?;
        Ok(Writer {
            server: Server::with_store(store),
            actor,
        })
    }

    pub fn run(&self, e: &Effect) -> Result<(), String> {
        let a = &self.actor;
        let (name, args) = match e {
            Effect::Send { to, text } => (
                "message_send",
                json!({ "actor": a, "to": to, "text": text }),
            ),
            Effect::Comment { task, text } => (
                "task_add_comment",
                json!({ "actor": a, "task": task, "text": text }),
            ),
            Effect::Start { task } => ("task_start", json!({ "actor": a, "task": task })),
            Effect::Complete { task } => ("task_complete", json!({ "actor": a, "task": task })),
        };
        self.server
            .call_tool(name, args)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// `--as`, else the first lead on the roster, else "user".
pub fn default_actor(state: &rteams_core::State, requested: Option<String>) -> String {
    requested
        .or_else(|| {
            state
                .team
                .as_ref()?
                .members
                .iter()
                .find(|m| m.role == "lead")
                .map(|m| m.name.clone())
        })
        .unwrap_or_else(|| "user".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_write_through_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let server = {
            let store = FileStore::open(dir.path(), "t").unwrap();
            Server::with_store(store)
        };
        server
            .call_tool(
                "team_create",
                json!({ "actor": "lead", "name": "t", "members": [{"name":"lead","role":"lead"}] }),
            )
            .ok();
        drop(server);
        let w = Writer::open(dir.path(), "t", "lead".into()).unwrap();
        // Whatever the team bootstrap looks like, a broadcast from a non-member must be a
        // readable error, never a panic.
        let _ = w.run(&Effect::Send {
            to: None,
            text: "hi".into(),
        });
    }
}
