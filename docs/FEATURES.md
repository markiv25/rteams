# rteams feature guide

rteams is a native Rust rebuild of [agent-teams-ai](https://github.com/777genius/agent-teams-ai):
you are the lead, AI coding agents (Claude Code, Codex, OpenCode, ...) are your team, and the
shared state is an append-only event log (`<home>/<team>/events.jsonl`). The names in brackets
are the original's feature names. See [PARITY.md](PARITY.md) for what is still different.

Crates: `rteams-core` (domain, commands, events, projections), `rteams-store` (file store with
locking), `rteams-server` (MCP server, `rteams serve`), `rteams-services` (everything around
the app: config, i18n, usage, schedules, ...), `rteams-gui` (desktop app), `rteams-tui`
(terminal UI), `rteams-cli`.

## Desktop app screens

- **Dashboard** [dashboard]: recent projects, running teams, project picker, update banner.
- **Teams** [team list / create team / templates / import]: all teams under the home, search,
  create (prompt, members, project path), template picker, import dialog.
- **Team view**: header with project path and Launch / Stop team, member rows (status, task
  progress, chat, assign, launch/stop/restart), `+ Add` teammate with parody names and icons,
  member dialog (tasks, logs, messages, stats, edit role/model/prompt/icon/worktree).
- **Kanban** [kanban]: TODO / IN PROGRESS / REVIEW / DONE / APPROVED, search (`#id` or text),
  filters, sort, drag order, Trash with restore, task folders.
- **Task dialog** [TaskDetailDialog]: edit, status actions, related tasks, attachments,
  comments, workflow history, task logs, changed files with unified diff and revert.
- **Change review** [team/review]: hunk-level review (file tree, accept/reject per hunk or file,
  line comments, j/k/a/r keys); reject reverse-applies the hunk with `git apply -R`.
- **Messages** [group chat / direct chats]: Group chat, one chat per agent, user-created group
  chats, unread badges, reply quote, reactions, mode `Do | Ask | Delegate`, attachments.
- **Composer**: `@member`, `#task` and `/command` popups, paste/drop images and files,
  attach code context (file and line range).
- **Activity feed**: lead thoughts grouping and pending-replies block.
- **Graph view** [agent-graph]: agents, tasks, edges, minimap, provisioning HUD.
- **Org map** [organizations]: nested/hierarchy canvas, inspector, team creation.
- **Usage** [token-usage]: analytics and budgets.
- **Schedules** [schedule]: cron editor, run log (also `schedule_set` for the lead).
- **Editor** [team/editor]: file tree, tabs, search, quick open, go to line, markdown preview,
  git badges.
- **Terminal workspace**, **Sessions** (viewer, waterfall, report, search), **Processes** and
  **Claude Logs**.
- **Extensions** [extensions]: MCP servers, plugins, skills, API keys.
- **Providers** [runtime-provider-management]: runtime detection and model choice.
- **Command palette** (Cmd/Ctrl+K) and **global search** (Cmd/Ctrl+Shift+K).
- **Launch dialog**: effort, fast mode, context limit, permissions per member.
- **Per-member MCP policy** panel: which extra MCP servers an agent may use.
- **Onboarding wizard**: first-run team setup with detected runtimes.
- **Work sync panel**: per-member diagnostics (see below).
- **Settings**: General, Advanced, Connection (SSH), Claude root, Workspace profiles,
  Notifications and triggers, CLI runtime, config editor, Permissions / tool-approval rules,
  Recovery, Queue, Terminal. 29 UI languages (`rteams-services/src/locales`).
- **Update and news** banners/dialogs. Sponsor and web-preview banners are intentionally absent.

## How agents run

Each agent is a one-shot CLI run wired to `rteams serve`. The GUI wakes an agent only when the
log gives it work (message, assignment, review request, changes requested, comment, unblock),
debounces 2 s, rate-limits (20 s gap, 6/hour), caps concurrency at 3, and resumes the member's
session. Auto-resume after rate limits parses the reset time. Solo mode, per-member worktrees,
post-crash recovery and the tmux installer are included. Runtimes: Claude, Codex, OpenCode,
Cursor, Gemini, Qwen, Copilot, Grok, Kiro (see PARITY.md for depth).

### Work sync [member work sync]

Every ~60 s the shell asks `worksync::WorkSync::nudges`: a member with actionable work whose
agenda fingerprint (task id, status, reason) stayed unhandled for 10 minutes and who is idle and
was not seen running since the change gets one wake. At most one nudge per hour and three per
fingerprint; a user can stop auto-resume per member. Agents can call `member_work_sync_status`
and `member_work_sync_ack` (see tools) to read their agenda state or confirm they have seen it.
Records: `<home>/<team>/work-sync/<member>.json`.

## Agent tools (MCP, `rteams serve`)

`tools/list` is the source of truth: **76 tools**, covering all 51 of the original app's MCP tools
under the same names (see PARITY.md for the mapping). Write tools take `actor`; read tools take
`member`; writes accept `command_id` for idempotency.

- **Team**: `team_create`, `team_get`, `team_list`, `team_configure`, `team_update`, `team_launch`,
  `team_stop`, `team_trash`, `team_delete` (lead), `team_get_launch_state`, `member_add`,
  `member_update`, `member_remove`, `member_briefing`, `lead_briefing`, `task_briefing`,
  `app_get_connection_info`.
- **Tasks**: `task_create`, `task_create_from_message`, `task_get`, `task_list`, `task_update`,
  `task_start`, `task_complete`, `task_set_status`, `task_set_owner`, `task_set_dependencies`,
  `task_link`, `task_unlink`, `task_block`, `task_unblock`, `task_archive`, `task_restore`,
  `task_set_clarification`, `task_attach_file`, `task_add_comment`, `task_get_comment`,
  `task_comment_files`, `task_attach_comment_file`.
- **Kanban / review**: `kanban_get`, `kanban_set_order`, `kanban_set_column`, `kanban_clear`,
  `kanban_add_reviewer`, `kanban_remove_reviewer`, `kanban_list_reviewers`, `review_request`,
  `review_start`, `review_approve`, `review_request_changes`.
- **Messages**: `message_send` (`from_team`, `chat`, `attachments`), `message_react`, `inbox_get`,
  `group_chat_create` / `group_chat_update` / `group_chat_delete` / `group_chat_list`,
  `cross_team_list_targets`, `cross_team_send`, `cross_team_get_outbox`.
- **Processes**: `process_register`, `process_list`, `process_stop`, `process_unregister`.
- **Runtime**: `runtime_heartbeat`, `runtime_bootstrap_checkin`, `runtime_deliver`,
  `runtime_deliver_message`, `runtime_task_event`.
- **Work sync**: `member_work_sync_status`, `member_work_sync_report` (token-free),
  `member_work_sync_ack`.
- **Schedules / permissions**: `schedule_set`, `schedule_remove`, `permission_request`,
  `permission_decide`.

## Settings and configuration

- Server: `--home` / `RTEAMS_HOME`, `--team` / `RTEAMS_TEAM`, `RTEAMS_MEMBER`,
  `RTEAMS_PERMISSION_TIMEOUT_MS`.
- GUI: `--home`, `--team`, `--as` (env `RTEAMS_ACTOR`).
- Per-member: runtime, model, effort (low/medium/high), fast mode, context limit, worktree,
  extra MCP servers.
