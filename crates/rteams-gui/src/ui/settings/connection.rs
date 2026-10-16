//! Connection (SSH): opt-in remote access through the system `ssh`. Profile CRUD, connection
//! test, and listing the remote `~/.claude/projects`. Password auth is not supported.

use std::path::Path;
use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use rteams_services::ssh::{self, Auth, SshProfile};
use serde_json::json;

use super::{card, extra_bool, hint};
use crate::theme;

pub const ENABLED: &str = "ssh_enabled";

#[derive(Default)]
pub struct State {
    profiles: Option<Vec<SshProfile>>,
    edit: Option<SshProfile>,
    confirm_delete: Option<usize>,
    test_rx: Option<Receiver<Result<(), String>>>,
    test_result: Option<Result<(), String>>,
    remote_rx: Option<Receiver<Result<Vec<String>, String>>>,
    remote: Option<(String, Result<Vec<String>, String>)>,
    remote_host: String,
    config_hosts: Option<Vec<String>>,
    error: Option<String>,
}

impl State {
    /// Skip loading `ssh-profiles.json` (tests).
    #[cfg(test)]
    pub fn set_profiles(&mut self, p: Vec<SshProfile>) {
        self.profiles = Some(p);
        self.config_hosts = Some(vec![]);
    }
}

fn new_id() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

pub fn show(
    ui: &mut egui::Ui,
    cfg: &mut AppConfig,
    st: &mut State,
    tr: &I18n,
    home: &Path,
) -> bool {
    let mut changed = false;
    if st.profiles.is_none() {
        st.profiles = Some(ssh::load_profiles(home));
    }
    if st.config_hosts.is_none() {
        st.config_hosts = Some(
            rteams_services::claude_root::user_home()
                .and_then(|h| std::fs::read_to_string(h.join(".ssh/config")).ok())
                .map(|t: String| ssh::parse_config_hosts(&t))
                .unwrap_or_default(),
        );
    }
    if let Some(rx) = &st.test_rx {
        if let Ok(r) = rx.try_recv() {
            st.test_result = Some(r);
            st.test_rx = None;
        }
    }
    if let Some(rx) = &st.remote_rx {
        if let Ok(r) = rx.try_recv() {
            st.remote = Some((st.remote_host.clone(), r));
            st.remote_rx = None;
        }
    }
    if st.test_rx.is_some() || st.remote_rx.is_some() {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(150));
    }

    card(ui, &tr.t("settings.connection.title"), |ui| {
        hint(ui, &tr.t("settings.connection.description"));
        let mut on = extra_bool(&cfg.extra, ENABLED, false);
        if ui
            .checkbox(&mut on, "Enable remote connections (uses the system ssh)")
            .changed()
        {
            cfg.extra.insert(ENABLED.into(), json!(on));
            changed = true;
        }
        hint(
            ui,
            "Password authentication is not supported; use an SSH agent or a private key.",
        );
        ui.label(
            tr.t_args(
                "settings.connection.currentMode.local",
                &[(
                    "path",
                    &super::local_claude_root::effective(
                        cfg,
                        None,
                        &rteams_services::claude_root::user_home().unwrap_or_default(),
                    )
                    .path
                    .display()
                    .to_string(),
                )],
            ),
        );
    });
    if !extra_bool(&cfg.extra, ENABLED, false) {
        return changed;
    }

    let mut profiles = st.profiles.clone().unwrap_or_default();
    let mut dirty = false;
    card(ui, &tr.t("settings.connection.savedProfiles.title"), |ui| {
        if profiles.is_empty() {
            ui.label(tr.t("settings.workspaceProfiles.empty.title"));
            hint(ui, &tr.t("settings.workspaceProfiles.empty.description"));
        }
        let mut test: Option<SshProfile> = None;
        let mut connect: Option<SshProfile> = None;
        let mut edit: Option<SshProfile> = None;
        for (i, p) in profiles.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&p.name).strong());
                hint(ui, &format!("{}:{}", p.target(), p.port));
                if ui
                    .small_button(tr.t("settings.connection.actions.testConnection"))
                    .clicked()
                {
                    test = Some(p.clone());
                }
                if ui
                    .small_button(tr.t("settings.connection.actions.connect"))
                    .clicked()
                {
                    connect = Some(p.clone());
                }
                if ui
                    .small_button(tr.t("settings.workspaceProfiles.actions.editProfile"))
                    .clicked()
                {
                    edit = Some(p.clone());
                }
                if ui
                    .small_button(tr.t("settings.workspaceProfiles.actions.deleteProfile"))
                    .clicked()
                {
                    st.confirm_delete = Some(i);
                }
            });
        }
        if let Some(p) = test {
            let (tx, rx) = mpsc::channel();
            st.test_rx = Some(rx);
            st.test_result = None;
            std::thread::spawn(move || {
                let _ = tx.send(ssh::test_connection(&p));
            });
        }
        if let Some(p) = connect {
            let (tx, rx) = mpsc::channel();
            st.remote_rx = Some(rx);
            st.remote_host = p.name.clone();
            st.remote = None;
            std::thread::spawn(move || {
                let _ = tx.send(ssh::list_remote_dir(&p, "~/.claude/projects"));
            });
        }
        if edit.is_some() {
            st.edit = edit;
        }
        if let Some(i) = st.confirm_delete {
            match profiles.get(i).map(|p| p.name.clone()) {
                Some(name) => {
                    ui.colored_label(
                        theme::WARNING,
                        tr.t_args(
                            "settings.workspaceProfiles.deleteConfirm.message",
                            &[("name", &name)],
                        ),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .button(tr.t("settings.workspaceProfiles.deleteConfirm.confirmLabel"))
                            .clicked()
                        {
                            profiles.remove(i);
                            dirty = true;
                            st.confirm_delete = None;
                        }
                        if ui
                            .button(tr.t("settings.workspaceProfiles.actions.cancel"))
                            .clicked()
                        {
                            st.confirm_delete = None;
                        }
                    });
                }
                None => st.confirm_delete = None,
            }
        }
        if st.edit.is_none()
            && ui
                .button(tr.t("settings.workspaceProfiles.actions.addProfile"))
                .clicked()
        {
            st.edit = Some(SshProfile {
                id: new_id(),
                ..Default::default()
            });
        }

        if st.test_rx.is_some() {
            ui.label(tr.t("settings.connection.actions.testing"));
        }
        match &st.test_result {
            Some(Ok(())) => {
                ui.colored_label(theme::SUCCESS, tr.t("settings.connection.test.success"));
            }
            Some(Err(e)) => {
                let e = if e.is_empty() {
                    tr.t("settings.connection.test.unknownError")
                } else {
                    e.clone()
                };
                ui.colored_label(
                    theme::ERROR,
                    tr.t_args("settings.connection.test.failed", &[("error", &e)]),
                );
            }
            None => {}
        }
        if st.remote_rx.is_some() {
            ui.label(tr.t("settings.connection.actions.connecting"));
        }
        match &st.remote {
            Some((host, Ok(dirs))) => {
                ui.colored_label(
                    theme::SUCCESS,
                    tr.t_args("settings.connection.status.connectedTo", &[("host", host)]),
                );
                hint(ui, &tr.t("settings.connection.status.remoteSessions"));
                for d in dirs.iter().take(100) {
                    ui.monospace(d);
                }
            }
            Some((_, Err(e))) => {
                ui.colored_label(
                    theme::ERROR,
                    tr.t_args("settings.connection.test.failed", &[("error", e)]),
                );
            }
            None => {}
        }

        let mut done = false;
        if let Some(p) = st.edit.as_mut() {
            ui.separator();
            ui.label(tr.t("settings.workspaceProfiles.form.name"));
            ui.add(
                egui::TextEdit::singleline(&mut p.name)
                    .hint_text(tr.t("settings.workspaceProfiles.form.namePlaceholder")),
            );
            ui.label(tr.t("settings.connection.form.host"));
            ui.add(
                egui::TextEdit::singleline(&mut p.host)
                    .hint_text(tr.t("settings.connection.form.hostPlaceholder")),
            );
            if let Some(hosts) = st.config_hosts.as_ref().filter(|h| !h.is_empty()) {
                egui::ComboBox::from_id_salt("ssh-config-hosts")
                    .selected_text(tr.t("settings.workspaceProfiles.authMethods.auto"))
                    .show_ui(ui, |ui| {
                        for h in hosts {
                            if ui.selectable_label(p.host == *h, h).clicked() {
                                p.host = h.clone();
                            }
                        }
                    });
            }
            ui.horizontal(|ui| {
                ui.label(tr.t("settings.connection.form.port"));
                ui.add(egui::DragValue::new(&mut p.port).range(1..=65535));
                ui.label(tr.t("settings.connection.form.username"));
                ui.add(
                    egui::TextEdit::singleline(&mut p.user)
                        .desired_width(120.0)
                        .hint_text(tr.t("settings.connection.form.usernamePlaceholder")),
                );
            });
            ui.horizontal(|ui| {
                ui.label(tr.t("settings.connection.form.authentication"));
                for (a, k) in [
                    (Auth::Agent, "agent"),
                    (Auth::Auto, "auto"),
                    (Auth::PrivateKey, "privateKey"),
                    (Auth::Password, "password"),
                ] {
                    ui.selectable_value(
                        &mut p.auth,
                        a,
                        tr.t(&format!("settings.workspaceProfiles.authMethods.{k}")),
                    );
                }
            });
            if p.auth == Auth::PrivateKey {
                ui.horizontal(|ui| {
                    ui.label(tr.t("settings.connection.form.privateKeyPath"));
                    ui.text_edit_singleline(&mut p.key_path);
                    if ui.small_button("...").clicked() {
                        if let Some(f) = rfd::FileDialog::new().pick_file() {
                            p.key_path = f.to_string_lossy().into_owned();
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui
                    .button(tr.t("settings.workspaceProfiles.actions.save"))
                    .clicked()
                {
                    if p.name.trim().is_empty() {
                        p.name = p.host.clone();
                    }
                    match p.validate() {
                        Ok(()) => {
                            match profiles.iter().position(|x| x.id == p.id) {
                                Some(i) => profiles[i] = p.clone(),
                                None => profiles.push(p.clone()),
                            }
                            dirty = true;
                            st.error = None;
                            done = true;
                        }
                        Err(e) => st.error = Some(e),
                    }
                }
                if ui
                    .button(tr.t("settings.workspaceProfiles.actions.cancel"))
                    .clicked()
                {
                    done = true;
                    st.error = None;
                }
            });
            if let Some(e) = &st.error {
                ui.colored_label(theme::ERROR, e);
            }
        }
        if done {
            st.edit = None;
        }
    });
    if dirty {
        match ssh::save_profiles(home, &profiles) {
            Ok(()) => st.profiles = Some(profiles),
            Err(e) => st.error = Some(e.to_string()),
        }
    }
    changed
}
