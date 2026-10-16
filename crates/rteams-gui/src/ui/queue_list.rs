//! Queued-messages panel (shown above the composer): list with Send / Edit / Delete and
//! reorder. Dispatch of due items is `QueueList::dispatch_idle`, called by the shell each tick.

use eframe::egui;
use rteams_services::queue::dispatch::{ack, take_due, take_now, Outgoing};
use rteams_services::queue::{Queue, QueueItem, QueueState};

use crate::actions::GuiCmd;
use crate::launcher::AgentStatus;
use crate::ui::FeatureCtx;
use crate::view::Intent;

fn tx(c: &FeatureCtx, key: &str, default: &str) -> String {
    if c.tr.has(key) {
        c.tr.t(key)
    } else {
        default.to_string()
    }
}

#[derive(Default)]
pub struct QueueList {
    team: String,
    queue: Option<Queue>,
    /// (item id, revision, text) being edited.
    editing: Option<(String, u32, String)>,
    error: Option<String>,
}

fn message_intent(o: &Outgoing) -> Intent {
    Intent::Run(GuiCmd::Message {
        to: o.to.clone(),
        text: o.text.clone(),
        mode: o.mode.clone(),
    })
}

impl QueueList {
    fn ensure(&mut self, home: &std::path::Path, team: &str) -> &mut Queue {
        if self.team != team || self.queue.is_none() {
            self.team = team.to_string();
            self.queue = Some(Queue::open(home, team));
            self.editing = None;
        }
        self.queue.as_mut().expect("just set")
    }

    /// Queue a message for later (composer "Add to queue").
    #[cfg(test)]
    pub fn enqueue(
        &mut self,
        home: &std::path::Path,
        team: &str,
        to: Option<&str>,
        text: &str,
        mode: Option<&str>,
        now: i64,
    ) -> Result<(), String> {
        self.ensure(home, team)
            .enqueue(to, text, mode, now)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Number of queued items.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.queue.as_ref().map_or(0, Queue::len)
    }

    /// Each tick: members whose run has ended (launched, not running) get their next queued
    /// message as an `Intent::Run(GuiCmd::Message)`. Cheap when the queue is empty (no IO).
    pub fn dispatch_idle(&mut self, c: &mut FeatureCtx, now: i64) {
        let (Some(team), Some(view)) = (c.team, c.view) else {
            return;
        };
        self.ensure(c.home, team);
        let q = self.queue.as_mut().expect("ensured");
        if q.is_empty() {
            return;
        }
        let idle: Vec<String> = view
            .agents
            .iter()
            .filter(|a| !matches!(a.status, AgentStatus::Running { .. }))
            .map(|a| a.member.clone())
            .collect();
        match take_due(q, &idle, true, now) {
            Ok(out) => {
                for o in out {
                    c.out.push(message_intent(&o));
                    let _ = ack(q, &o.id);
                }
            }
            Err(e) => {
                self.error = Some(e.to_string());
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let Some(team) = c.team else { return };
        let title = tx(c, "team.messages.manualQueue.title", "Queued messages");
        let (send, edit, delete, save, cancel) = (
            tx(c, "team.messages.manualQueue.send", "Send"),
            tx(c, "team.messages.manualQueue.edit", "Edit"),
            tx(c, "team.messages.manualQueue.delete", "Delete"),
            tx(c, "team.messages.manualQueue.save", "Save"),
            tx(c, "team.messages.manualQueue.cancel", "Cancel"),
        );
        let sending_label = tx(c, "team.messages.manualQueue.sending", "Sending");
        let unknown = tx(
            c,
            "team.messages.manualQueue.unknownWarning",
            "Delivery is unknown.",
        );
        self.ensure(c.home, team);
        let items: Vec<QueueItem> = self
            .queue
            .as_ref()
            .expect("ensured")
            .items()
            .into_iter()
            .cloned()
            .collect();
        if items.is_empty() && self.error.is_none() {
            return;
        }
        let mut action: Option<Act> = None;
        ui.group(|ui| {
            ui.strong(format!("{title} ({})", items.len()));
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            let last = items.len().saturating_sub(1);
            for (n, it) in items.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    let to = it.to.as_deref().unwrap_or("all");
                    ui.weak(format!("-> {to}"));
                    if let Some(m) = &it.mode {
                        ui.weak(m);
                    }
                    match it.state {
                        QueueState::Queued => {}
                        QueueState::Sending => {
                            ui.weak(&sending_label);
                        }
                        QueueState::Unknown => {
                            ui.colored_label(ui.visuals().warn_fg_color, &unknown);
                        }
                    }
                });
                if let Some((id, rev, text)) = self.editing.as_mut().filter(|e| e.0 == it.id) {
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(&save).clicked() {
                            action = Some(Act::Save(id.clone(), *rev, text.clone()));
                        }
                        if ui.button(&cancel).clicked() {
                            action = Some(Act::StopEdit);
                        }
                    });
                } else {
                    ui.label(&it.text);
                    ui.horizontal(|ui| {
                        let queued = it.state == QueueState::Queued;
                        if ui.add_enabled(queued, egui::Button::new(&send)).clicked() {
                            action = Some(Act::Send(it.id.clone()));
                        }
                        if ui.add_enabled(queued, egui::Button::new(&edit)).clicked() {
                            action = Some(Act::Edit(it.id.clone(), it.revision, it.text.clone()));
                        }
                        if ui
                            .add_enabled(queued && n > 0, egui::Button::new("Up"))
                            .clicked()
                        {
                            action = Some(Act::Move(it.id.clone(), true));
                        }
                        if ui
                            .add_enabled(queued && n < last, egui::Button::new("Down"))
                            .clicked()
                        {
                            action = Some(Act::Move(it.id.clone(), false));
                        }
                        if ui
                            .add_enabled(
                                it.state != QueueState::Sending,
                                egui::Button::new(&delete),
                            )
                            .clicked()
                        {
                            action = Some(Act::Delete(it.id.clone()));
                        }
                    });
                }
                ui.separator();
            }
        });
        let Some(a) = action else { return };
        let q = self.queue.as_mut().expect("ensured");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let res = match a {
            Act::Send(id) => take_now(q, &id, now).and_then(|o| {
                c.out.push(message_intent(&o));
                ack(q, &o.id)
            }),
            Act::Edit(id, rev, text) => {
                self.editing = Some((id, rev, text));
                Ok(())
            }
            Act::StopEdit => {
                self.editing = None;
                Ok(())
            }
            Act::Save(id, rev, text) => {
                let r = q.edit(&id, &text, rev);
                if r.is_ok() {
                    self.editing = None;
                }
                r
            }
            Act::Move(id, up) => q.move_item(&id, up),
            Act::Delete(id) => q.cancel(&id).map(|_| ()),
        };
        self.error = res.err().map(|e| e.to_string());
    }
}

enum Act {
    Send(String),
    Edit(String, u32, String),
    StopEdit,
    Save(String, u32, String),
    Move(String, bool),
    Delete(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{testing, Intents, View};
    use rteams_services::i18n::I18n;

    #[test]
    fn queue_list_renders_and_dispatches() {
        let (home, d) = testing::fixture("queue-list");
        let tr = I18n::english();
        let mut ql = QueueList::default();
        ql.enqueue(&home, "t", Some("ada"), "hello later", Some("do"), 1)
            .unwrap();
        ql.enqueue(&home, "t", Some("bob"), "bob later", None, 2)
            .unwrap();
        let agents: Vec<crate::launcher::AgentView> = vec![crate::launcher::AgentView {
            member: "ada".into(),
            status: AgentStatus::Exited { code: Some(0) },
            last_lines: vec![],
            rate_limited_until: None,
            last_error: None,
            runtime: "claude".into(),
        }];
        let view = View {
            state: d.state(),
            data: &d,
            actor: Some("boss"),
            agents: &agents,
            tr: crate::view::testing::tr(),
        };
        let mut out = Intents::default();
        let mut req = vec![];
        testing::render(|ui| {
            let mut c = FeatureCtx {
                home: &home,
                team: Some("t"),
                view: Some(&view),
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            ql.show(ui, &mut c);
        });
        assert_eq!(ql.len(), 2);
        let mut c = FeatureCtx {
            home: &home,
            team: Some("t"),
            view: Some(&view),
            out: &mut out,
            tr: &tr,
            req: &mut req,
        };
        ql.dispatch_idle(&mut c, 10);
        // only ada (idle, launched) dispatches; bob is offline and stays queued
        assert_eq!(out.0.len(), 1);
        assert_eq!(ql.len(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }
}
