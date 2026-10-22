# Parity: `team:*` channels (group A)

Counts: Done 32, N/A 14, Implemented 4, Left 1 (total 51).

| Channel | What it does | rteams equivalent (file::fn) | Status |
|---|---|---|---|
| `team:addTaskRelationship` | Link two tasks (related) | task_dialog.rs::related section -> GuiCmd::LinkTasks{linked:true}; core Command::LinkTasks | Done |
| `team:removeTaskRelationship` | Unlink two tasks | task_dialog.rs -> GuiCmd::LinkTasks{linked:false} | Done |
| `team:aliveList` | List teams with a live lead process | launcher.rs agent status; teams.rs cards | Done |
| `team:processAlive` | Is a team's lead process alive | launcher.rs::AgentStatus (per-member running state in team.rs::members) | Done |
| `team:processSend` | Send a message to the team's lead process | actions.rs::GuiCmd::Message / messages.rs composer | Done |
| `team:forceStop` | Force-stop a team | view.rs Intent::StopAll / launcher.rs::kill_all_agents (process-group kill) | Done |
| `team:cancelProvisioning` | Cancel an in-progress provisioning run | No provisioning phase: agents are launched directly and stopped with Intent::Stop | N/A |
| `team:prepareProvisioning` | Pre-flight CLI/auth checks before provisioning | launcher.rs spawn + diagnostics.rs::classify cover failures after launch; no separate prepare step | N/A |
| `team:provisioningStatus` | Poll provisioning run status | launcher.rs AgentView status | N/A |
| `team:provisioningProgress` | Provisioning progress event | immediate-mode GUI repaints from launcher state | N/A |
| `team:createConfig` | Create a team config (draft) | actions.rs::GuiCmd::CreateTeam / core Command::CreateTeam | Done |
| `team:updateConfig` | Edit team name/path/prompt | actions.rs::GuiCmd::ConfigureTeam, RenameTeam | Done |
| `team:deleteDraft` | Delete a team dir that has no config | rteams-store file.rs::FileStore::delete_team_permanently (allows a log with no TeamCreated); teams.rs delete-forever | Implemented |
| `team:deleteTeam` | Soft-delete a team (trash) | teams.rs::card_actions -> GuiCmd::DeleteTeam{deleted:true} | Done |
| `team:restoreTeam` | Restore a trashed team | teams.rs::card_actions -> GuiCmd::DeleteTeam{deleted:false} | Done |
| `team:permanentlyDeleteTeam` | Delete a trashed team from disk | rteams-store FileStore::delete_team_permanently; teams.rs Teams::take_purge_request; app.rs handler (only trashed or draft teams, confirmed) | Implemented |
| `team:getSavedRequest` | Read the saved launch request to relaunch a team | launcher specs come from the roster (Member runtime/model fields) every launch; nothing separate is saved | N/A |
| `team:softDeleteTask` | Soft-delete a task | board.rs/task_dialog.rs -> GuiCmd::Archive{archived:true} | Done |
| `team:restoreTask` | Restore a soft-deleted task | board.rs trash view (~line 395) / task_dialog.rs -> GuiCmd::Archive{archived:false} | Done |
| `team:getDeletedTasks` | List deleted tasks | board.rs trash view (archived tasks, newest first) | Done |
| `team:discardQueuedUserMessages` | Drop messages queued for the lead | ui/queue_list.rs::QueueList::show (cancel/delete per item) | Done |
| `team:getQueuedUserMessages` | List queued user messages | ui/queue_list.rs::QueueList::show | Done |
| `team:getMessagesPage` | Page through message history | messages.rs renders the in-memory log; no paging needed | N/A |
| `team:getMemberLogs` | Find a member's session logs | logs.rs::Logs::load_history / show (runs/<member>/) | Done |
| `team:getTaskActivity` | Task activity feed | task_dialog.rs history + task_logs.rs::entries | Done |
| `team:getTaskActivityDetail` | One activity entry detail | task_logs.rs::Entry in logs.rs::task_log_section | Done |
| `team:getTaskExactLogSummaries` | Exact log summaries per task | task_logs.rs::Attributor / logs.rs::task_log_section | Done |
| `team:getTaskExactLogDetail` | Exact log detail | task_logs.rs::entries / logs.rs::task_log_section | Done |
| `team:getTaskLogStreamSummary` | Live log stream summary | task_logs.rs::ingest (live) / logs.rs::task_log_section | Done |
| `team:getTaskChangePresence` | Which tasks have file changes | task_dialog.rs (runs::changed_files per task) | Done |
| `team:setChangePresenceTracking` | Toggle main-process change tracking | Electron polling toggle; GUI computes on demand | N/A |
| `team:setTaskLogStreamTracking` | Toggle log stream tracking | Electron event-subscription toggle | N/A |
| `team:setToolActivityTracking` | Toggle tool activity tracking | Electron event-subscription toggle | N/A |
| `team:setProjectBranchTracking` | Toggle branch polling | Electron polling toggle; app.rs refreshes the branch on git actions and settings open | N/A |
| `team:getProjectBranch` | Current git branch of the project | git.rs::current_branch; app.rs shows it (self.branch); team.rs git readiness | Done |
| `team:projectBranchChange` | Branch-changed event | app.rs::sync_repo refresh | N/A |
| `team:killProcess` | Kill a registered process | actions.rs::GuiCmd::KillProcess + kill_pid (only registered running pids), team.rs::processes kill button | Implemented |
| `team:launchFailureDiagnostics` | Diagnose a failed launch | diagnostics.rs::diagnose_exit / classify; team.rs::diagnose | Done |
| `team:memberSpawnStatuses` | Per-member spawn status | diagnostics.rs::SpawnState; team.rs::diagnose | Done |
| `team:leadActivity` | Lead active/idle state | core MemberHeartbeat status; team.rs member row | Done |
| `team:leadContext` | Lead context-window usage | `ctx 84k / 200k` meter on running member rows, warning above 80%; usage parsed from Claude/Codex streams | Implemented: `context_usage.rs::meter`, `launcher.rs::Supervisor::launch` |
| `team:getMemberActivityMeta` | Per-member last activity | core HeartbeatInfo; team.rs::diagnose/members row | Done |
| `team:replaceMembers` | Replace the whole roster in one call | member_dialog.rs add/edit/remove (UpdateMember, RemoveMember{removed}); import_team draft for bulk create | Done |
| `team:updateMemberSettings` | Member effort/fast/context | member_dialog.rs -> GuiCmd::UpdateMember | Done |
| `team:updateKanban` | Move cards / review state / order | board.rs -> GuiCmd::SetColumn / SetColumnOrder / review commands | Done |
| `team:showMessageNotification` | Desktop notification for a message | notify.rs::notify / notifications() | Done |
| `team:toolApprovalRespond` | Allow/deny a tool request | approvals.rs::Approvals::request -> GuiCmd::DecidePermission | Done |
| `team:toolApprovalSettings` | Auto-allow rules (all / file edits / safe bash) | approvals.rs::AutoAllow, Approvals::auto_decide + settings (per-team approval_settings.json) | Implemented |
| `team:toolApprovalEvent` | New-approval event | approvals.rs::auto_open + notify.rs::approval_notifications | N/A |
| `team:getOpenCodeRuntimeDeliveryStatus` | OpenCode runtime message delivery | OpenCode-only runtime plumbing | N/A |
| `team:retryFailedOpenCodeSecondaryLanes` | Retry OpenCode secondary lanes | OpenCode-only runtime plumbing | N/A |

Notes:
- Implemented here: permanent delete of a trashed team / draft (store + Teams list), kill a registered process, per-team tool-approval auto-allow rules. Each has tests (store `permanent_delete_only_from_trash_or_draft`, gui `kill_process_signals_only_registered_running_pids`, `auto_allow_*`, teams purge request).
- Auto-allow rules apply while the app window is open on that team; the original runs them in its main process.
- Left: lead context usage (needs per-runtime token usage parsing in launcher.rs).
