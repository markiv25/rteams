//! tmux::strategy: pick a package manager for the platform and build the exact install command.
//! Nothing here runs anything.

use super::platform::{on_path, Os};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Brew,
    Apt,
    Dnf,
    Pacman,
    None,
}

impl Strategy {
    pub fn name(self) -> &'static str {
        match self {
            Strategy::Brew => "brew",
            Strategy::Apt => "apt",
            Strategy::Dnf => "dnf",
            Strategy::Pacman => "pacman",
            Strategy::None => "none",
        }
    }
}

fn any(ids: &[String], names: &[&str]) -> bool {
    ids.iter().any(|i| names.contains(&i.as_str()))
}

/// Strategy for `os`; `has(bin)` tells whether a binary is on PATH (injected for tests).
pub fn choose(os: &Os, has: &dyn Fn(&str) -> bool) -> Strategy {
    match os {
        Os::MacOs => {
            if has("brew") {
                Strategy::Brew
            } else {
                Strategy::None
            }
        }
        Os::Linux { ids } => {
            if any(ids, &["debian", "ubuntu", "linuxmint", "pop", "raspbian"]) && has("apt-get") {
                Strategy::Apt
            } else if any(
                ids,
                &["fedora", "rhel", "centos", "rocky", "almalinux", "amzn"],
            ) && has("dnf")
            {
                Strategy::Dnf
            } else if any(ids, &["arch", "manjaro", "endeavouros"]) && has("pacman") {
                Strategy::Pacman
            } else if has("apt-get") {
                Strategy::Apt
            } else if has("dnf") {
                Strategy::Dnf
            } else if has("pacman") {
                Strategy::Pacman
            } else {
                Strategy::None
            }
        }
        _ => Strategy::None,
    }
}

/// The install command for a strategy. `root` skips `sudo`.
pub fn command_for(s: Strategy, root: bool) -> Option<Vec<String>> {
    let base: &[&str] = match s {
        Strategy::Brew => return Some(vec!["brew".into(), "install".into(), "tmux".into()]),
        Strategy::Apt => &["apt-get", "install", "-y", "tmux"],
        Strategy::Dnf => &["dnf", "install", "-y", "tmux"],
        Strategy::Pacman => &["pacman", "-S", "--noconfirm", "tmux"],
        Strategy::None => return None,
    };
    let mut v: Vec<String> = Vec::new();
    if !root {
        v.push("sudo".into());
    }
    v.extend(base.iter().map(|s| s.to_string()));
    Some(v)
}

fn is_root() -> bool {
    std::env::var("USER").is_ok_and(|u| u == "root")
}

/// `install_command()` for the current machine.
pub fn install_command() -> Option<Vec<String>> {
    command_for(choose(&super::platform::current_os(), &on_path), is_root())
}

/// Shell-quote for display ("exact command").
pub fn display(cmd: &[String]) -> String {
    cmd.iter()
        .map(|a| {
            if a.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./=:@".contains(c))
            {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where to read installation steps when no strategy fits.
pub const MANUAL_GUIDE_URL: &str = "https://github.com/tmux/tmux/wiki/Installing";

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(ids: &[&str]) -> Os {
        Os::Linux {
            ids: ids.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn has(list: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |b| list.contains(&b)
    }

    #[test]
    fn per_platform() {
        assert_eq!(choose(&Os::MacOs, &has(&["brew"])), Strategy::Brew);
        assert_eq!(choose(&Os::MacOs, &has(&[])), Strategy::None);
        assert_eq!(
            choose(&linux(&["ubuntu", "debian"]), &has(&["apt-get"])),
            Strategy::Apt
        );
        assert_eq!(choose(&linux(&["fedora"]), &has(&["dnf"])), Strategy::Dnf);
        assert_eq!(
            choose(&linux(&["arch"]), &has(&["pacman"])),
            Strategy::Pacman
        );
        assert_eq!(choose(&linux(&["weird"]), &has(&["dnf"])), Strategy::Dnf);
        assert_eq!(choose(&linux(&["weird"]), &has(&[])), Strategy::None);
        assert_eq!(choose(&Os::Windows, &has(&["brew"])), Strategy::None);
    }

    #[test]
    fn commands() {
        assert_eq!(
            command_for(Strategy::Brew, false).unwrap(),
            ["brew", "install", "tmux"]
        );
        assert_eq!(
            command_for(Strategy::Apt, false).unwrap(),
            ["sudo", "apt-get", "install", "-y", "tmux"]
        );
        assert_eq!(command_for(Strategy::Dnf, true).unwrap()[0], "dnf");
        assert_eq!(command_for(Strategy::None, false), None);
        assert_eq!(
            display(&command_for(Strategy::Pacman, false).unwrap()),
            "sudo pacman -S --noconfirm tmux"
        );
        assert_eq!(display(&["a b".into()]), "'a b'");
    }
}
