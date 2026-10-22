# Parity: review, editor, skills, schedule, organizations, notifications, MCP registry

Group B of the REA channel audit (46 channels, original `index.cjs` preload vs rteams).
The GUI is egui: no IPC, so "equivalent" means a screen/action/function with the same
user-facing result. Status: Done = already existed, Implemented = added in this pass,
N/A = Electron plumbing an immediate-mode GUI does not need, Left = not done (with why).

## review

| Channel | What it does | rteams equivalent | Status |
|---|---|---|---|
| review:getTaskChanges | Files and hunks a task changed | `git.rs::file_hunks` / `parse_diff`, loaded by `ui/review/mod.rs::ReviewDialog::load` | Done |
| review:getAgentChanges | Per-agent change list | `changes.rs::agent_changes` / `member_paths` (all files the member edited across their task segments in `runs/<member>`, diffed vs HEAD on a worker `Job`); "Changes" tab in `member_dialog.rs::changes_tab`, rows and "Review diff" open `ReviewDialog` | Implemented (aggregates from run logs like the original, not from task records) |
| review:getChangeStats | Added/removed totals | `ui/review/mod.rs::ReviewDialog::totals` (footer "+A -R across N files"); per file in `tree` | Implemented |
| review:getTeamTaskChangeSummaries | Per-task change badges for the whole team | `changes.rs::Cache::refresh` / `ChangeSummaries::poll` (worker thread, cached by HEAD + touched-file mtime/size); badge "N files +A -R" via `board.rs::change_badge`, click opens the task | Implemented |
| review:invalidateTaskChangeSummaries | Drop the summary cache | `changes.rs::fingerprint` invalidates automatically when HEAD or a touched file changes | Implemented |
| review:checkConflict | Is the file changed since load | `ui/review/conflict.rs::changed_since`, `stale_decisions`, `ReviewDialog::file_changed` | Done |
| review:loadDecisionConflictCandidates | Saved decisions that no longer match | `ui/review/conflict.rs::stale_decisions` / `stale_ids` (conflict dialog on load) | Done |
| review:loadDraftHistoryConflictCandidates | History entries that conflict with disk | `ui/review/history.rs::restore` returns `RestoreError::Changed`; dialog `conflict_dialog` | Done |
| review:rejectHunks | Reverse selected hunks | `git.rs::reject_hunk`, `preview_reject`, `ReviewDialog::reject_one` / `reject_file` | Done |
| review:executeMutation | Run a reject/edit with recovery | `ReviewDialog::do_reject`, `save_edit`, `finish_mutation` (history `record` first) | Done |
| review:retryMutationRecovery | Retry a failed mutation | `history::restore(..., force)` via "Restore" in `history_ui` | Done |
| review:saveDraftHistoryEntry | Save pre-change content for undo | `ui/review/history.rs::record` | Done |
| review:restoreHistory | Restore a history entry | `ui/review/history.rs::restore`, `ReviewDialog::after_restore` | Done |
| review:saveDecisions / loadDecisions / clearDecisions | Persist accept/reject/comments | `ui/review/store.rs::ReviewState::save` / `load`; clear = `Act::Refresh` after accept-all/reject-all | Done |
| review:applyDecisions | Write rejected hunks to disk | Applied immediately on reject (`reject_one`); `Act::RejectAll` for bulk | Done |
| review:watchFiles / unwatchFiles | Watch files under review | Immediate-mode repaint plus content-hash check each load (`conflict::changed_since`) | N/A |
| review:cmdN | Cmd+N shortcut forwarded to renderer | Keyboard handled in egui (`ReviewDialog::show`: J/K/A/R) | N/A |

## editor

| Channel | What it does | rteams equivalent | Status |
|---|---|---|---|
| editor:writeFile | Save with hash guard | `rteams-services/src/editor/files.rs::write_text` (expected hash) + `editor/conflict.rs` | Done |
| editor:renameFile | Rename path | `editor/files.rs::rename` / `move_to`, `ui/editor/mod.rs` `Dialog::rename` | Done |
| editor:deleteFile | Delete/trash | `editor/files.rs::trash_path` / `delete_path`, `DialogResult::Trash` | Done |
| editor:readBinaryPreview | Image/binary preview | `ui/editor/image_preview.rs::show` / `show_document` (images, sheets, docs) and `files::looks_binary` | Done |
| editor:setWatchedDirs / setWatchedFiles | Register fs watches | `editor/watch.rs::FsWatcher::watch_dir`, `tree.rs` reload | N/A (watcher is wired; no registration API needed) |

## skills

| Channel | What it does | rteams equivalent | Status |
|---|---|---|---|
| skills:previewImport / applyImport | Import folder/zip/git, show plan, apply | `ext_skills/import.rs::prepare`, `plan_import`; `Plan::apply`; `ui/ext/skill_import.rs`, `skill_review.rs` | Done |
| skills:previewUpsert / applyUpsert | Create/edit skill with plan preview | `ext_skills/scaffold.rs::plan_create_with`, `plan.rs::Plan::build`; `ui/ext/skill_editor.rs` | Done |
| skills:startWatching / stopWatching | Watch skill roots | `ext_skills/watch.rs::SkillWatcher::start`, held in `skills_panel.rs` | Done |

## schedule

| Channel | What it does | rteams equivalent | Status |
|---|---|---|---|
| schedule:get | Schedules + next runs | `schedule/store.rs::load`, `runner.rs::next_run`, `ui/schedules/list.rs` | Done |
| schedule:getRunLogs | Run history and output | `schedule/history.rs::read`, `ui/schedules/run_log.rs::RunLog` | Done |
| schedule:triggerNow | Run a schedule now | `ui/schedules/list.rs::ListAction::RunNow`, handled in `ui/schedules/mod.rs` | Done |

## organizations

| Channel | What it does | rteams equivalent | Status |
|---|---|---|---|
| organizations:createOrganization | New org | `orgs/structure.rs::create_org` / `Edit::CreateOrg`; `ui/orgs/editor_panel.rs` | Done |
| organizations:upsertUnit / moveUnit / removeUnit | Place, move, remove a team in org/group | `Edit::PlaceTeam`, `RemoveTeam`, `CreateGroup`/`RenameGroup`/`DeleteGroup`, `SetParent`; drag in `ui/orgs/mod.rs` | Done |
| organizations:upsertRelation / deleteRelation | Add/remove relation | `orgs/structure.rs::add_relation` / `remove_relation`; `editor_panel.rs` "manualRelation" | Done |

## notifications

| Channel | What it does | rteams equivalent | Status |
|---|---|---|---|
| notifications:get | Notification list | `notifications.rs::NotificationCenter` (dropdown `show`) | Done |
| notifications:testNotification | Send a test notification | `NotificationCenter::record_test` + button in `prefs_ui` (Settings > Notifications) | Implemented |
| notifications:setViewedTeam | Tell main which team is on screen (suppress its toasts) | One team is open at a time; `NotificationCenter::load(home, team)` rebinds on team switch and `app.rs::tick_notifications` only reads the open team | N/A |

## mcpRegistry, todo-change

| Channel | What it does | rteams equivalent | Status |
|---|---|---|---|
| mcpRegistry:installCustom | Install a user-defined MCP server | `ui/ext/custom_mcp_dialog.rs::CustomMcpDialog::build`, `ext/mcp_install.rs::install` | Done |
| todo-change | File-watch event for todos | rteams has no todo files; tasks come from the event log and the GUI re-reads it each frame | N/A |

## Counts

Done 34, Implemented 5 (review totals, notification test, agent changes, task change summaries, cache invalidation), N/A 7, Left 0. Total 46.
