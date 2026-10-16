//! Team detail header, offline/failure banner and the member list (&v.tr.t("common.layout.sections.team") section).

use std::cell::Cell;

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Stroke, Vec2};
use rteams_core::Status;
use rteams_services::i18n::I18n;

use crate::actions::GuiCmd;
use crate::icons::{self, Icon};
use crate::launcher::AgentStatus;
use crate::theme;
use crate::view::{Intent, Intents, View};

const GLYPH_POWER: &str = "⚡";
const GLYPH_WRENCH: &str = "\u{1F527}";
const GLYPH_USERS: &str = "\u{1F465}";

/// Status chip text and color for one member's launcher state (`None` = never launched).
pub fn status_chip(status: Option<&AgentStatus>) -> (String, Color32) {
    match status {
        None => ("idle".into(), theme::MUTED),
        Some(AgentStatus::Running { .. }) => ("running".into(), theme::SUCCESS),
        Some(AgentStatus::Exited { code: Some(0) }) => ("finished".into(), theme::COL_DONE),
        Some(AgentStatus::Exited { code: Some(n) }) => (format!("exit {n}"), theme::ERROR),
        Some(AgentStatus::Exited { code: None }) => ("exit ?".into(), theme::ERROR),
        Some(AgentStatus::Failed(_)) => ("failed".into(), theme::ERROR),
    }
}

/// Localized display text for the English state words produced by the helpers above.
pub fn loc_state(tr: &I18n, s: &str) -> String {
    let key = match s {
        "idle" => "team.agentGraph.popover.member.state.idle",
        "active" => "team.agentGraph.popover.member.state.active",
        "running" => "team.processes.running",
        "stopped" => "team.processes.stopped",
        "finished" => "rteams.team.finished",
        "failed" => "team.agentGraph.popover.member.spawn.failed",
        "lead" => "team.agentGraph.popover.member.lead",
        _ => return s.to_string(),
    };
    let t = tr.t(key);
    // The catalog capitalizes some of these; the chips are lowercase.
    if s.chars().next().is_some_and(char::is_lowercase) {
        t.to_lowercase()
    } else {
        t
    }
}

/// Bell button text: the bell, plus the unread count when there is one.
#[cfg_attr(not(test), allow(dead_code))]
pub fn bell_label(unread: usize) -> String {
    match unread {
        0 => "\u{1F514}".to_string(),
        n if n > 99 => "\u{1F514} 99+".to_string(),
        n => format!("\u{1F514} {n}"),
    }
}

/// How recently a member must have acted to count as active.
const ACTIVE_WINDOW_MS: i64 = 10 * 60 * 1000;

/// `"active"` when `member` has an event within the last ten minutes, else `"idle"`.
pub fn member_activity(events: &[rteams_core::Event], member: &str, now_ms: i64) -> &'static str {
    let last = events
        .iter()
        .rev()
        .find(|e| e.actor == member)
        .map(|e| e.ts);
    match last {
        Some(ts) if now_ms.saturating_sub(ts) <= ACTIVE_WINDOW_MS => "active",
        _ => "idle",
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `rate limited until HH:MM`, only while the reset time is still ahead of `now_ms`.
pub fn rate_limit_text(tr: &I18n, until: Option<i64>, now_ms: i64) -> Option<String> {
    until
        .filter(|&u| u > now_ms)
        .map(|u| tr.t_args("rteams.team.rateLimited", &[("time", &theme::hhmm(u))]))
}

/// `(done, total)` of the non-archived tasks owned by `name`; done = Done or Approved.
pub fn task_progress(v: &View, name: &str) -> (usize, usize) {
    let owned = v
        .state
        .tasks
        .iter()
        .filter(|t| !t.archived && t.owner.as_deref() == Some(name));
    owned.fold((0, 0), |(d, n), t| {
        let done = matches!(t.status, Status::Done | Status::Approved);
        (d + usize::from(done), n + 1)
    })
}

fn tasks_label(tr: &I18n, total: usize) -> String {
    tr.t_count("rteams.team.tasks", total as u64)
}

fn is_failed(s: &AgentStatus) -> bool {
    match s {
        AgentStatus::Failed(_) => true,
        AgentStatus::Exited { code } => *code != Some(0),
        AgentStatus::Running { .. } => false,
    }
}

fn any_running(v: &View) -> bool {
    v.agents
        .iter()
        .any(|a| matches!(a.status, AgentStatus::Running { .. }))
}

/// Banner to show above the team list: `(text, failed)`; `None` while agents are running
/// without failures.
pub fn banner_text(v: &View) -> Option<(String, bool)> {
    let expected = v.agent_members().len();
    let failed = v
        .agents
        .iter()
        .filter(|a| is_failed(&a.status))
        .count()
        .min(expected.max(1));
    if failed > 0 {
        return Some((
            v.tr.t_args(
                "team.detail.offline.partialMissing",
                &[
                    ("missing", &failed.to_string()),
                    ("expected", &expected.to_string()),
                ],
            ),
            true,
        ));
    }
    (!any_running(v)).then(|| (v.tr.t("team.detail.offline.offline"), false))
}

/// Team title, project path (or the free working-directory field when none is configured),
/// Stop / approvals / bell and a settings button; the Launch button lives in the offline banner.
/// Returns true when `Settings` was clicked.
pub fn header(
    ui: &mut egui::Ui,
    v: &View,
    workdir: &mut String,
    unread: usize,
    out: &mut Intents,
) -> bool {
    let running = any_running(v);
    let mut settings_clicked = false;
    let project = v
        .team()
        .and_then(|t| t.project_path.as_deref())
        .filter(|p| !p.trim().is_empty());
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            let name = v
                .team()
                .map_or(v.tr.t("rteams.team.noTeam"), |t| t.name.clone());
            ui.label(RichText::new(name).size(24.0).strong());
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                icons::icon(ui, Icon::Folder, 14.0, theme::MUTED);
                if let Some(p) = project {
                    ui.add(
                        egui::Label::new(RichText::new(p).size(12.0).color(theme::MUTED))
                            .truncate(),
                    )
                    .on_hover_text(v.tr.t("rteams.team.projectPathHint"));
                } else {
                    let w = (ui.available_width() - 160.0).clamp(80.0, 420.0);
                    ui.add(
                        egui::TextEdit::singleline(workdir)
                            .font(egui::TextStyle::Small)
                            .text_color(theme::TEXT_2)
                            .frame(egui::Frame::NONE)
                            .hint_text(v.tr.t("rteams.team.workdirHint"))
                            .desired_width(w),
                    );
                }
            });
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::pill_icon_button(
                ui,
                Some(Icon::Settings),
                &v.tr.t("common.layout.menu.settings"),
                false,
            )
            .clicked()
            {
                settings_clicked = true;
            }
            let label = if unread == 0 {
                String::new()
            } else if unread > 99 {
                "99+".to_string()
            } else {
                unread.to_string()
            };
            let bell = if label.is_empty() {
                theme::icon_button(ui, Icon::Bell, &v.tr.t("common.notifications.title"))
            } else {
                theme::pill_icon_button(ui, Some(Icon::Bell), &label, false)
                    .on_hover_text(v.tr.t("common.notifications.title"))
            };
            if bell.clicked() {
                out.push(Intent::OpenNotifications);
            }
            let pending = crate::approvals::Approvals::pending_count(v);
            if pending > 0
                && pill20(
                    ui,
                    Some(Icon::Shield),
                    &format!("{pending} {}", v.tr.t("team.tasks.status.pending")),
                    theme::WARNING,
                )
                .on_hover_text(v.tr.t("rteams.team.toolApprovals"))
                .clicked()
            {
                out.push(Intent::OpenApprovals);
            }
            if running
                && pill20(
                    ui,
                    Some(Icon::Square),
                    &v.tr.t("team.detail.tooltips.stopTeam"),
                    theme::ERROR,
                )
                .clicked()
            {
                out.push(Intent::StopAll);
            }
        });
    });
    ui.add_space(8.0);
    if let Some((text, failed)) = banner_text(v) {
        let c = if failed { theme::ERROR } else { theme::WARNING };
        egui::Frame::new()
            .fill(theme::tint(c, 0.07))
            .stroke(Stroke::new(1.0, theme::tint(c, 0.22)))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width().max(0.0));
                ui.horizontal(|ui| {
                    let (tile, _) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(tile, CornerRadius::same(8), theme::tint(c, 0.15));
                    ui.painter().text(
                        tile.center(),
                        egui::Align2::CENTER_CENTER,
                        GLYPH_POWER,
                        egui::FontId::proportional(15.0),
                        c,
                    );
                    ui.label(RichText::new(text).size(14.0));
                    if !running {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if launch_button(ui, v.tr).clicked() {
                                out.push(Intent::LaunchAll);
                            }
                        });
                    }
                });
            });
        ui.add_space(8.0);
    }
    settings_clicked
}

/// Compact outlined pill (20px tall, 10.5px text) with an optional leading icon.
fn pill20(ui: &mut egui::Ui, icon: Option<Icon>, text: &str, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(10.5),
        Color32::PLACEHOLDER,
    );
    let icon_w = if icon.is_some() { 14.0 } else { 0.0 };
    let size = Vec2::new(galley.size().x + 16.0 + icon_w, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.hovered() {
            theme::tint(color, 0.14)
        } else {
            theme::tint(color, 0.06)
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(255), fill);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(255),
            Stroke::new(1.0, theme::tint(color, 0.4)),
            egui::StrokeKind::Inside,
        );
        let mut x = rect.left() + 8.0;
        if let Some(i) = icon {
            icons::draw(
                ui.painter(),
                egui::Rect::from_center_size(
                    egui::pos2(x + 5.0, rect.center().y),
                    Vec2::splat(11.0),
                ),
                i,
                color,
            );
            x += icon_w;
        }
        ui.painter().galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn launch_button(ui: &mut egui::Ui, tr: &I18n) -> egui::Response {
    pill20(
        ui,
        Some(Icon::Play),
        &tr.t("team.detail.actions.launch"),
        theme::SUCCESS,
    )
}

/// Flat outlined status chip: 20px tall, no fill.
fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(10.0),
        Color32::PLACEHOLDER,
    );
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 16.0, 20.0),
        egui::Sense::hover(),
    );
    if ui.is_rect_visible(rect) {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(255),
            Stroke::new(1.0, theme::tint(color, 0.4)),
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(
            egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }
    resp
}

pub fn members(ui: &mut egui::Ui, v: &View, out: &mut Intents) {
    let (removed, members): (Vec<_>, Vec<_>) = v.members().iter().partition(|m| m.removed);
    let add_member = Cell::new(false);
    theme::section(
        ui,
        "team",
        GLYPH_USERS,
        &v.tr.t("common.layout.sections.team"),
        Some(members.len()),
        true,
        |ui| {
            if theme::ghost(ui, &format!("+ {}", v.tr.t("team.detail.actions.add")))
                .on_hover_text(v.tr.t("rteams.addMember.title"))
                .clicked()
            {
                add_member.set(true);
            }
        },
        |ui| {
            if members.is_empty() {
                ui.label(
                    RichText::new(v.tr.t("team.members.list.soloLeadOnly")).color(theme::MUTED),
                );
                return;
            }
            for m in &members {
                member_row(ui, v, &m.name, &m.role, out);
                theme::rule(ui);
            }
        },
    );
    if !removed.is_empty() {
        theme::section(
            ui,
            "removed",
            GLYPH_USERS,
            &v.tr.t("team.memberDraft.removed"),
            Some(removed.len()),
            false,
            |_| {},
            |ui| {
                for m in &removed {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&m.name).color(theme::TEXT_2));
                        ui.label(RichText::new(&m.role).size(12.0).color(theme::MUTED));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if theme::ghost(ui, &v.tr.t("team.kanban.trash.restore")).clicked() {
                                out.push(Intent::Run(GuiCmd::RemoveMember {
                                    name: m.name.clone(),
                                    removed: false,
                                }));
                            }
                        });
                    });
                    theme::rule(ui);
                }
            },
        );
    }
    if add_member.get() {
        out.push(Intent::AddMember);
    }
}

/// Row texts for one process: `(state, pid, port)`.
pub fn process_row_text(p: &rteams_core::ProcessEntry) -> (&'static str, String, Option<String>) {
    (
        if p.running { "running" } else { "stopped" },
        format!("pid {}", p.pid),
        p.port.map(|n| format!(":{n}")),
    )
}

/// &v.tr.t("rteams.team.processes") section: registered dev servers etc. Default closed.
pub fn processes(ui: &mut egui::Ui, v: &View) {
    let procs = &v.state.processes;
    let live = procs.iter().filter(|p| p.running).count();
    theme::section(
        ui,
        "processes",
        GLYPH_WRENCH,
        &v.tr.t("rteams.team.processes"),
        Some(live),
        false,
        |_| {},
        |ui| {
            if procs.is_empty() {
                ui.label(RichText::new(v.tr.t("rteams.team.noProcesses")).color(theme::MUTED));
                return;
            }
            for p in procs {
                let (state, pid, port) = process_row_text(p);
                ui.horizontal_wrapped(|ui| {
                    let c = if p.running {
                        theme::SUCCESS
                    } else {
                        theme::MUTED
                    };
                    ui.label(RichText::new("•").size(12.0).color(c))
                        .on_hover_text(loc_state(v.tr, state));
                    ui.label(RichText::new(&p.label).strong());
                    ui.label(RichText::new(&p.owner).color(v.color(&p.owner)));
                    ui.label(RichText::new(pid).size(11.0).color(theme::MUTED));
                    if let Some(port) = port {
                        ui.label(RichText::new(port).size(11.0).color(theme::TEXT_2));
                    }
                    if let Some(url) = &p.url {
                        ui.hyperlink_to(url, url);
                    }
                });
                if let Some(cmd) = &p.command {
                    ui.add(
                        egui::Label::new(
                            RichText::new(cmd)
                                .monospace()
                                .size(11.0)
                                .color(theme::MUTED),
                        )
                        .truncate(),
                    );
                }
                theme::rule(ui);
            }
        },
    );
}

/// Runtime of an agent member: the launcher's, else the roster's (default `claude`).
fn runtime_label(v: &View, name: &str) -> Option<String> {
    let live = v
        .agent(name)
        .map(|a| a.runtime.clone())
        .filter(|r| !r.is_empty());
    live.or_else(|| {
        v.members()
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.runtime.clone().unwrap_or_else(|| "claude".to_string()))
    })
}

/// Round 26px icon-only button for the member row (muted, soft hover).
fn row_icon(ui: &mut egui::Ui, icon: Icon, color: Color32, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .circle_filled(rect.center(), 13.0, Color32::from_rgb(0x25, 0x27, 0x33));
    }
    icons::draw(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), Vec2::splat(14.0)),
        icon,
        if resp.hovered() { theme::TEXT } else { color },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn member_row(ui: &mut egui::Ui, v: &View, name: &str, role: &str, out: &mut Intents) {
    let is_human = v.actor == Some(name);
    let agent = v.agent(name);
    let running = agent.is_some_and(|a| matches!(a.status, AgentStatus::Running { .. }));
    let color = v.color(name);
    let presence = if running {
        theme::Presence::Online
    } else {
        theme::Presence::Offline
    };
    ui.horizontal(|ui| {
        ui.set_min_height(65.0);
        ui.spacing_mut().item_spacing.x = 8.0;
        let av = theme::avatar(ui, name, color, 44.0, Some(presence));
        let mut open = av.clicked();
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.add_space(13.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            let nm = ui.add(
                egui::Label::new(RichText::new(name).size(16.0).strong())
                    .truncate()
                    .sense(egui::Sense::click()),
            );
            open |= nm.clicked();
            let sub = if is_human {
                format!("{} \u{00B7} {role}", v.tr.t("team.messages.chats.you"))
            } else {
                let rt = capitalize(&runtime_label(v, name).unwrap_or_else(|| "claude".into()));
                let model = v
                    .members()
                    .iter()
                    .find(|m| m.name == name)
                    .and_then(|m| m.model.clone())
                    .filter(|m| !m.trim().is_empty());
                match model {
                    Some(m) => format!("{rt} \u{00B7} {m} \u{00B7} {role}"),
                    None => format!("{rt} \u{00B7} {role}"),
                }
            };
            ui.add(egui::Label::new(RichText::new(sub).size(12.0).color(theme::MUTED)).truncate());
        });
        if open {
            out.push(Intent::OpenMember(name.to_string()));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if !is_human {
                if running {
                    if row_icon(
                        ui,
                        Icon::Square,
                        theme::ERROR,
                        &v.tr.t("team.detail.actions.stop"),
                    )
                    .clicked()
                    {
                        out.push(Intent::Stop(name.to_string()));
                    }
                    if row_icon(
                        ui,
                        Icon::Refresh,
                        theme::TEXT_2,
                        &v.tr.t("team.members.detail.restart"),
                    )
                    .clicked()
                    {
                        out.push(Intent::Restart(name.to_string()));
                    }
                } else if row_icon(
                    ui,
                    Icon::Play,
                    theme::SUCCESS,
                    &v.tr.t("team.detail.actions.launch"),
                )
                .clicked()
                {
                    out.push(Intent::Launch(name.to_string()));
                }
            }
            if row_icon(
                ui,
                Icon::Plus,
                theme::TEXT_2,
                &v.tr.t("team.members.actions.assignTask"),
            )
            .clicked()
            {
                out.push(Intent::NewTask {
                    owner: Some(name.to_string()),
                });
            }
            if row_icon(
                ui,
                Icon::MessageSquare,
                theme::TEXT_2,
                &v.tr.t("team.members.actions.sendMessage"),
            )
            .clicked()
            {
                out.push(Intent::OpenChat(Some(name.to_string())));
            }
            ui.add_space(4.0);
            let (done, total) = task_progress(v, name);
            let r = chip(ui, &tasks_label(v.tr, total), theme::TEXT_2);
            if total > 0 {
                let rect = r.rect;
                let w = rect.width() * done as f32 / total as f32;
                let line = egui::Rect::from_min_size(
                    egui::pos2(rect.left(), rect.bottom() + 2.0),
                    Vec2::new(w.max(0.0), 2.0),
                );
                ui.painter().rect_filled(line, 1.0, theme::SUCCESS);
            }
            let (text, c) = if is_human {
                ("lead".to_string(), theme::ACCENT)
            } else {
                match agent {
                    Some(a) => status_chip(Some(&a.status)),
                    None => {
                        let state = member_activity(v.data.events(), name, now_ms());
                        let c = if state == "active" {
                            theme::SUCCESS
                        } else {
                            theme::MUTED
                        };
                        (state.to_string(), c)
                    }
                }
            };
            let r = chip(ui, &loc_state(v.tr, &text), c);
            if let Some(AgentStatus::Failed(why)) = agent.map(|a| &a.status) {
                let why = agent
                    .and_then(|a| a.last_error.as_deref())
                    .unwrap_or(why.as_str());
                r.on_hover_text(why);
            } else if let Some(err) = agent.and_then(|a| a.last_error.as_deref()) {
                if text != "running" && text != "finished" {
                    r.on_hover_text(err);
                }
            }
            if let Some(t) =
                rate_limit_text(v.tr, agent.and_then(|a| a.rate_limited_until), now_ms())
            {
                chip(ui, &t, theme::WARNING);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launcher::AgentView;
    use crate::view::testing::{fixture, render};

    fn agent(member: &str, status: AgentStatus) -> AgentView {
        AgentView {
            member: member.into(),
            status,
            last_lines: vec![],
            rate_limited_until: None,
            last_error: None,
            runtime: String::new(),
        }
    }

    #[test]
    fn chip_text_for_every_status() {
        assert_eq!(status_chip(None).0, "idle");
        assert_eq!(
            status_chip(Some(&AgentStatus::Running { pid: 1 })).0,
            "running"
        );
        assert_eq!(
            status_chip(Some(&AgentStatus::Exited { code: Some(0) })).0,
            "finished"
        );
        assert_eq!(
            status_chip(Some(&AgentStatus::Exited { code: Some(2) })).0,
            "exit 2"
        );
        assert_eq!(
            status_chip(Some(&AgentStatus::Exited { code: None })).0,
            "exit ?"
        );
        assert_eq!(
            status_chip(Some(&AgentStatus::Failed("x".into()))).0,
            "failed"
        );
    }

    #[test]
    fn progress_and_banner_and_render() {
        let (home, data) = fixture("team");
        let agents = vec![
            agent("ada", AgentStatus::Running { pid: 1 }),
            agent("bob", AgentStatus::Exited { code: Some(1) }),
        ];
        let v = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &agents,
            tr: crate::view::testing::tr(),
        };
        assert_eq!(task_progress(&v, "ada").1, 2);
        assert_eq!(task_progress(&v, "nobody"), (0, 0));
        assert_eq!(tasks_label(crate::view::testing::tr(), 1), "1 task");
        assert!(banner_text(&v).unwrap().0.contains("1/2"));
        let idle = View {
            agents: &[],
            ..View {
                state: data.state(),
                data: &data,
                actor: Some("boss"),
                agents: &[],
                tr: crate::view::testing::tr(),
            }
        };
        assert_eq!(banner_text(&idle).unwrap().0, "Team is offline");

        for ag in [&agents[..], &[]] {
            let v = View {
                state: data.state(),
                data: &data,
                actor: Some("boss"),
                agents: ag,
                tr: crate::view::testing::tr(),
            };
            let mut wd = "/tmp".to_string();
            render(|ui| {
                let mut out = Intents::default();
                let _ = header(ui, &v, &mut wd, 3, &mut out);
                members(ui, &v, &mut out);
                processes(ui, &v);
            });
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn removed_member_and_project_path_render() {
        let (home, _) = fixture("teamremoved");
        let mut c = crate::actions::Commander::new(home.clone(), "t".into());
        for cmd in [
            GuiCmd::RemoveMember {
                name: "bob".into(),
                removed: true,
            },
            GuiCmd::ConfigureTeam {
                project_path: Some("/work/proj".into()),
                lead_prompt: None,
            },
        ] {
            c.run("boss", &cmd).unwrap();
        }
        let mut data = crate::data::Data::open(home.clone(), "t").unwrap();
        data.refresh().unwrap();
        let agents = vec![agent("ada", AgentStatus::Running { pid: 1 })];
        let v = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &agents,
            tr: crate::view::testing::tr(),
        };
        assert!(v.members().iter().any(|m| m.name == "bob" && m.removed));
        let mut out = Intents::default();
        let mut wd = String::new();
        render(|ui| {
            let _ = header(ui, &v, &mut wd, 3, &mut out);
            members(ui, &v, &mut out);
        });
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn bell_activity_and_rate_limit_text() {
        assert_eq!(bell_label(0), "\u{1F514}");
        assert_eq!(bell_label(5), "\u{1F514} 5");
        assert_eq!(bell_label(500), "\u{1F514} 99+");
        let ev = |ts: i64, actor: &str| rteams_core::Event {
            seq: 1,
            ts,
            actor: actor.into(),
            kind: rteams_core::EventKind::MessageSent {
                to: None,
                from_team: None,
                text: "x".into(),
                mode: None,
            },
        };
        let evs = [ev(1_000, "ada"), ev(5_000_000, "bob")];
        assert_eq!(member_activity(&evs, "ada", 2_000), "active");
        assert_eq!(member_activity(&evs, "ada", 5_000_000), "idle");
        assert_eq!(member_activity(&evs, "bob", 5_100_000), "active");
        assert_eq!(member_activity(&evs, "zed", 0), "idle");
        assert_eq!(rate_limit_text(crate::view::testing::tr(), None, 0), None);
        assert_eq!(
            rate_limit_text(crate::view::testing::tr(), Some(5), 10),
            None
        );
        assert_eq!(
            rate_limit_text(crate::view::testing::tr(), Some(3_600_000 + 5 * 60_000), 0).unwrap(),
            "rate limited until 01:05"
        );
    }

    #[test]
    fn chips_render_for_limited_failed_agents() {
        let (home, data) = fixture("teamchips");
        let mut a = agent("ada", AgentStatus::Failed("spawn failed".into()));
        a.rate_limited_until = Some(i64::MAX / 4);
        a.last_error = Some("boom".into());
        a.runtime = "codex".into();
        let agents = vec![a];
        let v = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &agents,
            tr: crate::view::testing::tr(),
        };
        assert_eq!(runtime_label(&v, "ada").as_deref(), Some("codex"));
        assert_eq!(runtime_label(&v, "bob").as_deref(), Some("claude"));
        let mut out = Intents::default();
        render(|ui| members(ui, &v, &mut out));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn process_text() {
        let p = rteams_core::ProcessEntry {
            pid: 42,
            label: "web".into(),
            owner: "ada".into(),
            port: Some(3000),
            url: None,
            command: None,
            running: true,
        };
        assert_eq!(
            process_row_text(&p),
            ("running", "pid 42".to_string(), Some(":3000".to_string()))
        );
    }
}
