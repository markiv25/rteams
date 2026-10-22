# Parity: sessions, config, usage budget, trust, context files

Original = Agent Teams AI (Electron, IPC channels). rteams = egui, so an IPC channel is mapped to the screen/function that gives the same user-visible behaviour.

Status: Done (already existed), Implemented (added in this pass), N/A (Electron or Windows/WSL plumbing), Left.

| Channel | What it does | rteams equivalent | Status |
|---|---|---|---|
| config:get | Return the whole app config | `services/config.rs::AppConfig::load` | Done |
| config:hideSession / unhideSession | Hide or unhide one session | `ui/sessions/mod.rs::SessionsScreen::toggle_list` (persists `hidden_sessions`), `scanner::order_for_display` | Done |
| config:hideSessions / unhideSessions | Bulk hide / unhide | `ui/sessions/mod.rs::set_hidden_bulk` + "Hide all / Unhide all" in `session_list.rs` (`ListAction::SetHiddenAll`); test `bulk_hide_and_unhide_persist` | Implemented |
| config:pinSession / unpinSession | Pin or unpin a session | `ui/sessions/mod.rs::toggle_list` (`pinned_sessions`, pinned first) | Done |
| config:addIgnoreRepository / removeIgnoreRepository | Repos that never raise notifications | `ui/settings/notifications.rs` (ignored repos list, `Notifications::ignored_repos`) | Done |
| config:clearSnooze | Cancel notification snooze | `ui/settings/notifications.rs` (snooze, `claude_root::next_local_midnight`, `Notifications::snooze_until`) | Done |
| config:updateTrigger / removeTrigger | Edit or delete a trigger rule | `ui/trigger_settings.rs::show` / `rule_card`; `sessions/triggers.rs::{load_rules,save_rules,validate}` | Done |
| config:getClaudeRootInfo | Resolved Claude root and its source | `services/claude_root.rs::{resolve,validate}`, `ui/settings/local_claude_root.rs::show` | Done |
| config:selectFolders | Native folder picker | `local_claude_root.rs` path input with validation and confirmation (native picker is Electron dialog plumbing) | N/A |
| config:openInEditor | Open a file in the external editor | Built-in editor screen (`ui/editor/**`) | Done |
| config:findWslClaudeRoots | Find Claude roots in WSL distros | Windows/WSL only; rteams targets macOS/Linux | N/A |
| get-repository-groups | Sessions grouped per repository | `services/sessions/worktrees.rs::group_by_repo`, `ui/sessions/project_list.rs` | Done |
| get-worktree-sessions | Sessions of all worktrees of a repo | `sessions/worktrees.rs::{repo_of,RepoCache}` (worktrees fold into the repo group) | Done |
| get-sessions-paginated / get-sessions-by-ids | Session lists | `sessions/scanner.rs` + `metadata_index.rs`; virtualized list `session_list.rs::show` (show_rows) | Done |
| get-session-groups | Date grouping | `ui/sessions/project_list.rs`, `scanner::order_for_display` | Done |
| get-session-detail | Full parsed session | `sessions/chunks.rs::Session::from_msgs`, `ui/sessions/viewer.rs` | Done |
| get-session-metrics | Tokens, cost, durations | `sessions/report.rs`, `ui/sessions/report_tab.rs` (Session report tab) | Done |
| get-subagent-detail | Subagent transcript | `sessions/subagents.rs::{load_all,link}`, subagent cards in `viewer.rs` | Done |
| get-waterfall-data | Timing waterfall | `ui/sessions/waterfall.rs::show` (Waterfall tab) | Done |
| token-usage:get-budget-settings / update-budget-settings | Read and save budgets | `services/usage/budget.rs::{load,save}`, `ui/usage/budget_dialog.rs` | Done |
| token-usage:get-budget-status / budget-status-changed | Budget status and change events | `budget.rs::{build_status,crossings,budget_alerts}`, status shown in `ui/usage/mod.rs` and turned into toasts | Done |
| token-usage:refresh-snapshot / snapshot-changed | Rescan usage and push the new snapshot | `ui/usage/mod.rs` refresh (stale refresh, off-thread scan), `usage/state.rs` | Done |
| workspace-trust:get-project-status | Is a project trusted | `services/trust.rs::is_claude_trusted_at`; shown in new Context tab (`ui/sessions/context_tab.rs`) | Implemented |
| workspace-trust:get-launch-status | Trust state at launch | `trust.rs::{ensure_claude_trusted,codex_trust_args,launch_notice}` (rteams grants trust at launch) | Done |
| validate-mentions | Check `@path` mentions exist inside the project | `sessions/context_files.rs::{extract_mentions,validate_mentions}`; Context tab shows each mention valid/missing. Composer untouched (rteams composer `@` is team members). Test `mentions_extract_validate_read` | Implemented |
| read-mentioned-file | Size/token-check a mentioned file (25k cap, project-contained) | `context_files.rs::read_mentioned_file` (service + test; not yet shown in UI beyond validity) | Implemented |
| read-claude-md-files | CLAUDE.md, rules, local, auto-memory sizes | `context_files.rs::claude_md_files`, Context tab "Memory files" (token estimate per source); test `claude_md_and_rules_and_agents` | Implemented |
| read-agent-configs | `.claude/agents/*.md` names/colours | `context_files.rs::read_agent_configs`, Context tab "Project agents" | Implemented |

New tab: Sessions viewer "Context" (`ViewerTab::Context`); its strings fall back to English defaults, no locale keys added. Test `context_tab::tests::builds_from_session_cwd_and_prompts`.

Left: none. Notes: original `read-claude-md-files` only returns sizes, as here; the file bodies are not displayed.

### Added in integration review (2026-10-20)

| Channel | What it does | rteams equivalent (file::fn) | Status |
|---|---|---|---|
| config:unpinSession | unpin a session | `gui/ui/sessions/session_list.rs` pin toggle, `ui/sessions/mod.rs` | Done |
| config:unhideSession | unhide one session | `gui/ui/sessions/session_list.rs` hide toggle | Done |
| config:unhideSessions | unhide many | `gui/ui/sessions/mod.rs` `ListAction::SetHiddenAll` (Unhide all) | Done |
| config:removeTrigger | delete a notification trigger | `gui/ui/trigger_settings.rs` delete button | Done |
| config:removeIgnoreRepository | stop ignoring a repo | `gui/ui/settings/notifications.rs` / `trigger_settings.rs` ignored-repo list remove | Done |
| token-usage:update-budget-settings | save budget settings | `gui/ui/usage/budget_dialog.rs`, `services/usage/budget.rs` | Done |
| token-usage:budget-status-changed | push event | N/A: immediate-mode UI recomputes budget status each frame | N/A |
| token-usage:snapshot-changed | push event | N/A: usage screen re-reads the snapshot on refresh | N/A |
