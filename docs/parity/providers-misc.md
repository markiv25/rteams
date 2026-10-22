# Parity: providers, installers, announcements, misc (group D, 44 channels)

Status: Done (already existed under another name), Implemented (added in this pass), N/A (reason given), Left.

| Channel | What it does | rteams equivalent | Status |
|---|---|---|---|
| announcements:cancelAsset | Abort an in-flight asset download | services/announcements/assets.rs::ensure_cover (small bounded fetch on a worker; results are dropped if the dialog closes) | N/A (nothing long-running to abort) |
| announcements:cancelCover | Abort a cover download | same as above | N/A |
| announcements:loadCover | Fetch and cache a cover image | announcements/assets.rs::ensure_cover / cached_cover / sync_covers; ui/news_dialog.rs | Done |
| announcements:openManual | Open an article on demand | ui/news_dialog.rs::open_article, news_button.rs::show | Done |
| announcements:prepareAuto | Pick one unseen item to auto-show once | announcements/mod.rs::claim_auto, policy.rs::select_auto | Done |
| announcements:stateChanged | Push unread/state changes to UI | news_button.rs::tick/unread (polled each frame) | Done |
| apiKeys:storageStatus | Show where keys are stored | services/ext/apikeys.rs::backend_label, shown in ui/ext/apikeys_panel.rs | Done |
| app:getWindowsElevationStatus | Windows admin check | none | N/A (Windows elevation; rteams never elevates, tmux installer closes stdin) |
| app:openCodeStartupCleanupStatus | OpenCode startup cleanup state | none | N/A (OpenCode-only runtime internals) |
| app:retryOpenCodeStartupCleanup | Retry that cleanup | none | N/A (OpenCode-only) |
| cliInstaller:getProviderStatus | Detect one provider (installed, version, auth, models) | providers.rs::probe_all / Providers::refresh, runtimes/mod.rs detection | Done |
| cliInstaller:invalidateStatus | Drop cached provider status | providers.rs::Providers::refresh (Refresh button) | Done |
| cliInstaller:verifyProviderModels | Check that the provider's models respond | providers.rs::verify_summary + CardAction::VerifyAll ("Verify all models" button, reuses start_test) | Implemented |
| discord:getMemberCount | Discord widget member count | none | N/A (Discord widget, no equivalent surface) |
| member-log-stream:getMemberLogPreviews | Last few activity lines of a member | member_preview.rs::latest_activity, shown in member_dialog.rs stats tab (also ui/log_stream.rs for the merged stream) | Implemented |
| member-log-stream:getMemberRuntimeLogTail | Raw runtime log tail | member_preview.rs::runtime_tail, collapsible "Runtime log tail" in the member dialog | Implemented |
| member-work-sync:refreshStatus | Recompute work-sync status | ui/work_sync_panel.rs::show recomputes worksync diagnostics every frame (always live, no cache to refresh) | Done |
| openCodeRuntime:invalidateStatus | Reset OpenCode runtime status cache | none | N/A (OpenCode-only; generic detection is re-run by Providers::refresh) |
| runtimeProviderManagement:cancelModelLoad | Cancel a model list load | providers.rs local scan runs on a worker with short timeouts | N/A (no long model-load to cancel) |
| runtimeProviderManagement:clearProjectDefault | Clear OpenCode per-project default model | none | N/A (OpenCode project config only; rteams keeps model per member, cleared by choosing the default in the member editor) |
| runtimeProviderManagement:companionAction | Companion (OpenCode helper) action | none | N/A (OpenCode companion) |
| runtimeProviderManagement:companionConnect | Connect companion | none | N/A |
| runtimeProviderManagement:companionInstall | Install companion | none | N/A |
| runtimeProviderManagement:companionProgress | Companion progress | none | N/A |
| runtimeProviderManagement:companionStatus | Companion status | none | N/A |
| runtimeProviderManagement:localProviderConfigure | Configure Ollama/LM Studio | provider_cfg.rs (scan, limits override) | Done |
| runtimeProviderManagement:localProviderList | List local providers | provider_cfg.rs::scan_local, providers.rs local section | Done |
| runtimeProviderManagement:localProviderProbe | Probe one local provider | provider_cfg.rs::scan_one | Done |
| runtimeProviderManagement:oauthCancel | Cancel OAuth flow | none | N/A (sign-in handled by each CLI in the terminal drawer) |
| runtimeProviderManagement:oauthCode | Submit OAuth code | none | N/A (CLI-handled) |
| runtimeProviderManagement:oauthProgress | OAuth progress | providers.rs "awaiting sign-in" state + Refresh | Done |
| runtimeProviderManagement:setupForm | Provider setup form (API key) | providers.rs card key input; provider_cfg.rs::connect_key / forget_key | Done |
| team-import:create-draft | Turn an import preview into a team draft | services/team_import.rs::ImportPreview::into_draft, ui/import_team_dialog.rs | Done |
| telemetry:getSentryContext | Sentry context | none | N/A (Sentry telemetry; rteams sends none) |
| telemetry:getSentryStatus | Sentry status | none | N/A |
| tmux:cancelInstall | Stop a running tmux install | services/tmux/runner.rs::InstallRun::cancel; Cancel button in ui/tmux_banner.rs (cancelled returns to Missing, not Failed) | Implemented |
| tmux:getInstallerSnapshot | Current installer state and log | ui/tmux_banner.rs (Phase + streamed log, drawn live) | Done |
| tmux:invalidateStatus | Re-check tmux | tmux_banner.rs::start_probe (Re-check button) | Done |
| tmux:submitInstallerInput | Send a password to the installer | none by design | N/A (rteams never collects passwords; shows "run in a terminal" hint, tmux::needs_input) |
| window:fullscreen-changed | Fullscreen event | eframe/winit | N/A (native window handling) |
| window:isMaximized | Query maximized | eframe native decorations | N/A |
| window:maximize | Maximize | native title bar buttons (decorations on) | N/A |
| window:minimize | Minimize | native title bar buttons | N/A |
| window:zoom-factor-changed | UI zoom | egui built-in Cmd/Ctrl +, -, 0 (Options::zoom_with_keyboard is on by default and not disabled) | Done |

Counts: Done 17, Implemented 4, N/A 23, Left 0.
