# Parity with agent-teams-ai

Last reviewed 2026-10-20 by reading the code and running `rteams serve` `tools/list` (not by
exercising every screen). Status: Done = implemented and wired (file given); Partial = exists with
the limits stated; In progress = landing today, see date; Not planned = intentionally skipped.

## MCP tools

The original ships 51 tools (`mcp-server/index.js` in the app bundle). rteams serves **76**
(`tools/list`, defined in `crates/rteams-server/src/tools.rs`, `extra.rs`, and `permission_request`
in `lib.rs`). All 51 original tools exist in rteams under the same name; 25 more are rteams-only.
`runtime_deliver_message` was added on 2026-10-20 next to the older `runtime_deliver`.

| Group | Tools (original = same name in rteams unless noted) |
|---|---|
| Team / lead | `team_create`, `team_get`, `team_list`, `team_update`, `team_launch`, `team_stop`, `team_trash`, `lead_briefing`, `member_briefing`, `task_briefing`, `app_get_connection_info` |
| Tasks | `task_create`, `task_create_from_message`, `task_get`, `task_list`, `task_start`, `task_complete`, `task_set_status`, `task_set_owner`, `task_set_clarification`, `task_restore`, `task_link`, `task_unlink`, `task_add_comment`, `task_get_comment`, `task_attach_file`, `task_attach_comment_file` |
| Kanban / review | `kanban_get`, `kanban_set_column`, `kanban_clear`, `kanban_add_reviewer`, `kanban_remove_reviewer`, `kanban_list_reviewers`, `review_request`, `review_start`, `review_approve`, `review_request_changes` |
| Messaging | `message_send`, `cross_team_list_targets`, `cross_team_send`, `cross_team_get_outbox` |
| Processes | `process_register`, `process_list`, `process_stop`, `process_unregister` |
| Runtime | `runtime_heartbeat`, `runtime_bootstrap_checkin`, `runtime_deliver_message`, `runtime_task_event` |
| Work sync | `member_work_sync_status`, `member_work_sync_report` (token-free, 2026-10-20) |

rteams-only (25; the original does these through renderer IPC, not MCP): `team_configure`,
`team_delete`, `team_get_launch_state`, `member_add`, `member_update`, `member_remove`,
`task_update`, `task_archive`, `task_block`, `task_unblock`,
`task_set_dependencies`, `task_comment_files`, `kanban_set_order`, `message_react`, `inbox_get`,
`group_chat_create`, `group_chat_update`, `group_chat_delete`, `group_chat_list`,
`schedule_set`, `schedule_remove`, `permission_request`, `permission_decide`,
`member_work_sync_ack`, `runtime_deliver`.

Verification: `comm` of the original names against `tools/list` gives 0 missing; the 25 extras are
exactly the names above that are not in the original.

## Feature areas (renderer IPC)

The original's renderer exposes 386 IPC channels. Grouped by namespace:

| Namespace (channels) | rteams module / screen | Status | Notes |
|---|---|---|---|
| `team` (90): data, tasks, kanban, comments, attachments, launch/stop, members | `core`, `store`, `gui/board.rs`, `task_dialog.rs`, `team.rs`, `launcher.rs`, `member_dialog.rs`, `ui/launch_dialog.rs` | Partial | Done: tasks, kanban, comments, attachments (`services/attachments`), launch/stop, restart member, tool approvals (`approvals.rs`), queued messages (`ui/queue_list.rs`), worktrees (`git.rs`), launch args validated against `claude --help`. Missing: launch-failure diagnostics, worktree readiness banner, per-task exact log detail/stream tracking |
| `team` rename | `core` `RenameTeam`/`TeamRenamed`, `server` `team_update {name}`, `gui/teams.rs` (Rename on team card) | Done | display name only; the directory stays the team id |
| `team` member settings (effort, fast, context limit) | `gui/member_dialog.rs` | Done | fields present in code 2026-10-20; unverified by running |
| `team-import` (2) | `ui/import_team_dialog.rs`, `services/team_import.rs` | Done | |
| `crossTeam` (3) | `messages.rs`, server `cross_team_*` | Done | |
| `member-work-sync` (7) | `ui/work_sync_panel.rs`, `services/worksync/` | Partial | status/ack/report and scheduler Done; per-member stop/resume Done; metrics view minimal |
| `review` (34) | `ui/review/` | Partial | hunk accept/reject, per-file, line comments, saved decisions in local file; no conflict-resolution dialogs, no draft-history restore UI |
| `editor` (18) | `ui/editor/`, `services/editor/` | Partial | tree, tabs, find/search, quick open, preview; git status badges only, no stage/commit UI |
| `terminal`, `terminalWorkspace` (8) | `ui/terminal/`, `services/terminal/` | Done | |
| `tmux` (7) | `services/tmux/`, `ui/tmux_banner.rs` | Partial | detect + banner; verify installer input flow |
| `schedule` (11) | `ui/schedules/`, `services/schedule/` | Done | cron editor, run log |
| `notifications`, `notification` (11) | `notifications.rs`, `notify.rs`, `ui/settings/notifications.rs` | Done | includes snooze |
| `config` (26) | `ui/settings/`, `services/config.rs`, `claude_root.rs`, `ui/trigger_settings.rs` | Done | triggers, ignore lists, pins, claude root, WSL roots |
| `sessions` family (`get-sessions*`, `search-*`, `get-waterfall-data`, `get-session-*`, `get-subagent-detail`, `get-projects`, repo groups) | `ui/sessions/`, `services/sessions/`, `services/projects/` | Done | waterfall in `ui/sessions/waterfall.rs` |
| `token-usage` (7) | `ui/usage/`, `services/usage/` | Done | budget dialog included |
| `skills` (10) | `ui/ext/skills_panel.rs`, `skill_editor.rs`, `skill_import.rs`, `services/ext_skills/` | Done | |
| `plugin` (4), `mcpRegistry` (9), `apiKeys` (5) | `ui/ext/`, `services/ext/` | Done | |
| `organizations` (10) | `ui/orgs/`, `services/orgs/` | Done | |
| `httpServer` (3) | `services/http_server/`, started from `gui/app.rs` | Done | |
| `runtimeProviderManagement` (25) | `gui/providers.rs`, `gui/runtimes/` | Partial | Providers screen: Install + per-model Test (2026-10-20). Missing: OAuth flows, companion connect, local-provider scan (Ollama/LM Studio), model-limit config |
| `cliInstaller` (6), `openCodeRuntime` (4) | `gui/installer.rs`, `providers.rs`, `ui/settings/cli_runtime.rs` | Done | in-app installer with live log present in code 2026-10-20; unverified by running |
| `ssh` (9) | `services/ssh.rs`, `ui/settings/connection.rs` | Partial | config + connection settings; remote sessions not verified |
| `workspace-trust`, `project-folder`, `validate-path`, `validate-mentions`, `read-mentioned-file`, `read-*claude-md*`, `read-agent-configs` | `services/trust.rs`, `ui/composer/`, `links.rs` | Done | |
| Composer paste (clipboard image) | `ui/composer/mod.rs` `attach_pasted_image` | Done | present in code 2026-10-20; unverified by running |
| `context` (4), `window` (7), `shell` (3), `app` (4), `renderer` (3), `appStartup`, `file-change`, `todo-change`, `documentPreview` | egui shell, `ui/mod.rs`, `services/docpreview/` | Done | window chrome is native; zoom/fullscreen not applicable |
| `app` OpenCode startup cleanup, relaunch | | Not planned | Electron process management |
| `external-agent-connection` (2) | server `app_get_connection_info` | Done | MCP tool added 2026-10-20 |
| `announcements` (11) | `services/announcements/`, `ui/news_dialog.rs`, `ui/news_button.rs` | Partial | configurable JSON feed, default empty (disabled); no cover/asset caching or auto-claim |
| `updater` (4) | `services/updater.rs`, `ui/update_banner.rs`, `update_dialog.rs` | Partial | passive check, opens release page; never downloads or installs; default feed empty |
| `telemetry` (2: Sentry) | none | Not planned | no telemetry in this fork (no Sentry code found) |
| `discord` (1: member count) | none | Not planned | Electron marketing widget; no code found |
| `app` `getWindowsElevationStatus` | none | Not planned | Windows-only elevation prompt; no code found |

Other gaps (not IPC-driven): pane splitting and tab drag, onboarding free-model start, Z.AI/MiniMax
gateways, codex login flow, per-agent CPU/RAM history. Sponsor / web-preview banners are
intentionally absent. Runtimes: claude, codex, cursor, gemini, opencode, qwen, copilot, grok, kiro.

### Work sync [member work sync]

Every ~60 s the shell asks `worksync::WorkSync::nudges`: a member with actionable work whose
agenda fingerprint (task id, status, reason) stayed unhandled for 10 minutes and who is idle and
was not seen running since the change gets one wake. At most one nudge per hour and three per
fingerprint; a user can stop auto-resume per member. Agents call `member_work_sync_status`,
`member_work_sync_report` (token-free) and `member_work_sync_ack`.
Records: `<home>/<team>/work-sync/<member>.json`.
