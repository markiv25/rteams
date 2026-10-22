//! Notification centre: a history of what happened (newest first) with read state, plus the
//! per-type toggles that decide what is recorded and shown. Toggles persist in
//! `<home>/<team>/gui-state.json` under `"notify"`, next to other modules' keys.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Align2, RichText};
use serde_json::{json, Value};

use crate::notify::{Note, NotifyKind};
use crate::theme;
use crate::view::Intents;
use rteams_services::i18n::I18n;

const MAX_HISTORY: usize = 200;

/// One toggle per notification type; all on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyPrefs {
    pub task_done: bool,
    pub review_requested: bool,
    pub messages: bool,
    pub comments: bool,
    pub blocked: bool,
    pub approvals: bool,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        NotifyPrefs {
            task_done: true,
            review_requested: true,
            messages: true,
            comments: true,
            blocked: true,
            approvals: true,
        }
    }
}

impl NotifyPrefs {
    const KEYS: [&'static str; 6] = [
        "task_done",
        "review_requested",
        "messages",
        "comments",
        "blocked",
        "approvals",
    ];

    fn flags(&mut self) -> [&mut bool; 6] {
        [
            &mut self.task_done,
            &mut self.review_requested,
            &mut self.messages,
            &mut self.comments,
            &mut self.blocked,
            &mut self.approvals,
        ]
    }

    /// Missing or non-boolean keys keep their default (on).
    fn from_json(v: &Value) -> Self {
        let mut p = NotifyPrefs::default();
        for (key, flag) in Self::KEYS.iter().zip(p.flags()) {
            if let Some(b) = v.get(key).and_then(Value::as_bool) {
                *flag = b;
            }
        }
        p
    }

    fn to_json(mut self) -> Value {
        let mut m = serde_json::Map::new();
        for (key, flag) in Self::KEYS.iter().zip(self.flags()) {
            m.insert((*key).to_string(), Value::Bool(*flag));
        }
        Value::Object(m)
    }

    pub fn enabled(&self, kind: NotifyKind) -> bool {
        match kind {
            NotifyKind::TaskDone => self.task_done,
            NotifyKind::ReviewRequested => self.review_requested,
            NotifyKind::Message => self.messages,
            NotifyKind::Comment => self.comments,
            NotifyKind::Blocked => self.blocked,
            NotifyKind::Approval => self.approvals,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub title: String,
    pub body: String,
    pub ts: i64,
    pub read: bool,
}

#[derive(Debug, Default)]
pub struct NotificationCenter {
    items: Vec<Item>,
    prefs: NotifyPrefs,
    is_open: bool,
    path: Option<PathBuf>,
    test_sent: bool,
}

/// Read the `"notify"` key of a gui-state file (defaults when missing or unreadable).
pub fn load_prefs(path: &Path) -> NotifyPrefs {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("notify").map(NotifyPrefs::from_json))
        .unwrap_or_default()
}

/// Merge `prefs` into the file under `"notify"`, keeping every other key.
pub fn save_prefs(path: &Path, prefs: &NotifyPrefs) {
    let mut root = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    root["notify"] = prefs.to_json();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&root) {
        let _ = std::fs::write(path, text);
    }
}

impl NotificationCenter {
    /// Bind to `<home>/<team>/gui-state.json` and load the saved toggles. History is
    /// per-session, so it is cleared (call on team switch).
    pub fn load(&mut self, home: &Path, team: &str) {
        let path = home.join(team).join("gui-state.json");
        self.prefs = load_prefs(&path);
        self.path = Some(path);
        self.items.clear();
    }

    /// Whether notifications of `kind` are switched on.
    pub fn enabled(&self, kind: NotifyKind) -> bool {
        self.prefs.enabled(kind)
    }

    /// Record a notification (newest last internally; shown newest first).
    pub fn push(&mut self, title: &str, body: &str, ts: i64) {
        self.items.push(Item {
            title: title.to_string(),
            body: body.to_string(),
            ts,
            read: false,
        });
        let extra = self.items.len().saturating_sub(MAX_HISTORY);
        self.items.drain(..extra);
    }

    /// Record `note` if its type is enabled; returns whether it was recorded.
    pub fn push_note(&mut self, note: &Note) -> bool {
        let on = self.enabled(note.kind);
        if on {
            self.push(&note.title, &note.body, note.ts);
        }
        on
    }

    pub fn unread(&self) -> usize {
        self.items.iter().filter(|i| !i.read).count()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn mark_all_read(&mut self) {
        self.items.iter_mut().for_each(|i| i.read = true);
    }

    pub fn open(&mut self) {
        self.is_open = true;
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// The per-type toggles (used by Settings > Notifications). Saves on change.
    pub fn prefs_ui(&mut self, ui: &mut egui::Ui, tr: &I18n) {
        let before = self.prefs;
        let p = &mut self.prefs;
        ui.label(RichText::new(tr.t("rteams.notify.about")).color(theme::TEXT_2));
        ui.checkbox(&mut p.task_done, tr.t("rteams.notify.taskDone"));
        ui.checkbox(
            &mut p.review_requested,
            tr.t("team.taskDetail.workflowTimeline.reviewRequested"),
        );
        ui.checkbox(&mut p.messages, tr.t("team.messages.title"));
        ui.checkbox(&mut p.comments, tr.t("rteams.notify.comments"));
        ui.checkbox(&mut p.blocked, tr.t("rteams.notify.blocked"));
        ui.checkbox(&mut p.approvals, tr.t("rteams.notify.approvals"));
        if self.prefs != before {
            if let Some(path) = &self.path {
                save_prefs(path, &self.prefs);
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tr.t("settings.notifications.test.label")).strong());
        ui.label(
            RichText::new(tr.t("settings.notifications.test.description")).color(theme::TEXT_2),
        );
        if theme::pill_button(ui, &tr.t("settings.notifications.test.action"), false).clicked() {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            let (title, body) = self.record_test(ts);
            crate::notify::notify(&title, &body);
            self.test_sent = true;
        }
        if self.test_sent {
            ui.colored_label(theme::SUCCESS, tr.t("settings.notifications.test.sent"));
        }
    }

    /// The "send test notification" action: adds a history entry (ignoring the per-type
    /// toggles, since it tests delivery) and returns the text to show as an OS notification.
    pub fn record_test(&mut self, ts: i64) -> (String, String) {
        let (title, body) = ("rteams".to_string(), "Test notification".to_string());
        self.push(&title, &body, ts);
        (title, body)
    }

    /// The dropdown window, anchored top right. Opening it does not mark anything read.
    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n, _out: &mut Intents) {
        if !self.is_open {
            return;
        }
        let screen = ctx.content_rect();
        let width = (screen.width() - 24.0).clamp(1.0, 380.0);
        let height = (screen.height() - 80.0).clamp(1.0, 460.0);
        let mut open = true;
        egui::Window::new(tr.t("common.notifications.title"))
            .open(&mut open)
            .anchor(Align2::RIGHT_TOP, [-12.0, 48.0])
            .fixed_size([width, height.min(320.0)])
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(
                            tr.t_count("common.notifications.counts.unread", self.unread() as u64),
                        )
                        .size(12.0)
                        .color(theme::MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(tr.t("common.notifications.actions.markAllRead"))
                            .clicked()
                        {
                            self.mark_all_read();
                        }
                    });
                });
                theme::rule(ui);
                if self.items.is_empty() {
                    ui.label(RichText::new(tr.t("rteams.notify.nothing")).color(theme::MUTED));
                    return;
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for i in self.items.iter().rev() {
                            ui.horizontal_wrapped(|ui| {
                                let dot = if i.read { theme::MUTED } else { theme::ACCENT };
                                ui.label(RichText::new("•").size(10.0).color(dot));
                                ui.label(RichText::new(&i.title).strong());
                                ui.label(
                                    RichText::new(theme::hhmm(i.ts))
                                        .size(11.0)
                                        .color(theme::MUTED),
                                );
                            });
                            ui.label(RichText::new(&i.body).size(12.5).color(theme::TEXT_2));
                            theme::rule(ui);
                        }
                    });
            });
        if !open {
            self.is_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::render;

    #[test]
    fn history_unread_and_filtering() {
        let mut c = NotificationCenter::default();
        assert_eq!(c.unread(), 0);
        c.push("a", "b", 1);
        c.push("c", "d", 2);
        assert_eq!((c.unread(), c.len()), (2, 2));
        c.mark_all_read();
        assert_eq!(c.unread(), 0);
        c.prefs.blocked = false;
        let n = Note {
            kind: NotifyKind::Blocked,
            title: "t".into(),
            body: "b".into(),
            ts: 3,
        };
        assert!(!c.push_note(&n));
        assert!(c.push_note(&Note {
            kind: NotifyKind::Message,
            ..n
        }));
        assert_eq!(c.unread(), 1);
        for i in 0..300 {
            c.push("x", "y", i);
        }
        assert_eq!(c.len(), MAX_HISTORY);
    }

    #[test]
    fn test_notification_is_recorded_even_when_all_off() {
        let mut c = NotificationCenter::default();
        c.prefs.task_done = false;
        c.prefs.messages = false;
        let (t, b) = c.record_test(9);
        assert_eq!((t.as_str(), b.as_str()), ("rteams", "Test notification"));
        assert_eq!((c.len(), c.unread()), (1, 1));
    }

    #[test]
    fn prefs_persist_and_keep_other_keys() {
        let dir = std::env::temp_dir().join(format!("rteams-gui-notif-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("t")).unwrap();
        let file = dir.join("t").join("gui-state.json");
        std::fs::write(&file, r#"{"read":{"group":4}}"#).unwrap();
        assert_eq!(load_prefs(&file), NotifyPrefs::default());
        let p = NotifyPrefs {
            messages: false,
            ..Default::default()
        };
        save_prefs(&file, &p);
        assert_eq!(load_prefs(&file), p);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["read"]["group"], 4);
        let mut c = NotificationCenter::default();
        c.load(&dir, "t");
        assert!(!c.enabled(NotifyKind::Message));
        assert!(c.enabled(NotifyKind::Comment));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn renders_empty_and_filled() {
        let mut c = NotificationCenter::default();
        c.open();
        render(|ui| {
            c.show(
                ui.ctx(),
                crate::view::testing::tr(),
                &mut Intents::default(),
            )
        });
        c.push(
            "Task done",
            "#1 something quite long ".repeat(10).as_str(),
            3_600_000,
        );
        render(|ui| {
            c.show(
                ui.ctx(),
                crate::view::testing::tr(),
                &mut Intents::default(),
            );
            c.prefs_ui(ui, crate::view::testing::tr());
        });
        assert!(c.is_open());
    }
}
