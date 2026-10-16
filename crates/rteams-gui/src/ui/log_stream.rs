//! Live merged agent output ("Claude Logs"): tails every member's newest recorded run, merges
//! by time, and filters by member / kind / text. Virtualized, so long logs stay cheap.

use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use rteams_services::logtail::cache::ReadMarks;
use rteams_services::logtail::merge::{self, Filter, MERGED_CAP};
use rteams_services::logtail::stream::TeamTail;
use rteams_services::logtail::{LogEntry, LogKind};

use crate::launcher::AgentStatus;
use crate::theme;
use crate::ui::schedules::run_log::kind_color;
use crate::ui::FeatureCtx;

const POLL: Duration = Duration::from_millis(1000);
const KINDS: [LogKind; 5] = [
    LogKind::Text,
    LogKind::ToolUse,
    LogKind::ToolResult,
    LogKind::Thinking,
    LogKind::Error,
];

pub struct LogStream {
    team: Option<String>,
    tail: TeamTail,
    merged: Vec<LogEntry>,
    filter: Filter,
    follow: bool,
    marks: ReadMarks,
    polled: Option<Instant>,
}

impl Default for LogStream {
    fn default() -> Self {
        LogStream {
            team: None,
            tail: TeamTail::default(),
            merged: vec![],
            filter: Filter::default(),
            follow: true,
            marks: ReadMarks::default(),
            polled: None,
        }
    }
}

fn kind_icon(k: LogKind) -> crate::icons::Icon {
    use crate::icons::Icon;
    match k {
        LogKind::Thinking => Icon::Eye,
        LogKind::ToolUse => Icon::Code,
        LogKind::ToolResult | LogKind::Result => Icon::Check,
        LogKind::Error => Icon::X,
        LogKind::Text => Icon::MessageSquare,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Members that have a `runs/<member>` directory.
fn recorded_members(home: &std::path::Path, team: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(home.join(team).join("runs"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    v.sort();
    v
}

impl LogStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// Entries currently held (merged, unfiltered).
    #[cfg(test)]
    pub fn entries(&self) -> &[LogEntry] {
        &self.merged
    }

    /// Entries produced by `member` since the log was last viewed.
    #[cfg(test)]
    pub fn unread(&self, member: &str) -> u64 {
        self.marks
            .unread(member, self.tail.members.get(member).map_or(0, |m| m.total))
    }

    fn poll(&mut self, home: &std::path::Path, team: &str) {
        if self.team.as_deref() != Some(team) {
            *self = LogStream {
                filter: std::mem::take(&mut self.filter),
                ..LogStream::default()
            };
            self.team = Some(team.to_string());
            self.marks = ReadMarks::load(home, team);
        }
        if self.polled.is_some_and(|p| p.elapsed() < POLL) {
            return;
        }
        self.polled = Some(Instant::now());
        let members = recorded_members(home, team);
        if !self.tail.poll(home, team, &members, now_ms()).is_empty() || self.merged.is_empty() {
            let per: Vec<Vec<LogEntry>> = members.iter().map(|m| self.tail.entries(m)).collect();
            self.merged = merge::merge(per, MERGED_CAP);
        }
    }

    /// Draw the panel.
    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let tr = c.tr;
        let Some(team) = c.team else {
            ui.label(RichText::new(tr.t("team.claudeLogs.noLogsYet")).color(theme::MUTED));
            return;
        };
        self.poll(c.home, team);
        let running = c.view.is_some_and(|v| {
            v.agents
                .iter()
                .any(|a| matches!(a.status, AgentStatus::Running { .. }))
        });

        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.filter.query)
                    .hint_text(tr.t("team.claudeLogs.searchPlaceholder"))
                    .desired_width(200.0),
            );
            let all = self.filter.members.is_empty();
            if ui
                .selectable_label(all, tr.t("team.memberLogStream.filters.all"))
                .clicked()
            {
                self.filter.members.clear();
            }
            let mut names: Vec<String> = self.tail.members.keys().cloned().collect();
            names.sort();
            for m in names {
                let on = self.filter.members.contains(&m);
                if ui.selectable_label(on, &m).clicked() {
                    if on {
                        self.filter.members.retain(|x| x != &m);
                    } else {
                        self.filter.members.push(m);
                    }
                }
            }
            for k in KINDS {
                let hidden = self.filter.hide.contains(&k);
                if ui.selectable_label(!hidden, k.label()).clicked() {
                    if hidden {
                        self.filter.hide.retain(|x| *x != k);
                    } else {
                        self.filter.hide.push(k);
                    }
                }
            }
            ui.checkbox(&mut self.follow, "Follow");
        });
        ui.separator();

        let rows = merge::filter(&self.merged, &self.filter);
        if rows.is_empty() {
            let key = if self.merged.is_empty() && !running {
                "team.claudeLogs.teamNotRunning"
            } else {
                "team.claudeLogs.noLogsYet"
            };
            ui.label(RichText::new(tr.t(key)).color(theme::MUTED));
        } else {
            let h = 30.0;
            let mut area = egui::ScrollArea::vertical()
                .id_salt("log-stream")
                .auto_shrink([false, false]);
            if self.follow {
                area = area.stick_to_bottom(true);
            }
            ui.spacing_mut().item_spacing.y = 3.0;
            area.show_rows(ui, h, rows.len(), |ui, range| {
                for e in &rows[range] {
                    let color = kind_color(e.kind);
                    let icon = kind_icon(e.kind);
                    let w = ui.available_width().max(1.0);
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(9),
                        theme::tint(color, 0.05),
                    );
                    let tile = egui::Rect::from_center_size(
                        egui::pos2(rect.left() + 18.0, rect.center().y),
                        egui::vec2(22.0, 22.0),
                    );
                    ui.painter().rect_filled(
                        tile,
                        egui::CornerRadius::same(6),
                        theme::tint(color, 0.16),
                    );
                    crate::icons::draw(
                        ui.painter(),
                        egui::Rect::from_center_size(tile.center(), egui::vec2(13.0, 13.0)),
                        icon,
                        color,
                    );
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(egui::Rect::from_min_max(
                                egui::pos2(tile.right() + 10.0, rect.top()),
                                egui::pos2(rect.right() - 10.0, rect.bottom()),
                            ))
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    child.spacing_mut().item_spacing.x = 8.0;
                    child.label(RichText::new(&e.member).strong().size(12.5).color(
                        theme::member_color(c.view.and_then(|v| v.team()), &e.member),
                    ));
                    child.add(
                        egui::Label::new(RichText::new(&e.text).size(12.5).color(color)).truncate(),
                    );
                    let time = theme::hhmm(e.ts_ms);
                    ui.painter().text(
                        egui::pos2(rect.right() - 10.0, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        time,
                        egui::FontId::proportional(11.5),
                        theme::MUTED,
                    );
                }
            });
        }
        // Everything shown counts as read.
        let totals: Vec<(String, u64)> = self
            .tail
            .members
            .iter()
            .map(|(m, t)| (m.clone(), t.total))
            .collect();
        let mut dirty = false;
        for (m, t) in totals {
            if self.marks.unread(&m, t) > 0 {
                self.marks.mark(&m, t);
                dirty = true;
            }
        }
        if dirty {
            let _ = self.marks.save(c.home, team);
        }
        ui.ctx().request_repaint_after(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{testing, Intents};
    use rteams_services::i18n::I18n;

    fn write_run(home: &std::path::Path, member: &str, ms: u64, text: &str) {
        let dir = home.join("t/runs").join(member);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{ms}.jsonl")),
            format!("{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n"),
        )
        .unwrap();
    }

    #[test]
    fn tails_merges_and_renders() {
        let d = tempfile::tempdir().unwrap();
        write_run(d.path(), "ada", 1000, "from ada");
        write_run(d.path(), "bob", 2000, "from bob");
        let tr = I18n::english();
        let mut ls = LogStream::new();
        testing::render(|ui| {
            let mut out = Intents::default();
            let mut req = vec![];
            let mut c = FeatureCtx {
                home: d.path(),
                team: Some("t"),
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            ls.show(ui, &mut c);
        });
        let texts: Vec<_> = ls.entries().iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["from ada", "from bob"]);
        // Viewing marked everything read.
        assert_eq!(ls.unread("ada"), 0);
        assert_eq!(ReadMarks::load(d.path(), "t").unread("bob", 1), 0);
    }

    #[test]
    fn empty_and_teamless_render() {
        let d = tempfile::tempdir().unwrap();
        let tr = I18n::english();
        let mut ls = LogStream::new();
        testing::render(|ui| {
            let mut out = Intents::default();
            let mut req = vec![];
            let mut c = FeatureCtx {
                home: d.path(),
                team: Some("t"),
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            ls.show(ui, &mut c);
            let mut c2 = FeatureCtx {
                home: d.path(),
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            ls.show(ui, &mut c2);
        });
        assert!(ls.entries().is_empty());
    }
}
