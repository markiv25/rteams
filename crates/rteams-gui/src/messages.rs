//! Messages: the left chat list (Group chat + one direct chat per member) and the conversation
//! pane with its composer. Pure helpers (membership, unread, preview, mode string) are separate
//! functions so they can be unit tested.

use std::collections::HashMap;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Rect, RichText, Sense, Shape, Stroke, Vec2,
};
use rteams_core::{Attachment, EventKind, Message, Reaction, Scope};

use crate::actions::GuiCmd;
use crate::icons::{self, Icon};
use crate::launcher::AgentStatus;
use crate::links;
use crate::theme;
use crate::view::{Intent, Intents, View};

/// A chat: `None` = Group chat (broadcasts), `Some(member)` = direct chat with that member.
type Chat = Option<String>;

/// How the message is framed for the recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Do,
    Ask,
    Delegate,
}

impl Mode {
    const ALL: [Mode; 3] = [Mode::Do, Mode::Ask, Mode::Delegate];

    fn label_key(self) -> &'static str {
        match self {
            Mode::Do => "rteams.messages.mode.do",
            Mode::Ask => "rteams.messages.mode.ask",
            Mode::Delegate => "rteams.messages.mode.delegate",
        }
    }
}

/// Wire value of a mode, as carried by `GuiCmd::Message::mode`.
pub fn mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Do => "do",
        Mode::Ask => "ask",
        Mode::Delegate => "delegate",
    }
}

/// Persisted read cursors: `{"read": {"group": 12, "dm:ada": 7}}`.
fn cursor_key(chat: Option<&str>) -> String {
    match chat {
        None => "group".to_string(),
        Some(INBOX) => "inbox".to_string(),
        Some(g) if g.starts_with(GROUP_PFX) => format!("grp:{}", &g[GROUP_PFX.len()..]),
        Some(m) => format!("dm:{m}"),
    }
}

/// Prefix of the pseudo chat name that stands for a user-created group chat in `Chat`.
const GROUP_PFX: &str = "\u{2}group:";

/// The `Chat` value of group chat `id`.
pub fn group_key(id: &str) -> Chat {
    Some(format!("{GROUP_PFX}{id}"))
}

/// The group chat id behind a `Chat` value, if it is one.
pub fn group_id(chat: Option<&str>) -> Option<&str> {
    chat?.strip_prefix(GROUP_PFX)
}

/// Id for a new group chat named `name`: a slug, made unique among `existing`.
pub fn new_group_id(name: &str, existing: &[String]) -> String {
    let mut slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    slug = slug
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        slug = "group".into();
    }
    let mut id = slug.clone();
    let mut n = 2;
    while existing.contains(&id) {
        id = format!("{slug}-{n}");
        n += 1;
    }
    id
}

/// Pseudo chat name under which the My-inbox read cursor is kept in `seen`.
const INBOX: &str = "\u{1}inbox";

fn load_cursor_file(path: &std::path::Path) -> HashMap<Chat, u64> {
    let mut out = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return out;
    };
    if let Some(map) = v.get("read").and_then(|r| r.as_object()) {
        for (k, n) in map {
            let Some(n) = n.as_u64() else { continue };
            if k == "group" {
                out.insert(None, n);
            } else if k == "inbox" {
                out.insert(Some(INBOX.to_string()), n);
            } else if let Some(g) = k.strip_prefix("grp:") {
                out.insert(group_key(g), n);
            } else if let Some(m) = k.strip_prefix("dm:") {
                out.insert(Some(m.to_string()), n);
            }
        }
    }
    out
}

/// Merge `seen` into the shared `gui-state.json`, keeping its other keys.
fn save_cursor_file(path: &std::path::Path, seen: &HashMap<Chat, u64>) {
    let mut root = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    let read: serde_json::Map<String, serde_json::Value> = seen
        .iter()
        .map(|(c, n)| (cursor_key(c.as_deref()), serde_json::json!(n)))
        .collect();
    root["read"] = serde_json::Value::Object(read);
    if let Ok(text) = serde_json::to_string_pretty(&root) {
        let _ = std::fs::write(path, text);
    }
}

/// Does `m` belong to `chat`? Group chat = broadcasts; a direct chat = direct messages sent
/// by or to that member. Task comments belong to no chat.
pub fn in_chat(m: &Message, chat: Option<&str>) -> bool {
    match (&m.scope, chat) {
        (Scope::Broadcast, None) => true,
        (Scope::Group { chat: c }, Some(sel)) => group_id(Some(sel)) == Some(c.as_str()),
        // A cross-team sender is not a member here: match on the recipient only, and show it in
        // the group chat too so the human never misses it.
        (Scope::Direct { .. }, None) => m.from_team.is_some(),
        (Scope::Direct { to }, Some(member)) if m.from_team.is_some() => to == member,
        (Scope::Direct { to }, Some(member)) => m.from == member || to == member,
        _ => false,
    }
}

/// Messages of `chat` in log order.
pub fn chat_messages<'a>(msgs: &'a [Message], chat: Option<&str>) -> Vec<&'a Message> {
    msgs.iter().filter(|m| in_chat(m, chat)).collect()
}

/// Messages in `chat` newer than `last_seen` that were not written by `actor`.
pub fn unread_count(
    msgs: &[Message],
    chat: Option<&str>,
    actor: Option<&str>,
    last_seen: u64,
) -> usize {
    msgs.iter()
        .filter(|m| in_chat(m, chat) && m.seq > last_seen && Some(m.from.as_str()) != actor)
        .count()
}

/// Emoji offered by the `+` reaction button.
pub const QUICK_EMOJI: [&str; 6] = ["👍", "✅", "👀", "🎉", "❤", "🔥"];

/// One reaction chip: an emoji, how many members used it, and whether `me` is one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReactionCount {
    pub emoji: String,
    pub count: usize,
    pub mine: bool,
    pub who: Vec<String>,
}

/// Group reactions by emoji, in order of first use.
pub fn reaction_counts(reactions: &[Reaction], me: Option<&str>) -> Vec<ReactionCount> {
    let mut out: Vec<ReactionCount> = Vec::new();
    for r in reactions {
        let i = match out.iter().position(|c| c.emoji == r.emoji) {
            Some(i) => i,
            None => {
                out.push(ReactionCount {
                    emoji: r.emoji.clone(),
                    count: 0,
                    mine: false,
                    who: Vec::new(),
                });
                out.len() - 1
            }
        };
        let c = &mut out[i];
        c.count += 1;
        c.mine |= Some(r.from.as_str()) == me;
        c.who.push(r.from.clone());
    }
    out
}

/// Split a message into a bold summary (its first line) and the rest, when it has several
/// non-empty lines.
pub fn split_summary(text: &str) -> Option<(&str, &str)> {
    let t = text.trim_start_matches('\n');
    let (first, rest) = t.split_once('\n')?;
    (!first.trim().is_empty() && !rest.trim().is_empty())
        .then(|| (first.trim_end(), rest.trim_start_matches('\n')))
}

/// Members of other teams under `home` that can be messaged: `(team dir, member names)`.
fn scan_other_teams(home: &std::path::Path, current: &str) -> Vec<(String, Vec<String>)> {
    crate::teams::scan(home)
        .into_iter()
        .filter(|c| c.dir != current)
        .filter_map(|c| {
            let store = rteams_store::FileStore::open_read_only(home, &c.dir).ok()?;
            let members: Vec<String> = store
                .loaded_state()
                .team
                .as_ref()?
                .members
                .iter()
                .filter(|m| !m.removed)
                .map(|m| m.name.clone())
                .collect();
            (!members.is_empty()).then_some((c.dir, members))
        })
        .collect()
}

/// First line of `text`, whitespace collapsed, cut to `max` chars with an ellipsis.
pub fn preview(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let mut s: String = line.chars().take(max.saturating_sub(1)).collect();
    s.push('\u{2026}');
    s
}

// ---- My inbox ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxKind {
    Direct,
    Comment,
    Mention,
    Review,
    Question,
}

impl InboxKind {
    fn label(self) -> &'static str {
        match self {
            InboxKind::Direct => "Message",
            InboxKind::Comment => "Comment",
            InboxKind::Mention => "Mention",
            InboxKind::Review => "Review",
            InboxKind::Question => "Question",
        }
    }

    fn color(self) -> Color32 {
        match self {
            InboxKind::Direct => theme::INFO,
            InboxKind::Comment => theme::ACCENT,
            InboxKind::Mention => theme::WARNING,
            InboxKind::Review => theme::COL_REVIEW,
            InboxKind::Question => theme::ERROR,
        }
    }
}

/// One thing addressed to the human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    pub seq: u64,
    pub ts: i64,
    pub from: String,
    pub text: String,
    pub task: Option<String>,
    pub kind: InboxKind,
}

fn mentions(text: &str, who: &str) -> bool {
    links::tokenize(text)
        .iter()
        .any(|t| matches!(t, links::Token::Mention(n) if n == who))
}

/// Everything addressed to `actor`: direct messages, comments on tasks they own or created,
/// @mentions, review requests and pending clarification questions. Oldest first.
pub fn inbox_from(
    actor: &str,
    msgs: &[Message],
    tasks: &[rteams_core::Task],
    events: &[rteams_core::Event],
) -> Vec<InboxItem> {
    let mut creators: HashMap<&str, &str> = HashMap::new();
    for e in events {
        if let EventKind::TaskCreated { task, .. } = &e.kind {
            creators.insert(task.as_str(), e.actor.as_str());
        }
    }
    let task = |id: &str| tasks.iter().find(|t| t.id == id);
    let mut out: Vec<InboxItem> = Vec::new();
    for m in msgs {
        if m.from == actor && m.from_team.is_none() {
            continue;
        }
        let mentioned = mentions(&m.text, actor);
        let (kind, tid) = match &m.scope {
            Scope::Direct { to } if to == actor => (Some(InboxKind::Direct), None),
            Scope::Task { task: id } => {
                let mine = task(id).is_some_and(|t| t.owner.as_deref() == Some(actor))
                    || creators.get(id.as_str()) == Some(&actor);
                let k = if mine {
                    Some(InboxKind::Comment)
                } else if mentioned {
                    Some(InboxKind::Mention)
                } else {
                    None
                };
                (k, Some(id.clone()))
            }
            _ => (mentioned.then_some(InboxKind::Mention), None),
        };
        if let Some(kind) = kind {
            out.push(InboxItem {
                seq: m.seq,
                ts: m.ts,
                from: m.from.clone(),
                text: m.text.clone(),
                task: tid,
                kind,
            });
        }
    }
    let mut questions: HashMap<String, InboxItem> = HashMap::new();
    for e in events {
        if e.actor == actor {
            continue;
        }
        match &e.kind {
            EventKind::ReviewRequested { task: id, reviewer } if reviewer == actor => {
                let subject = task(id).map_or("", |t| t.subject.as_str());
                out.push(InboxItem {
                    seq: e.seq,
                    ts: e.ts,
                    from: e.actor.clone(),
                    text: format!("Review requested: {subject}"),
                    task: Some(id.clone()),
                    kind: InboxKind::Review,
                });
            }
            EventKind::TaskClarificationSet {
                task: id,
                question: Some(q),
            } if task(id).is_some_and(|t| t.clarification.is_some()) => {
                questions.insert(
                    id.clone(),
                    InboxItem {
                        seq: e.seq,
                        ts: e.ts,
                        from: e.actor.clone(),
                        text: q.clone(),
                        task: Some(id.clone()),
                        kind: InboxKind::Question,
                    },
                );
            }
            EventKind::TaskClarificationSet {
                task: id,
                question: None,
            } => {
                questions.remove(id);
            }
            _ => {}
        }
    }
    out.extend(questions.into_values());
    out.sort_by_key(|i| i.seq);
    out
}

pub fn inbox_items(v: &View) -> Vec<InboxItem> {
    v.actor.map_or_else(Vec::new, |a| {
        inbox_from(a, &v.state.messages, &v.state.tasks, v.data.events())
    })
}

/// Leading `> ` lines of a message as a quote, plus the rest.
pub fn split_quote(text: &str) -> (Option<String>, &str) {
    let mut quote: Vec<&str> = Vec::new();
    let mut rest = text;
    while let Some(line_end) = rest.find('\n').or(if rest.starts_with('>') {
        Some(rest.len())
    } else {
        None
    }) {
        let line = &rest[..line_end];
        let Some(q) = line.strip_prefix("> ") else {
            break;
        };
        quote.push(q);
        rest = rest[line_end..].trim_start_matches('\n');
        if rest.is_empty() {
            break;
        }
    }
    if quote.is_empty() {
        (None, text)
    } else {
        (Some(quote.join("\n")), rest)
    }
}

// ---- small drawing helpers ----

#[derive(Clone, Copy)]
enum Glyph {
    Reply,
    Copy,
    More,
    Inbox,
}

fn glyph(p: &egui::Painter, r: Rect, g: Glyph, col: Color32, bg: Color32) {
    let c = r.center();
    let s = r.width() / 16.0;
    let st = Stroke::new(1.4, col);
    let pt = |x: f32, y: f32| c + Vec2::new(x, y) * s;
    match g {
        Glyph::Reply => {
            p.add(Shape::line(
                vec![pt(-3.0, -5.0), pt(-7.0, -1.0), pt(-3.0, 3.0)],
                st,
            ));
            p.add(Shape::line(
                vec![pt(-6.5, -1.0), pt(1.0, -1.0), pt(6.0, 2.0), pt(6.0, 6.0)],
                st,
            ));
        }
        Glyph::Copy => {
            let rr = CornerRadius::same(2);
            p.rect_stroke(
                Rect::from_min_max(pt(-2.0, -6.0), pt(6.0, 2.0)),
                rr,
                st,
                egui::StrokeKind::Middle,
            );
            let front = Rect::from_min_max(pt(-6.0, -2.0), pt(2.0, 6.0));
            p.rect_filled(front, rr, bg);
            p.rect_stroke(front, rr, st, egui::StrokeKind::Middle);
        }
        Glyph::More => {
            for x in [-5.0, 0.0, 5.0] {
                p.circle_filled(pt(x, 0.0), 1.3 * s, col);
            }
        }
        Glyph::Inbox => {
            p.add(Shape::line(
                vec![pt(-7.0, 1.0), pt(-4.0, -5.0), pt(4.0, -5.0), pt(7.0, 1.0)],
                st,
            ));
            p.add(Shape::line(
                vec![
                    pt(-7.0, 1.0),
                    pt(-7.0, 6.0),
                    pt(7.0, 6.0),
                    pt(7.0, 1.0),
                    pt(3.0, 1.0),
                    pt(2.0, 3.0),
                    pt(-2.0, 3.0),
                    pt(-3.0, 1.0),
                    pt(-7.0, 1.0),
                ],
                st,
            ));
        }
    }
}

fn tool_btn(ui: &mut egui::Ui, g: Glyph, tip: &str, bg: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(6),
            Color32::from_rgb(0x2c, 0x2e, 0x3b),
        );
    }
    let col = if resp.hovered() {
        theme::TEXT
    } else {
        theme::TEXT_2
    };
    let hover_bg = if resp.hovered() {
        Color32::from_rgb(0x2c, 0x2e, 0x3b)
    } else {
        bg
    };
    glyph(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(15.0)),
        g,
        col,
        hover_bg,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
}

fn paint_tiny_avatar(p: &egui::Painter, c: egui::Pos2, r: f32, name: &str, color: Color32) {
    p.circle_filled(c, r, theme::tint(color, 0.22));
    p.circle_stroke(c, r - 0.5, Stroke::new(1.2, color));
    p.text(
        c,
        egui::Align2::CENTER_CENTER,
        crate::names::icon_for(p.ctx(), name),
        egui::FontId::proportional(r * 1.1),
        color,
    );
}

/// `text` cut with an ellipsis so it is at most `max_w` wide.
fn fit(p: &egui::Painter, text: &str, font: &egui::FontId, max_w: f32) -> String {
    let w = |s: &str| {
        p.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE)
            .size()
            .x
    };
    if w(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().collect::<String>() + "\u{2026}";
        if w(&s) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "\u{2026}"
}

fn online(v: &View, member: &str) -> bool {
    matches!(
        v.agent(member).map(|a| &a.status),
        Some(AgentStatus::Running { .. })
    )
}

/// What a sidebar row shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RowKind {
    Chat(Chat),
    Inbox,
}

#[derive(Default)]
pub struct Messages {
    selected: Chat,
    /// "My inbox" is the open chat.
    inbox: bool,
    full: bool,
    /// Per-chat seq of the newest message the user has seen (the inbox under `INBOX`).
    seen: HashMap<Chat, u64>,
    /// Recipient picked in the composer; `None` = follow the selected chat.
    to: Option<Chat>,
    mode: Mode,
    text: String,
    /// `<home>/<team>/gui-state.json`, once the shell has called `load_cursors`.
    cursor_path: Option<std::path::PathBuf>,
    /// Teams home and this team's dir name, for the composer's other-teams section.
    home: Option<(std::path::PathBuf, String)>,
    other_teams: Vec<(String, Vec<String>)>,
    other_scanned: Option<std::time::Instant>,
    /// Cross-team recipient `(team, member)`; overrides `to` while set.
    cross: Option<(String, String)>,
    /// Inbox item the composer is replying to (by seq).
    inbox_sel: Option<u64>,
    /// Send the reply as a comment on the selected item's task.
    reply_comment: bool,
    /// Quoted message `(sender, text)` for the next send.
    quote: Option<(String, String)>,
    focus_composer: bool,
    open_logs: bool,
    composer: crate::ui::composer::Composer,
    /// Open "New / Edit group chat" dialog.
    group_draft: Option<GroupDraft>,
    /// Group chat id awaiting delete confirmation.
    group_delete: Option<String>,
}

/// State of the group chat dialog (`id: None` = creating).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupDraft {
    pub id: Option<String>,
    pub name: String,
    pub members: std::collections::BTreeSet<String>,
}

impl GroupDraft {
    /// Why the draft cannot be saved yet.
    pub fn problem(&self) -> Option<&'static str> {
        if self.name.trim().is_empty() {
            Some("rteams.group.namePlaceholder")
        } else if self.members.is_empty() {
            Some("rteams.group.noMembers")
        } else {
            None
        }
    }

    /// The command that saves the draft; `existing` = ids already taken.
    pub fn command(&self, existing: &[String]) -> Option<GuiCmd> {
        self.problem().is_none().then(|| {
            let members: Vec<String> = self.members.iter().cloned().collect();
            match &self.id {
                Some(id) => GuiCmd::GroupChatUpdate {
                    id: id.clone(),
                    name: Some(self.name.trim().to_string()),
                    members: Some(members),
                },
                None => GuiCmd::GroupChatCreate {
                    id: new_group_id(&self.name, existing),
                    name: self.name.trim().to_string(),
                    members,
                },
            }
        })
    }
}

fn group_name(v: &View, id: &str) -> String {
    v.team()
        .and_then(|t| t.group_chats.iter().find(|g| g.id == id))
        .map_or_else(|| id.to_string(), |g| g.name.clone())
}

fn max_seq(msgs: &[Message], chat: Option<&str>) -> u64 {
    msgs.iter()
        .filter(|m| in_chat(m, chat))
        .map(|m| m.seq)
        .max()
        .unwrap_or(0)
}

/// A request raised by a bubble this frame.
#[derive(Default)]
struct BubbleAct {
    reply: Option<(String, String)>,
    /// `(home, team dir)` for resolving attachment paths.
    home: Option<(std::path::PathBuf, String)>,
}

impl Messages {
    #[cfg(test)]
    pub fn open_inbox_for_tests(&mut self) {
        self.open_inbox();
        self.full = true;
    }

    #[cfg(test)]
    pub fn toggle_full_screen_for_tests(&mut self) {
        self.full = !self.full;
    }

    pub fn full_screen(&self) -> bool {
        self.full
    }

    pub fn clear_composer(&mut self) {
        self.text.clear();
        self.quote = None;
    }

    /// True once after the user clicked the `> Logs` strip.
    pub fn set_full_screen(&mut self, on: bool) {
        self.full = on;
    }

    pub fn take_open_logs(&mut self) -> bool {
        std::mem::take(&mut self.open_logs)
    }

    /// Show "My inbox" in the chat pane.
    pub fn open_inbox(&mut self) {
        self.inbox = true;
        self.to = None;
        self.cross = None;
        self.inbox_sel = None;
        self.reply_comment = false;
    }

    /// Where the teams live and which one is open; enables the cross-team recipients.
    pub fn set_home(&mut self, home: std::path::PathBuf, team: String) {
        self.composer.attach_dir = Some(home.join(&team).join("attachments"));
        self.home = Some((home, team));
        self.other_scanned = None;
        self.other_teams.clear();
        self.cross = None;
    }

    fn refresh_other_teams(&mut self) {
        let stale = self
            .other_scanned
            .is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(3));
        if let (true, Some((home, team))) = (stale, &self.home) {
            self.other_teams = scan_other_teams(home, team);
            self.other_scanned = Some(std::time::Instant::now());
        }
    }

    pub fn open_chat(&mut self, chat: Option<String>) {
        if self.selected != chat || self.inbox {
            self.to = None;
            self.cross = None;
        }
        self.inbox = false;
        self.selected = chat;
    }

    /// Load the persisted read cursors and keep saving them to `path` (best effort).
    pub fn load_cursors(&mut self, path: std::path::PathBuf) {
        for (chat, n) in load_cursor_file(&path) {
            let e = self.seen.entry(chat).or_insert(0);
            *e = (*e).max(n);
        }
        self.cursor_path = Some(path);
    }

    fn save_cursors(&self) {
        if let Some(p) = &self.cursor_path {
            save_cursor_file(p, &self.seen);
        }
    }

    fn mark_read(&mut self, v: &View, chat: Option<&str>) {
        let s = max_seq(&v.state.messages, chat);
        let e = self.seen.entry(chat.map(str::to_string)).or_insert(0);
        if s > *e {
            *e = s;
            self.save_cursors();
        }
    }

    fn mark_inbox_read(&mut self, v: &View) {
        let s = inbox_items(v).last().map_or(0, |i| i.seq);
        let e = self.seen.entry(Some(INBOX.to_string())).or_insert(0);
        if s > *e {
            *e = s;
            self.save_cursors();
        }
    }

    fn mark_all_read(&mut self, v: &View) {
        self.mark_read(v, None);
        for m in v.agent_members().into_iter().filter(|m| !m.removed) {
            self.mark_read(v, Some(&m.name));
        }
        self.mark_inbox_read(v);
        for g in v
            .team()
            .map(|t| t.group_chats.as_slice())
            .unwrap_or_default()
        {
            self.mark_read(v, group_key(&g.id).as_deref());
        }
    }

    fn unread(&self, v: &View, chat: Option<&str>) -> usize {
        let seen = self
            .seen
            .get(&chat.map(str::to_string))
            .copied()
            .unwrap_or(0);
        unread_count(&v.state.messages, chat, v.actor, seen)
    }

    fn inbox_unread(&self, v: &View) -> usize {
        let seen = self
            .seen
            .get(&Some(INBOX.to_string()))
            .copied()
            .unwrap_or(0);
        inbox_items(v).iter().filter(|i| i.seq > seen).count()
    }

    fn total_unread(&self, v: &View) -> usize {
        let mut n = self.unread(v, None) + self.inbox_unread(v);
        for g in v
            .team()
            .map(|t| t.group_chats.as_slice())
            .unwrap_or_default()
        {
            n += self.unread(v, group_key(&g.id).as_deref());
        }
        for m in v.agent_members().into_iter().filter(|m| !m.removed) {
            n += self.unread(v, Some(&m.name));
        }
        n
    }

    fn chat_title(&self, v: &View) -> String {
        if self.inbox {
            return "My inbox".to_string();
        }
        if let Some(id) = group_id(self.selected.as_deref()) {
            return group_name(v, id);
        }
        self.selected
            .clone()
            .unwrap_or_else(|| v.tr.t("team.messages.chats.teamFeed"))
    }

    // ---- sidebar ----

    pub fn sidebar(&mut self, ui: &mut egui::Ui, v: &View, out: &mut Intents) {
        self.group_dialogs(ui.ctx(), v, out);
        ui.horizontal(|ui| {
            ui.set_min_height(30.0);
            ui.label(
                RichText::new(v.tr.t("team.messages.title"))
                    .size(16.0)
                    .strong(),
            );
            let n = self.total_unread(v);
            if n > 0 {
                theme::count_badge(ui, n, true);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let more = theme::icon_button(ui, Icon::MoreHorizontal, "");
                egui::Popup::menu(&more).width(200.0).show(|ui| {
                    if theme::menu_item(
                        ui,
                        Some(Icon::Check),
                        &v.tr.t("team.messages.actions.markAllRead"),
                    )
                    .clicked()
                    {
                        self.mark_all_read(v);
                        ui.close();
                    }
                });
                theme::switch(ui, &mut self.full);
                ui.label(
                    RichText::new(v.tr.t("team.messages.fullScreen.label"))
                        .size(12.0)
                        .color(theme::TEXT_2),
                );
            });
        });
        ui.add_space(6.0);

        egui::Panel::bottom(ui.make_persistent_id("logs-strip"))
            .exact_size(38.0)
            .show_separator_line(false)
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                theme::rule(ui);
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(8),
                        theme::tint(Color32::WHITE, 0.04),
                    );
                }
                icons::draw(
                    ui.painter(),
                    Rect::from_center_size(
                        egui::pos2(rect.left() + 14.0, rect.center().y),
                        Vec2::splat(14.0),
                    ),
                    Icon::ChevronRight,
                    theme::MUTED,
                );
                ui.painter().text(
                    egui::pos2(rect.left() + 30.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Logs",
                    egui::FontId::proportional(14.0),
                    theme::TEXT_2,
                );
                icons::draw(
                    ui.painter(),
                    Rect::from_center_size(
                        egui::pos2(rect.right() - 14.0, rect.center().y),
                        Vec2::splat(15.0),
                    ),
                    Icon::PanelLeft,
                    theme::MUTED,
                );
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    self.open_logs = true;
                }
            });

        let members: Vec<_> = v
            .agent_members()
            .into_iter()
            .filter(|m| !m.removed)
            .collect();
        if members.is_empty() && v.state.messages.is_empty() {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(v.tr.t("team.messages.chats.emptyList")).color(theme::MUTED),
                );
            });
            return;
        }

        let mut rows: Vec<RowKind> = vec![RowKind::Chat(None)];
        if v.actor.is_some() {
            rows.push(RowKind::Inbox);
        }
        let groups: Vec<String> = v
            .team()
            .map(|t| t.group_chats.iter().map(|g| g.id.clone()).collect())
            .unwrap_or_default();
        let member_rows: Vec<RowKind> = members
            .iter()
            .map(|m| RowKind::Chat(Some(m.name.clone())))
            .collect();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for r in rows {
                    self.row(ui, v, out, r);
                }
                self.group_header(ui, v);
                for id in &groups {
                    self.row(ui, v, out, RowKind::Chat(group_key(id)));
                }
                if !groups.is_empty() {
                    ui.add_space(4.0);
                }
                for r in member_rows {
                    self.row(ui, v, out, r);
                }
            });
    }

    /// "Group chats" section header with the new-chat button.
    fn group_header(&mut self, ui: &mut egui::Ui, v: &View) {
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(
                RichText::new(v.tr.t("rteams.group.section"))
                    .size(11.5)
                    .color(theme::MUTED),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if v.actor.is_some()
                    && theme::icon_button(ui, Icon::Plus, &v.tr.t("rteams.group.new")).clicked()
                {
                    self.group_draft = Some(GroupDraft::default());
                }
            });
        });
    }

    fn edit_group(&mut self, v: &View, id: &str) {
        if let Some(g) = v
            .team()
            .and_then(|t| t.group_chats.iter().find(|g| g.id == id))
        {
            self.group_draft = Some(GroupDraft {
                id: Some(g.id.clone()),
                name: g.name.clone(),
                members: g.members.iter().cloned().collect(),
            });
        }
    }

    /// The new/edit dialog and the delete confirmation.
    fn group_dialogs(&mut self, ctx: &egui::Context, v: &View, out: &mut Intents) {
        if let Some(mut d) = self.group_draft.take() {
            let mut open = true;
            let mut save = false;
            let mut cancel = false;
            let title = v.tr.t(if d.id.is_some() {
                "rteams.group.edit"
            } else {
                "rteams.group.new"
            });
            let names: Vec<String> = v
                .agent_members()
                .iter()
                .filter(|m| !m.removed)
                .map(|m| m.name.clone())
                .collect();
            theme::dialog(ctx, "group-chat-dialog", &title, 380.0, &mut open, |ui| {
                ui.label(RichText::new(v.tr.t("rteams.group.name")).color(theme::TEXT_2));
                ui.add(
                    egui::TextEdit::singleline(&mut d.name)
                        .hint_text(v.tr.t("rteams.group.namePlaceholder"))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(8.0);
                ui.label(RichText::new(v.tr.t("rteams.group.members")).color(theme::TEXT_2));
                for n in &names {
                    let mut on = d.members.contains(n);
                    if ui.checkbox(&mut on, n).changed() {
                        if on {
                            d.members.insert(n.clone());
                        } else {
                            d.members.remove(n);
                        }
                    }
                }
                ui.add_space(8.0);
                let problem = d.problem();
                if d.id.is_none() && !d.name.trim().is_empty() {
                    if let Some(p) = problem {
                        ui.label(RichText::new(v.tr.t(p)).size(12.0).color(theme::MUTED));
                    }
                }
                ui.horizontal(|ui| {
                    let label = v.tr.t(if d.id.is_some() {
                        "rteams.group.save"
                    } else {
                        "rteams.group.create"
                    });
                    ui.add_enabled_ui(problem.is_none(), |ui| {
                        if theme::solid(ui, &label, theme::ACCENT).clicked() {
                            save = true;
                        }
                    });
                    if theme::ghost(ui, &v.tr.t("common.actions.cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
            open &= !cancel;
            if save {
                let existing: Vec<String> = v
                    .team()
                    .map(|t| t.group_chats.iter().map(|g| g.id.clone()).collect())
                    .unwrap_or_default();
                if let Some(cmd) = d.command(&existing) {
                    if let GuiCmd::GroupChatCreate { id, .. } = &cmd {
                        self.open_chat(group_key(id));
                        self.full = true;
                    }
                    out.push(Intent::Run(cmd));
                }
            } else if open {
                self.group_draft = Some(d);
            }
        }
        if let Some(id) = self.group_delete.clone() {
            let mut open = true;
            let mut yes = false;
            let mut cancel = false;
            theme::dialog(
                ctx,
                "group-chat-delete",
                &v.tr.t("rteams.group.delete"),
                340.0,
                &mut open,
                |ui| {
                    ui.label(group_name(v, &id));
                    ui.label(
                        RichText::new(v.tr.t("rteams.group.confirmDelete")).color(theme::TEXT_2),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if theme::solid(ui, &v.tr.t("rteams.group.delete"), theme::ERROR).clicked()
                        {
                            yes = true;
                        }
                        if theme::ghost(ui, &v.tr.t("common.actions.cancel")).clicked() {
                            cancel = true;
                        }
                    });
                },
            );
            open &= !cancel;
            if yes {
                if group_id(self.selected.as_deref()) == Some(id.as_str()) {
                    self.open_chat(None);
                }
                out.push(Intent::Run(GuiCmd::GroupChatDelete { id }));
                self.group_delete = None;
            } else if !open {
                self.group_delete = None;
            }
        }
    }

    fn row(&mut self, ui: &mut egui::Ui, v: &View, out: &mut Intents, kind: RowKind) {
        let w = ui.available_width().max(1.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 73.0), Sense::hover());
        let (selected, _) = match &kind {
            RowKind::Inbox => (self.inbox, None),
            RowKind::Chat(c) => (!self.inbox && &self.selected == c, c.clone()),
        };
        let id = ui.make_persistent_id(("chat-row", &kind));
        let resp = ui
            .interact(rect, id, Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let row_bg = if selected {
            theme::RAISED
        } else if resp.hovered() {
            theme::SIDEBAR.lerp_to_gamma(Color32::WHITE, 0.04)
        } else {
            theme::SIDEBAR
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(10), row_bg);

        // data
        let (name, last, unread): (String, Option<(String, String, i64)>, usize) = match &kind {
            RowKind::Inbox => {
                let items = inbox_items(v);
                let last = items.last().map(|i| (i.from.clone(), i.text.clone(), i.ts));
                ("My inbox".to_string(), last, self.inbox_unread(v))
            }
            RowKind::Chat(c) => {
                let msgs = chat_messages(&v.state.messages, c.as_deref());
                let last = msgs.last().map(|l| {
                    let who = if Some(l.from.as_str()) == v.actor {
                        v.tr.t("team.messages.chats.you")
                    } else {
                        l.from.clone()
                    };
                    (
                        if Some(l.from.as_str()) == v.actor {
                            who
                        } else {
                            l.from.clone()
                        },
                        l.text.clone(),
                        l.ts,
                    )
                });
                (
                    match group_id(c.as_deref()) {
                        Some(id) => group_name(v, id),
                        None => c
                            .clone()
                            .unwrap_or_else(|| v.tr.t("team.messages.chats.teamFeed")),
                    },
                    last,
                    self.unread(v, c.as_deref()),
                )
            }
        };

        // avatar
        let av = Rect::from_min_size(rect.left_top() + Vec2::new(10.0, 14.5), Vec2::splat(44.0));
        match &kind {
            RowKind::Inbox => {
                let c = av.center();
                ui.painter()
                    .circle_filled(c, 22.0, theme::tint(theme::INFO, 0.16));
                glyph(
                    ui.painter(),
                    Rect::from_center_size(c, Vec2::splat(22.0)),
                    Glyph::Inbox,
                    theme::INFO,
                    row_bg,
                );
            }
            RowKind::Chat(None) => {
                let c = av.center();
                ui.painter()
                    .circle_filled(c, 22.0, theme::tint(theme::TEXT_2, 0.15));
                icons::draw(
                    ui.painter(),
                    Rect::from_center_size(c, Vec2::splat(20.0)),
                    Icon::Users,
                    theme::TEXT_2,
                );
            }
            RowKind::Chat(Some(g)) if group_id(Some(g)).is_some() => {
                let c = av.center();
                ui.painter()
                    .circle_filled(c, 22.0, theme::tint(theme::ACCENT, 0.16));
                icons::draw(
                    ui.painter(),
                    Rect::from_center_size(c, Vec2::splat(20.0)),
                    Icon::Users,
                    theme::ACCENT,
                );
            }
            RowKind::Chat(Some(m)) => {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(av));
                theme::avatar(&mut child, m, v.color(m), 44.0, None);
                let at = av.right_bottom() - Vec2::new(5.0, 5.0);
                let dot = if online(v, m) {
                    theme::SUCCESS
                } else {
                    theme::ERROR
                };
                ui.painter().circle_filled(at, 6.5, row_bg);
                ui.painter().circle_filled(at, 4.5, dot);
            }
        }

        // text
        let left = av.right() + 12.0;
        let right = (rect.right() - 12.0).max(left);
        let painter = ui.painter().with_clip_rect(rect);
        painter.text(
            egui::pos2(left, rect.top() + 24.0),
            egui::Align2::LEFT_CENTER,
            &name,
            egui::FontId::proportional(16.0),
            theme::TEXT,
        );
        if let Some((who, text, ts)) = &last {
            painter.text(
                egui::pos2(right, rect.top() + 24.0),
                egui::Align2::RIGHT_CENTER,
                theme::hhmm(*ts),
                egui::FontId::proportional(12.0),
                theme::MUTED,
            );
            let badge_w = if unread > 0 { 30.0 } else { 0.0 };
            let line_y = rect.top() + 50.0;
            let sender_color = v.color(who);
            paint_tiny_avatar(
                &painter,
                egui::pos2(left + 7.0, line_y),
                7.0,
                who,
                sender_color,
            );
            let font = egui::FontId::proportional(13.0);
            let avail = (right - left - 18.0 - badge_w).max(0.0);
            let line = fit(
                &painter,
                &preview(&format!("{who}: {text}"), 200),
                &font,
                avail,
            );
            painter.text(
                egui::pos2(left + 18.0, line_y),
                egui::Align2::LEFT_CENTER,
                line,
                font,
                theme::TEXT_2,
            );
        }
        if unread > 0 {
            let label = if unread > 99 {
                "99+".to_string()
            } else {
                unread.to_string()
            };
            let g = painter.layout_no_wrap(
                label,
                egui::FontId::proportional(11.5),
                if kind == RowKind::Inbox {
                    Color32::WHITE
                } else {
                    theme::SURFACE
                },
            );
            let bw = (g.size().x + 12.0).max(20.0);
            let br = Rect::from_center_size(
                egui::pos2(right - bw / 2.0, rect.top() + 50.0),
                Vec2::new(bw, 20.0),
            );
            let fill = if kind == RowKind::Inbox {
                theme::INFO
            } else {
                Color32::WHITE
            };
            painter.rect_filled(br, CornerRadius::same(255), fill);
            painter.galley(br.center() - g.size() / 2.0, g, Color32::PLACEHOLDER);
        }

        if let Some(gid) = group_id(match &kind {
            RowKind::Chat(c) => c.as_deref(),
            RowKind::Inbox => None,
        })
        .map(str::to_string)
        {
            resp.context_menu(|ui| {
                if theme::menu_item(ui, Some(Icon::Edit), &v.tr.t("rteams.group.edit")).clicked() {
                    self.edit_group(v, &gid);
                    ui.close();
                }
                if theme::menu_item(ui, Some(Icon::Trash), &v.tr.t("rteams.group.delete")).clicked()
                {
                    self.group_delete = Some(gid.clone());
                    ui.close();
                }
            });
        }
        if resp.clicked() {
            match kind {
                RowKind::Inbox => {
                    self.open_inbox();
                    self.mark_inbox_read(v);
                    // Like chat rows: show the conversation pane (it only draws in full-screen chat).
                    self.full = true;
                }
                RowKind::Chat(c) => {
                    self.open_chat(c.clone());
                    self.mark_read(v, c.as_deref());
                    if group_id(c.as_deref()).is_some() {
                        self.full = true;
                    } else {
                        out.push(Intent::OpenChat(c));
                    }
                }
            }
        }
    }

    // ---- conversation ----

    pub fn chat(&mut self, ui: &mut egui::Ui, v: &View, out: &mut Intents) {
        let chat = self.selected.clone();
        if self.inbox {
            self.mark_inbox_read(v);
        } else {
            self.mark_read(v, chat.as_deref());
        }

        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.inbox {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                        ui.painter().circle_filled(
                            r.center(),
                            14.0,
                            theme::tint(theme::INFO, 0.16),
                        );
                        glyph(
                            ui.painter(),
                            Rect::from_center_size(r.center(), Vec2::splat(16.0)),
                            Glyph::Inbox,
                            theme::INFO,
                            theme::SURFACE,
                        );
                    } else {
                        match &chat {
                            Some(g) if group_id(Some(g)).is_some() => {
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                                ui.painter().circle_filled(
                                    r.center(),
                                    14.0,
                                    theme::tint(theme::ACCENT, 0.16),
                                );
                                icons::draw(
                                    ui.painter(),
                                    Rect::from_center_size(r.center(), Vec2::splat(16.0)),
                                    Icon::Users,
                                    theme::ACCENT,
                                );
                            }
                            None => {
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
                                ui.painter().circle_filled(
                                    r.center(),
                                    14.0,
                                    theme::tint(theme::TEXT_2, 0.15),
                                );
                                icons::draw(
                                    ui.painter(),
                                    Rect::from_center_size(r.center(), Vec2::splat(16.0)),
                                    Icon::Users,
                                    theme::TEXT_2,
                                );
                            }
                            Some(m) => {
                                theme::avatar(ui, m, v.color(m), 28.0, None);
                            }
                        }
                    }
                    ui.label(RichText::new(self.chat_title(v)).size(16.0).strong());
                    if let Some(gid) = group_id(chat.as_deref()).map(str::to_string) {
                        let members = v
                            .team()
                            .and_then(|t| t.group_chats.iter().find(|g| g.id == gid))
                            .map(|g| g.members.clone())
                            .unwrap_or_default();
                        ui.label(
                            RichText::new(v.tr.t_args(
                                "rteams.group.membersCount",
                                &[("n", &members.len().to_string())],
                            ))
                            .size(12.0)
                            .color(theme::MUTED),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let more = theme::icon_button(ui, Icon::MoreHorizontal, "");
                            egui::Popup::menu(&more).width(200.0).show(|ui| {
                                if theme::menu_item(
                                    ui,
                                    Some(Icon::Edit),
                                    &v.tr.t("rteams.group.edit"),
                                )
                                .clicked()
                                {
                                    self.edit_group(v, &gid);
                                    ui.close();
                                }
                                if theme::menu_item(
                                    ui,
                                    Some(Icon::Trash),
                                    &v.tr.t("rteams.group.delete"),
                                )
                                .clicked()
                                {
                                    self.group_delete = Some(gid.clone());
                                    ui.close();
                                }
                            });
                            for m in members.iter().rev().take(5) {
                                theme::avatar(ui, m, v.color(m), 22.0, None);
                            }
                        });
                    }
                });
            });
        theme::rule(ui);

        egui::Panel::bottom(ui.make_persistent_id("composer"))
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(12)))
            .show_separator_line(false)
            .show(ui, |ui| self.composer(ui, v, out));

        if self.inbox {
            self.inbox_list(ui, v, out);
            return;
        }

        let msgs = chat_messages(&v.state.messages, chat.as_deref());
        let mut act = BubbleAct {
            home: self.home.clone(),
            ..Default::default()
        };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add_space(12.0);
                if msgs.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(v.tr.t("team.messages.chats.emptyThread"))
                                .color(theme::MUTED),
                        );
                    });
                }
                for m in msgs {
                    bubble(ui, v, m, out, &mut act);
                    ui.add_space(12.0);
                }
            });
        if let Some((from, text)) = act.reply {
            let is_member = v.agent_members().iter().any(|a| a.name == from);
            self.to = Some(is_member.then_some(from.clone()));
            self.cross = None;
            self.quote = Some((from, text));
            self.focus_composer = true;
        }
    }

    fn inbox_list(&mut self, ui: &mut egui::Ui, v: &View, out: &mut Intents) {
        let items = inbox_items(v);
        let seen = self
            .seen
            .get(&Some(INBOX.to_string()))
            .copied()
            .unwrap_or(0);
        let mut reply: Option<InboxItem> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.add_space(12.0);
                if items.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("Nothing addressed to you yet.").color(theme::MUTED),
                        );
                    });
                }
                for it in &items {
                    let sel = self.inbox_sel == Some(it.seq);
                    // Vertical: inside a horizontal row the card's contents were laid out side
                    // by side, squeezing the message text into a sliver at the right edge.
                    ui.vertical(|ui| {
                        let w = (ui.available_width() - 24.0).clamp(120.0, 860.0);
                        egui::Frame::new()
                            .outer_margin(egui::Margin {
                                left: 12,
                                right: 0,
                                top: 0,
                                bottom: 0,
                            })
                            .fill(theme::RAISED)
                            .stroke(Stroke::new(
                                1.0,
                                if sel {
                                    theme::tint(theme::ACCENT, 0.6)
                                } else {
                                    theme::hairline(0.12)
                                },
                            ))
                            .corner_radius(CornerRadius::same(12))
                            .inner_margin(egui::Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_width(w - 30.0);
                                ui.horizontal(|ui| {
                                    let c = v.color(&it.from);
                                    let (r, _) =
                                        ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                                    paint_tiny_avatar(ui.painter(), r.center(), 11.0, &it.from, c);
                                    ui.label(RichText::new(&it.from).strong().size(13.0).color(c));
                                    theme::pill(ui, it.kind.label(), it.kind.color());
                                    if it.seq > seen {
                                        let (r, _) = ui
                                            .allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                                        ui.painter().circle_filled(r.center(), 3.5, theme::INFO);
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            RichText::new(theme::hhmm(it.ts))
                                                .size(12.0)
                                                .color(theme::MUTED),
                                        );
                                    });
                                });
                                if let Some(t) = &it.task {
                                    let subject = v
                                        .state
                                        .tasks
                                        .iter()
                                        .find(|x| &x.id == t)
                                        .map_or(String::new(), |x| x.subject.clone());
                                    let r = ui
                                        .add(
                                            egui::Label::new(
                                                RichText::new(format!(
                                                    "#{}  {}",
                                                    theme::short_id(t),
                                                    preview(&subject, 80)
                                                ))
                                                .size(12.0)
                                                .color(theme::INFO),
                                            )
                                            .sense(Sense::click()),
                                        )
                                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                                    if r.clicked() {
                                        out.push(Intent::OpenTask(t.clone()));
                                    }
                                }
                                ui.add_space(4.0);
                                readable_text(ui, &it.text, v, out);
                                ui.add_space(4.0);
                                if theme::pill_icon_button(ui, None, "Reply", sel).clicked() {
                                    reply = Some(it.clone());
                                }
                            });
                    });
                    ui.add_space(10.0);
                }
            });
        if let Some(it) = reply {
            let is_member = v.agent_members().iter().any(|a| a.name == it.from);
            self.to = Some(is_member.then_some(it.from.clone()));
            self.cross = None;
            self.inbox_sel = Some(it.seq);
            self.reply_comment = false;
            self.focus_composer = true;
        }
    }

    fn composer(&mut self, ui: &mut egui::Ui, v: &View, out: &mut Intents) {
        let to = self.to.clone().unwrap_or_else(|| self.selected.clone());
        let can_write = v.actor.is_some();
        let sel_task = self
            .inbox
            .then(|| {
                let seq = self.inbox_sel?;
                inbox_items(v)
                    .into_iter()
                    .find(|i| i.seq == seq)
                    .and_then(|i| i.task)
            })
            .flatten();
        egui::Frame::new()
            .fill(theme::RAISED)
            .stroke(Stroke::new(1.0, theme::hairline(0.12)))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                if let Some((who, text)) = self.quote.clone() {
                    let mut clear = false;
                    ui.horizontal(|ui| {
                        let r = egui::Frame::new()
                            .fill(theme::tint(theme::INFO, 0.08))
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(egui::Margin {
                                left: 12,
                                right: 8,
                                top: 4,
                                bottom: 4,
                            })
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!("{who}: {}", preview(&text, 90)))
                                        .size(12.0)
                                        .color(theme::TEXT_2),
                                );
                            });
                        let rr = r.response.rect;
                        ui.painter().rect_filled(
                            Rect::from_min_size(rr.min, Vec2::new(3.0, rr.height())),
                            CornerRadius::same(2),
                            theme::INFO,
                        );
                        if theme::icon_button(ui, Icon::X, "").clicked() {
                            clear = true;
                        }
                    });
                    if clear {
                        self.quote = None;
                    }
                    ui.add_space(4.0);
                }
                ui.horizontal(|ui| {
                    let current = match &self.cross {
                        Some((t, m)) => format!("{t}/{m}"),
                        None => to
                            .clone()
                            .unwrap_or_else(|| v.tr.t("team.messageComposer.recipient.all")),
                    };
                    let icon = if to.is_some() && self.cross.is_none() {
                        Icon::MessageSquare
                    } else {
                        Icon::Users
                    };
                    let resp = theme::pill_icon_button(ui, Some(icon), &current, false);
                    egui::Popup::menu(&resp).width(240.0).show(|ui| {
                        let mut pick = None;
                        let local = self.cross.is_none();
                        if recipient_row(
                            ui,
                            &v.tr.t("team.messageComposer.recipient.all"),
                            None,
                            local && to.is_none(),
                        )
                        .clicked()
                        {
                            pick = Some(None);
                        }
                        for m in v.agent_members().into_iter().filter(|m| !m.removed) {
                            let sel = local && to.as_deref() == Some(m.name.as_str());
                            let dot = if online(v, &m.name) {
                                theme::SUCCESS
                            } else {
                                theme::MUTED
                            };
                            if recipient_row(ui, &m.name, Some(dot), sel).clicked() {
                                pick = Some(Some(m.name.clone()));
                            }
                        }
                        if pick.is_some() {
                            self.to = pick;
                            self.cross = None;
                            ui.close();
                        }
                        self.refresh_other_teams();
                        if !self.other_teams.is_empty() {
                            ui.separator();
                            ui.label(
                                RichText::new(v.tr.t("team.list.sections.otherTeams"))
                                    .size(11.0)
                                    .color(theme::MUTED),
                            );
                            let mut chosen = None;
                            for (team, members) in &self.other_teams {
                                for m in members {
                                    let sel =
                                        self.cross.as_ref() == Some(&(team.clone(), m.clone()));
                                    if recipient_row(ui, &format!("{team}/{m}"), None, sel)
                                        .clicked()
                                    {
                                        chosen = Some((team.clone(), m.clone()));
                                    }
                                }
                            }
                            if chosen.is_some() {
                                self.cross = chosen;
                                ui.close();
                            }
                        }
                    });
                    if sel_task.is_some() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            theme::switch(ui, &mut self.reply_comment);
                            ui.label(
                                RichText::new("Reply as comment")
                                    .size(12.0)
                                    .color(theme::TEXT_2),
                            );
                        });
                    }
                });
                ui.add_space(4.0);

                if std::mem::take(&mut self.focus_composer) {
                    self.composer.focus = true;
                }
                let ev = self.composer.show(ui, v, &mut self.text);
                let mut send = ev.send;
                let mut atts = ev.attachments;
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let labels: Vec<String> =
                        Mode::ALL.iter().map(|m| v.tr.t(m.label_key())).collect();
                    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                    let mut idx = Mode::ALL.iter().position(|m| *m == self.mode).unwrap_or(0);
                    if theme::segmented(ui, &mut idx, &refs) {
                        self.mode = Mode::ALL[idx];
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let has_atts = !atts.is_empty() || !self.composer.attachments().is_empty();
                        let ready = can_write && (!self.text.trim().is_empty() || has_atts);
                        let resp =
                            send_button(ui, &v.tr.t("team.messageComposer.actions.send"), ready);
                        if resp.clicked() {
                            send = true;
                            if atts.is_empty() {
                                atts = self.composer.attachments().to_vec();
                                let dir = self.composer.attach_dir.take();
                                self.composer = crate::ui::composer::Composer::default();
                                self.composer.attach_dir = dir;
                            }
                        }
                        if send && ready {
                            let body = with_attachments(self.text.trim(), &atts);
                            if let (true, Some(task)) = (self.reply_comment, sel_task.clone()) {
                                let body = with_file_refs(&body, &atts);
                                out.push(Intent::Run(GuiCmd::Comment { task, text: body }));
                                self.text.clear();
                                self.quote = None;
                            } else {
                                let text = match &self.quote {
                                    Some((who, q)) => {
                                        format!("> {who}: {}\n\n{body}", preview(q, 160))
                                    }
                                    None => body,
                                };
                                out.push(send_intent(
                                    self.cross.as_ref(),
                                    to.clone(),
                                    &text,
                                    self.mode,
                                    atts,
                                ));
                            }
                        }
                    });
                });
            });
    }
}

fn recipient_row(
    ui: &mut egui::Ui,
    label: &str,
    dot: Option<Color32>,
    selected: bool,
) -> egui::Response {
    let w = ui.available_width().max(160.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 30.0), Sense::click());
    if selected || resp.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(7),
            if selected {
                theme::tint(theme::ACCENT, 0.16)
            } else {
                Color32::from_rgb(0x2c, 0x2e, 0x3b)
            },
        );
    }
    let mut x = rect.left() + 12.0;
    if let Some(c) = dot {
        ui.painter()
            .circle_filled(egui::pos2(x + 4.0, rect.center().y), 4.0, c);
        x += 16.0;
    }
    ui.painter().text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        if selected { theme::ACCENT } else { theme::TEXT },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn send_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let font = egui::FontId::proportional(14.0);
    let ink = Color32::from_rgb(0x08, 0x14, 0x20);
    let g = ui.painter().layout_no_wrap(label.to_string(), font, ink);
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(g.size().x + 54.0, 34.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let fill = if !enabled {
        theme::tint(theme::SEND, 0.35)
    } else if resp.hovered() {
        theme::SEND.gamma_multiply(0.9)
    } else {
        theme::SEND
    };
    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    icons::draw(
        ui.painter(),
        Rect::from_center_size(
            egui::pos2(rect.left() + 21.0, rect.center().y),
            Vec2::splat(15.0),
        ),
        Icon::Send,
        ink,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 36.0, rect.center().y - g.size().y / 2.0),
        g,
        ink,
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// The intent for pressing Send: a cross-team delivery, or a plain message in this team.
fn send_intent(
    cross: Option<&(String, String)>,
    to: Chat,
    text: &str,
    mode: Mode,
    attachments: Vec<Attachment>,
) -> Intent {
    let mode = Some(mode_str(mode).to_string());
    match cross {
        Some((team, member)) => Intent::CrossTeamSend {
            team: team.clone(),
            to: Some(member.clone()),
            text: with_file_refs(text, &attachments),
        },
        None => {
            if let Some(id) = group_id(to.as_deref()) {
                return Intent::Run(GuiCmd::MessageRich {
                    to: None,
                    chat: Some(id.to_string()),
                    text: text.to_string(),
                    mode,
                    attachments,
                });
            }
            if attachments.is_empty() {
                Intent::Run(GuiCmd::Message {
                    to,
                    text: text.to_string(),
                    mode,
                })
            } else {
                Intent::Run(GuiCmd::MessageRich {
                    to,
                    chat: None,
                    text: text.to_string(),
                    mode,
                    attachments,
                })
            }
        }
    }
}

/// Empty text with attachments becomes a short list of the files; other text is unchanged.
pub fn with_attachments(text: &str, atts: &[Attachment]) -> String {
    if text.trim().is_empty() && !atts.is_empty() {
        let names: Vec<&str> = atts.iter().map(|a| a.name.as_str()).collect();
        return format!("Attached: {}", names.join(", "));
    }
    text.to_string()
}

/// `text` plus a markdown reference per file, for channels that carry no attachments.
pub fn with_file_refs(text: &str, atts: &[Attachment]) -> String {
    let mut out = with_attachments(text, &[]);
    for a in atts {
        out.push('\n');
        out.push_str(&rteams_services::attachments::attachment_markdown(a));
    }
    out
}

const CROSS_TEAM: Color32 = Color32::from_rgb(192, 132, 252);

fn bubble(ui: &mut egui::Ui, v: &View, m: &Message, out: &mut Intents, act: &mut BubbleAct) {
    let mine = Some(m.from.as_str()) == v.actor && m.from_team.is_none();
    let layout = if mine {
        Layout::right_to_left(Align::Min)
    } else {
        Layout::left_to_right(Align::Min)
    };
    let max_w = (ui.available_width() - 32.0 - 10.0 - 24.0 - 56.0).clamp(160.0, 820.0);
    let sender_color = if m.from_team.is_some() {
        CROSS_TEAM
    } else {
        v.color(&m.from)
    };
    ui.horizontal_top(|ui| {
        ui.add_space(12.0);
        let w = (ui.available_width() - 12.0).max(1.0);
        ui.allocate_ui_with_layout(Vec2::new(w, 0.0), layout, |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            theme::avatar(ui, &m.from, sender_color, 32.0, None);
            let (fill, border) = if m.from_team.is_some() {
                (
                    theme::RAISED.blend(Color32::from_rgba_unmultiplied(168, 85, 247, 15)),
                    Color32::from_rgba_unmultiplied(192, 132, 252, 90),
                )
            } else if mine {
                (
                    theme::RAISED,
                    Color32::from_rgba_unmultiplied(249, 115, 22, 90),
                )
            } else {
                (theme::RAISED, theme::hairline(0.12))
            };
            ui.vertical(|ui| {
                let fr = egui::Frame::new()
                    .fill(fill)
                    .stroke(Stroke::new(1.0, border))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_max_width(max_w);
                        ui.spacing_mut().item_spacing.y = 3.0;
                        bubble_header(ui, v, m, sender_color);
                        ui.add_space(2.0);
                        bubble_body(ui, v, m, out);
                        attachments_row(ui, m, act.home.as_ref());
                    });
                hover_toolbar(ui, v, m, fr.response.rect, fill, mine, out, act);
                reactions_row(ui, v, m, out);
            });
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn hover_toolbar(
    ui: &mut egui::Ui,
    v: &View,
    m: &Message,
    bubble: Rect,
    _bubble_fill: Color32,
    mine: bool,
    out: &mut Intents,
    act: &mut BubbleAct,
) {
    let more_id = ui.scope_id().with(("msg-more", m.seq));
    let menu_open = egui::Popup::is_id_open(ui.ctx(), more_id);
    let time = theme::hhmm(m.ts);
    let tw = 4.0 * 24.0 + 8.0 + 46.0;
    let right = if mine {
        bubble.left() + tw + 8.0
    } else {
        bubble.right() - 8.0
    };
    let tb = Rect::from_min_size(
        egui::pos2(right - tw, bubble.top() - 14.0),
        Vec2::new(tw, 28.0),
    );
    let hot = ui.rect_contains_pointer(bubble.union(tb)) || menu_open;
    if !hot {
        return;
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(tb)
            .layout(Layout::left_to_right(Align::Center)),
    );
    egui::Frame::new()
        .fill(theme::CARD_HEADER)
        .stroke(Stroke::new(1.0, theme::hairline(0.14)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(3, 1))
        .show(&mut child, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if tool_btn(ui, Glyph::Reply, "Reply", theme::CARD_HEADER).clicked() {
                act.reply = Some((m.from.clone(), m.text.clone()));
            }
            if tool_btn(
                ui,
                Glyph::Copy,
                &v.tr.t("team.terminalWorkspace.copy"),
                theme::CARD_HEADER,
            )
            .clicked()
            {
                ui.ctx().copy_text(m.text.clone());
            }
            ui.add_space(4.0);
            ui.label(RichText::new(time).size(11.5).color(theme::MUTED));
            ui.add_space(4.0);
            let more = tool_btn(ui, Glyph::More, "", theme::CARD_HEADER);
            egui::Popup::menu(&more)
                .id(more_id)
                .width(220.0)
                .show(|ui| {
                    if v.actor.is_some() {
                        ui.horizontal(|ui| {
                            for e in QUICK_EMOJI {
                                if ui.button(e).clicked() {
                                    out.push(Intent::Run(GuiCmd::React {
                                        message_seq: m.seq,
                                        emoji: e.to_string(),
                                    }));
                                    ui.close();
                                }
                            }
                        });
                        let agent = m.from_team.is_none()
                            && v.agent_members().iter().any(|a| a.name == m.from);
                        if theme::menu_item(
                            ui,
                            Some(Icon::Plus),
                            &v.tr.t("team.activity.actions.createTaskFromMessage"),
                        )
                        .clicked()
                        {
                            out.push(Intent::Run(GuiCmd::CreateTaskFromMessage {
                                message_seq: m.seq,
                                owner: agent.then(|| m.from.clone()),
                            }));
                            ui.close();
                        }
                    }
                });
        });
}

fn bubble_header(ui: &mut egui::Ui, v: &View, m: &Message, color: Color32) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let who = match &m.from_team {
            Some(t) => format!("{t}/{}", m.from),
            None => m.from.clone(),
        };
        ui.label(RichText::new(who).strong().size(12.5).color(color));
        icons::icon(ui, Icon::ChevronRight, 12.0, theme::MUTED);
        match &m.scope {
            Scope::Direct { to } => {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                paint_tiny_avatar(ui.painter(), r.center(), 7.0, to, v.color(to));
                ui.label(RichText::new(to).size(12.0).color(theme::TEXT_2));
            }
            Scope::Task { task } => {
                ui.label(
                    RichText::new(format!("#{}", theme::short_id(task)))
                        .size(12.0)
                        .color(theme::TEXT_2),
                );
            }
            Scope::Broadcast => {
                ui.label(RichText::new("all").size(12.0).color(theme::TEXT_2));
            }
            Scope::Group { chat } => {
                ui.label(
                    RichText::new(group_name(v, chat))
                        .size(12.0)
                        .color(theme::TEXT_2),
                );
            }
        }
        if let Some(mode) = m.mode.as_deref() {
            theme::pill(ui, mode, theme::INFO);
        }
    });
}

fn quote_block(ui: &mut egui::Ui, text: &str) {
    let r = egui::Frame::new()
        .fill(theme::tint(theme::INFO, 0.08))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin {
            left: 12,
            right: 8,
            top: 5,
            bottom: 5,
        })
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.5).color(theme::TEXT_2));
        });
    let rr = r.response.rect;
    ui.painter().rect_filled(
        Rect::from_min_size(rr.min, Vec2::new(3.0, rr.height())),
        CornerRadius::same(2),
        theme::INFO,
    );
    ui.add_space(4.0);
}

/// Split text into display lines, flagging bullets. Lines starting with `- `, `* `, `• `, `N)` or
/// `N.` are bullets; a one-paragraph message with an inline `1) .. 2) ..` list is split into a lead
/// line plus one bullet per item, so agent replies stay readable even when not formatted.
pub fn to_lines(text: &str) -> Vec<(bool, String)> {
    let item = regex::Regex::new(r"^\s*(?:[-*•]|\d{1,2}[.)])\s+").unwrap();
    let inline = regex::Regex::new(r"(?:^|\s)(\d{1,2})\)\s").unwrap();
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(m) = item.find(line) {
            out.push((true, line[m.end()..].trim().to_string()));
            continue;
        }
        // Inline enumeration: "Lead. 1) first. 2) second." -> lead + bullets.
        let starts: Vec<(usize, usize)> = inline
            .find_iter(line)
            .map(|m| (m.start(), m.end()))
            .collect();
        if starts.len() >= 2 {
            let lead = line[..starts[0].0].trim();
            if !lead.is_empty() {
                out.push((false, lead.to_string()));
            }
            for (i, &(_, body_start)) in starts.iter().enumerate() {
                let end = starts.get(i + 1).map_or(line.len(), |n| n.0);
                let body = line[body_start..end].trim();
                if !body.is_empty() {
                    out.push((true, body.to_string()));
                }
            }
        } else {
            out.push((false, line.to_string()));
        }
    }
    out
}

/// Draw text as readable lines: bullets get a dot and a hanging indent so wrapped lines align.
fn readable_text(ui: &mut egui::Ui, text: &str, v: &View, out: &mut Intents) {
    for (bullet, line) in to_lines(text) {
        if bullet {
            ui.horizontal_top(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("•").color(theme::ACCENT));
                ui.vertical(|ui| links::rich_text(ui, &line, v, out));
            });
        } else {
            links::rich_text(ui, &line, v, out);
        }
        ui.add_space(2.0);
    }
}

fn bubble_body(ui: &mut egui::Ui, v: &View, m: &Message, out: &mut Intents) {
    let (quote, text) = split_quote(&m.text);
    if let Some(q) = quote {
        quote_block(ui, &q);
    }
    // ```viz fences render as charts / tables; the text around them as usual.
    let blocks = rteams_services::viz::extract(text);
    if !blocks.is_empty() {
        let mut at = 0;
        for (range, spec) in &blocks {
            let before = text[at..range.start].trim();
            if !before.is_empty() {
                links::rich_text(ui, before, v, out);
            }
            crate::ui::viz::render_block(ui, spec);
            at = range.end;
        }
        let after = text[at..].trim();
        if !after.is_empty() {
            links::rich_text(ui, after, v, out);
        }
        return;
    }
    match split_summary(text) {
        Some((summary, rest)) => {
            let bold = links::TextLook {
                size: 14.5,
                strong: true,
                ..Default::default()
            };
            links::rich_text_with(ui, summary, v, out, bold);
            ui.add_space(2.0);
            readable_text(ui, rest, v, out);
        }
        None => readable_text(ui, text, v, out),
    }
}

/// Open `path` with the system handler.
fn open_external(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = "xdg-open";
    let _ = std::process::Command::new(cmd).arg(path).spawn();
}

/// Decoded thumbnail of an image attachment, cached per path (`None` = undecodable).
fn thumb(ctx: &egui::Context, path: &std::path::Path) -> Option<egui::TextureHandle> {
    let id = egui::Id::NULL.with(("att-thumb", path));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(id)) {
        return t;
    }
    let tex = rteams_services::attachments::thumbnail(path, 640).map(|(w, h, rgba)| {
        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        ctx.load_texture(path.to_string_lossy(), img, egui::TextureOptions::LINEAR)
    });
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// Images as inline previews, other files as chips; clicking opens the file.
fn attachments_row(ui: &mut egui::Ui, m: &Message, home: Option<&(std::path::PathBuf, String)>) {
    if m.attachments.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
        for a in &m.attachments {
            let path = match home {
                Some((h, t)) => rteams_services::attachments::resolve(h, t, a),
                None => std::path::PathBuf::from(&a.path),
            };
            let tex = rteams_services::attachments::is_image(a)
                .then(|| thumb(ui.ctx(), &path))
                .flatten();
            let resp = match tex {
                Some(t) => ui.add(
                    egui::Image::new(&t)
                        .max_size(Vec2::new(260.0, 180.0))
                        .corner_radius(CornerRadius::same(8))
                        .sense(Sense::click()),
                ),
                None => {
                    let r = egui::Frame::new()
                        .fill(theme::tint(theme::INFO, 0.08))
                        .stroke(Stroke::new(1.0, theme::hairline(0.1)))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                icons::icon(ui, Icon::Paperclip, 13.0, theme::TEXT_2);
                                ui.label(RichText::new(&a.name).size(12.5));
                            });
                        });
                    ui.interact(
                        r.response.rect,
                        ui.scope_id().with(("att", &a.path)),
                        Sense::click(),
                    )
                }
            };
            if resp
                .on_hover_text(path.display().to_string())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                open_external(&path);
            }
        }
    });
}

fn reactions_row(ui: &mut egui::Ui, v: &View, m: &Message, out: &mut Intents) {
    let counts = reaction_counts(&m.reactions, v.actor);
    if counts.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 2.0);
        for c in &counts {
            let (fill, fg) = if c.mine {
                (theme::tint(theme::ACCENT, 0.2), theme::ACCENT)
            } else {
                (theme::RAISED, theme::TEXT_2)
            };
            let b = egui::Button::new(
                RichText::new(format!("{} {}", c.emoji, c.count))
                    .size(12.0)
                    .color(fg),
            )
            .fill(fill)
            .corner_radius(CornerRadius::same(255))
            .min_size(Vec2::new(0.0, 18.0));
            let r = ui
                .add_enabled(v.actor.is_some(), b)
                .on_hover_text(c.who.join(", "));
            if r.clicked() {
                out.push(Intent::Run(GuiCmd::React {
                    message_seq: m.seq,
                    emoji: c.emoji.clone(),
                }));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_lists_and_dash_lines_become_bullets() {
        let l = to_lines(
            "Fixed 1-6. 1) Step token checked. 2) Points never auto-verify. 3) Intro card.",
        );
        assert_eq!(l[0], (false, "Fixed 1-6.".to_string()));
        assert_eq!(l.len(), 4);
        assert!(l[1..].iter().all(|(b, _)| *b));
        assert_eq!(l[2].1, "Points never auto-verify.");
        let l = to_lines("- one\n- two\nplain");
        assert_eq!(
            l,
            vec![
                (true, "one".into()),
                (true, "two".into()),
                (false, "plain".into())
            ]
        );
        assert_eq!(
            to_lines("see 1) only one item"),
            vec![(false, "see 1) only one item".into())]
        );
    }
    use crate::view::testing::{fixture, render};

    fn msg(seq: u64, from: &str, scope: Scope) -> Message {
        Message {
            seq,
            ts: 0,
            from: from.into(),
            scope,
            text: "x".into(),
            mode: None,
            from_team: None,
            reactions: Vec::new(),
            attachments: vec![],
        }
    }

    fn sample() -> Vec<Message> {
        vec![
            msg(1, "boss", Scope::Broadcast),
            msg(2, "boss", Scope::Direct { to: "ada".into() }),
            msg(3, "ada", Scope::Direct { to: "boss".into() }),
            msg(4, "bob", Scope::Direct { to: "boss".into() }),
            msg(5, "ada", Scope::Task { task: "1".into() }),
        ]
    }

    #[test]
    fn membership() {
        let s = sample();
        assert_eq!(chat_messages(&s, None).len(), 1);
        let ada: Vec<u64> = chat_messages(&s, Some("ada"))
            .iter()
            .map(|m| m.seq)
            .collect();
        assert_eq!(ada, vec![2, 3]);
        assert_eq!(chat_messages(&s, Some("bob")).len(), 1);
    }

    #[test]
    fn unread() {
        let s = sample();
        assert_eq!(unread_count(&s, Some("ada"), Some("boss"), 0), 1);
        assert_eq!(unread_count(&s, Some("ada"), Some("boss"), 3), 0);
        assert_eq!(unread_count(&s, None, Some("boss"), 0), 0);
        assert_eq!(unread_count(&s, None, None, 0), 1);
    }

    #[test]
    fn previews() {
        assert_eq!(preview("hello\nworld", 20), "hello");
        assert_eq!(preview("a  b   c", 20), "a b c");
        assert_eq!(preview("abcdefghij", 5), "abcd\u{2026}");
        assert_eq!(preview("abc", 0), "\u{2026}");
    }

    #[test]
    fn mode_strings() {
        assert_eq!(mode_str(Mode::Do), "do");
        assert_eq!(mode_str(Mode::Ask), "ask");
        assert_eq!(mode_str(Mode::Delegate), "delegate");
    }

    #[test]
    fn cursors_round_trip() {
        let path = std::env::temp_dir().join(format!("rteams-cursors-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, r#"{"other": 1}"#).unwrap();
        let mut m = Messages {
            cursor_path: Some(path.clone()),
            ..Default::default()
        };
        m.seen.insert(None, 4);
        m.seen.insert(Some("ada".into()), 9);
        m.save_cursors();
        let mut n = Messages::default();
        n.load_cursors(path.clone());
        assert_eq!(n.seen.get(&None), Some(&4));
        assert_eq!(n.seen.get(&Some("ada".to_string())), Some(&9));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"other\""));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn renders_sidebar_and_chat() {
        let (home, data) = fixture("messages");
        let v = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut m = Messages::default();
        let mut out = Intents::default();
        render(|ui| {
            m.sidebar(ui, &v, &mut out);
        });
        let as_ada = View {
            actor: Some("ada"),
            ..View {
                state: data.state(),
                data: &data,
                actor: None,
                agents: &[],
                tr: crate::view::testing::tr(),
            }
        };
        assert!(m.total_unread(&as_ada) > 0);
        render(|ui| m.chat(ui, &v, &mut out));
        m.open_chat(Some("ada".into()));
        render(|ui| m.chat(ui, &v, &mut out));
        assert_eq!(m.total_unread(&v), 0);
        assert!(out.0.is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    fn react(from: &str, emoji: &str) -> Reaction {
        Reaction {
            from: from.into(),
            emoji: emoji.into(),
        }
    }

    #[test]
    fn counts_group_by_emoji_in_first_use_order() {
        let rs = [react("ada", "👍"), react("bob", "🔥"), react("boss", "👍")];
        let c = reaction_counts(&rs, Some("boss"));
        assert_eq!(c.len(), 2);
        assert_eq!(
            (c[0].emoji.as_str(), c[0].count, c[0].mine),
            ("👍", 2, true)
        );
        assert_eq!(
            (c[1].emoji.as_str(), c[1].count, c[1].mine),
            ("🔥", 1, false)
        );
        assert_eq!(c[0].who, ["ada", "boss"]);
        assert!(reaction_counts(&[], None).is_empty());
        assert!(!reaction_counts(&rs, None)[0].mine);
    }

    #[test]
    fn summary_needs_several_lines() {
        assert_eq!(split_summary("one"), None);
        assert_eq!(split_summary("one\n"), None);
        assert_eq!(split_summary("one\n\n"), None);
        assert_eq!(
            split_summary("Title\nbody\nmore"),
            Some(("Title", "body\nmore"))
        );
        assert_eq!(split_summary("Title\n\nbody"), Some(("Title", "body")));
    }

    #[test]
    fn cross_team_messages_show_for_recipient_and_group() {
        let mut m = msg(1, "zed", Scope::Direct { to: "ada".into() });
        m.from_team = Some("other".into());
        assert!(in_chat(&m, None));
        assert!(in_chat(&m, Some("ada")));
        assert!(!in_chat(&m, Some("zed")));
        let plain = msg(2, "zed", Scope::Direct { to: "ada".into() });
        assert!(!in_chat(&plain, None));
    }

    #[test]
    fn send_picks_cross_team_or_local() {
        let cross = ("other".to_string(), "zed".to_string());
        assert_eq!(
            send_intent(Some(&cross), None, "hi", Mode::Do, vec![]),
            Intent::CrossTeamSend {
                team: "other".into(),
                to: Some("zed".into()),
                text: "hi".into()
            }
        );
        assert!(matches!(
            send_intent(None, Some("ada".into()), "hi", Mode::Ask, vec![]),
            Intent::Run(GuiCmd::Message { .. })
        ));
    }

    #[test]
    fn other_teams_exclude_current_and_list_members() {
        let (home, _d) = fixture("messages-other");
        let mut c = crate::actions::Commander::new(home.clone(), "z".into());
        c.run(
            "boss",
            &GuiCmd::CreateTeam {
                name: "z".into(),
                members: vec![("zed".into(), "dev".into())],
            },
        )
        .unwrap();
        let o = scan_other_teams(&home, "t");
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].0, "z");
        assert!(o[0].1.contains(&"zed".to_string()));
        assert!(scan_other_teams(&home, "z").iter().all(|(t, _)| t == "t"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn renders_reactions_cross_team_and_summary() {
        use crate::actions::Commander;
        let (home, mut data) = fixture("messages-react");
        let mut c = Commander::new(home.clone(), "t".into());
        let seq = data.state().messages[0].seq;
        for e in ["👍", "🔥"] {
            c.run(
                "ada",
                &GuiCmd::React {
                    message_seq: seq,
                    emoji: e.into(),
                },
            )
            .unwrap();
        }
        c.run(
            "boss",
            &GuiCmd::React {
                message_seq: seq,
                emoji: "👍".into(),
            },
        )
        .unwrap();
        c.run(
            "boss",
            &GuiCmd::Message {
                to: None,
                text: "Summary line #1 for @ada\nbody line two".into(),
                mode: None,
            },
        )
        .unwrap();
        data.refresh().unwrap();
        let mut st = data.state().clone();
        let mut x = st.messages[0].clone();
        x.seq = 999;
        x.from = "zed".into();
        x.from_team = Some("other".into());
        st.messages.push(x);
        let v = View {
            state: &st,
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        assert_eq!(v.state.messages[0].reactions.len(), 3);
        let mut m = Messages::default();
        m.set_home(home.clone(), "t".into());
        let mut out = Intents::default();
        render(|ui| m.chat(ui, &v, &mut out));
        let view_only = View {
            actor: None,
            ..View {
                state: &st,
                data: &data,
                actor: None,
                agents: &[],
                tr: crate::view::testing::tr(),
            }
        };
        render(|ui| m.chat(ui, &view_only, &mut out));
        assert!(out.0.is_empty());
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn quote_split() {
        assert_eq!(split_quote("plain"), (None, "plain"));
        assert_eq!(
            split_quote("> ada: hi\n\nreply"),
            (Some("ada: hi".to_string()), "reply")
        );
        assert_eq!(split_quote("> only"), (Some("only".to_string()), ""));
    }

    #[test]
    fn inbox_only_what_is_addressed_to_actor() {
        let (home, data) = fixture("inbox");
        let st = data.state();
        // fixture: boss created tasks 1-4 (owners ada/bob); a message to ada exists.
        let mut msgs = st.messages.clone();
        let mk = |seq: u64, from: &str, scope: Scope, text: &str| Message {
            seq,
            ts: 0,
            from: from.into(),
            scope,
            text: text.into(),
            mode: None,
            from_team: None,
            reactions: Vec::new(),
            attachments: vec![],
        };
        msgs.push(mk(900, "ada", Scope::Direct { to: "boss".into() }, "dm"));
        msgs.push(mk(901, "ada", Scope::Direct { to: "bob".into() }, "not me"));
        msgs.push(mk(
            902,
            "ada",
            Scope::Task { task: "1".into() },
            "on my task",
        ));
        msgs.push(mk(903, "ada", Scope::Broadcast, "ping @boss"));
        msgs.push(mk(904, "ada", Scope::Broadcast, "nobody"));
        msgs.push(mk(905, "boss", Scope::Direct { to: "ada".into() }, "own"));
        let items = inbox_from("boss", &msgs, &st.tasks, data.events());
        let seqs: Vec<u64> = items.iter().map(|i| i.seq).collect();
        for s in [900, 902, 903] {
            assert!(seqs.contains(&s), "{s} missing in {seqs:?}");
        }
        for s in [901, 904, 905] {
            assert!(!seqs.contains(&s), "{s} present");
        }
        let kind = |s: u64| items.iter().find(|i| i.seq == s).unwrap().kind;
        assert_eq!(kind(900), InboxKind::Direct);
        assert_eq!(kind(902), InboxKind::Comment);
        assert_eq!(kind(903), InboxKind::Mention);
        // ada is asked to review task 3's reviewer bob: not addressed to boss.
        assert!(items.iter().all(|i| i.kind != InboxKind::Review));
        let as_bob = inbox_from("bob", &msgs, &st.tasks, data.events());
        assert!(as_bob
            .iter()
            .any(|i| i.kind == InboxKind::Review && i.task.as_deref() == Some("3")));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn inbox_cursor_round_trips() {
        let path = std::env::temp_dir().join(format!("rteams-inbox-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut m = Messages {
            cursor_path: Some(path.clone()),
            ..Default::default()
        };
        m.seen.insert(Some(INBOX.to_string()), 7);
        m.save_cursors();
        let mut n = Messages::default();
        n.load_cursors(path.clone());
        assert_eq!(n.seen.get(&Some(INBOX.to_string())), Some(&7));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn group_ids_keys_and_scoping() {
        assert_eq!(new_group_id("Backend Sync!", &[]), "backend-sync");
        assert_eq!(new_group_id("  ", &[]), "group");
        assert_eq!(
            new_group_id("a", &["a".to_string(), "a-2".to_string()]),
            "a-3"
        );
        let k = group_key("g1");
        assert_eq!(group_id(k.as_deref()), Some("g1"));
        assert_eq!(group_id(Some("ada")), None);
        assert_eq!(cursor_key(k.as_deref()), "grp:g1");
        let g = msg(1, "ada", Scope::Group { chat: "g1".into() });
        assert!(in_chat(&g, k.as_deref()));
        assert!(!in_chat(&g, group_key("g2").as_deref()));
        assert!(!in_chat(&g, None));
        assert!(!in_chat(&g, Some("ada")));
        let p = std::env::temp_dir().join(format!("rteams-grp-{}.json", std::process::id()));
        let mut seen = HashMap::new();
        seen.insert(k.clone(), 5);
        save_cursor_file(&p, &seen);
        assert_eq!(load_cursor_file(&p).get(&k), Some(&5));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn group_draft_validates_and_builds_commands() {
        let mut d = GroupDraft::default();
        assert!(d.command(&[]).is_none());
        d.name = "Core crew".into();
        assert!(d.command(&[]).is_none());
        d.members.insert("ada".into());
        match d.command(&[]).unwrap() {
            GuiCmd::GroupChatCreate { id, name, members } => {
                assert_eq!((id.as_str(), name.as_str()), ("core-crew", "Core crew"));
                assert_eq!(members, vec!["ada".to_string()]);
            }
            other => panic!("{other:?}"),
        }
        d.id = Some("g1".into());
        assert!(matches!(
            d.command(&[]).unwrap(),
            GuiCmd::GroupChatUpdate { .. }
        ));
    }

    #[test]
    fn send_intent_routes_groups_and_attachments() {
        let att = vec![Attachment {
            name: "a.png".into(),
            path: "/x/a.png".into(),
        }];
        let Intent::Run(GuiCmd::MessageRich { chat, to, .. }) =
            send_intent(None, group_key("g1"), "hi", Mode::Do, vec![])
        else {
            panic!()
        };
        assert_eq!((chat.as_deref(), to), (Some("g1"), None));
        assert!(matches!(
            send_intent(None, Some("ada".into()), "hi", Mode::Do, vec![]),
            Intent::Run(GuiCmd::Message { .. })
        ));
        assert!(matches!(
            send_intent(None, Some("ada".into()), "hi", Mode::Do, att.clone()),
            Intent::Run(GuiCmd::MessageRich { .. })
        ));
        assert_eq!(with_attachments("", &att), "Attached: a.png");
        assert_eq!(with_attachments("t", &att), "t");
        assert!(with_file_refs("t", &att).contains("![a.png](/x/a.png)"));
    }

    #[test]
    fn renders_group_chat_with_attachments() {
        use crate::actions::Commander;
        let (home, mut data) = fixture("messages-group");
        let mut c = Commander::new(home.clone(), "t".into());
        c.run(
            "boss",
            &GuiCmd::GroupChatCreate {
                id: "g1".into(),
                name: "Crew".into(),
                members: vec!["ada".into()],
            },
        )
        .unwrap();
        data.refresh().unwrap();
        let mut st = data.state().clone();
        let mut gm = msg(900, "ada", Scope::Group { chat: "g1".into() });
        gm.text = "hello group".into();
        gm.attachments = vec![
            Attachment {
                name: "notes.txt".into(),
                path: "/nonexistent/notes.txt".into(),
            },
            Attachment {
                name: "pic.png".into(),
                path: "/nonexistent/pic.png".into(),
            },
        ];
        st.messages.push(gm);
        let v = View {
            state: &st,
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut m = Messages::default();
        m.set_home(home.clone(), "t".into());
        let mut out = Intents::default();
        render(|ui| m.sidebar(ui, &v, &mut out));
        assert_eq!(m.unread(&v, group_key("g1").as_deref()), 1);
        m.open_chat(group_key("g1"));
        assert_eq!(m.chat_title(&v), "Crew");
        render(|ui| m.chat(ui, &v, &mut out));
        assert_eq!(m.unread(&v, group_key("g1").as_deref()), 0);
        m.group_draft = Some(GroupDraft::default());
        m.group_delete = Some("g1".into());
        render(|ui| m.sidebar(ui, &v, &mut out));
        let _ = std::fs::remove_dir_all(home);
    }
}
