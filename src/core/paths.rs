//! Where Ratewise's files live. Pure: the caller supplies the home, config and
//! state directories (from `$HOME`, `$XDG_CONFIG_HOME`, `$XDG_STATE_HOME`).
//!
//! Every write must pass [`Paths::is_allowed`]: inside the config folders of
//! PipeWire, WirePlumber or PulseAudio, `~/.asoundrc`, or Ratewise's state folder.

use std::path::{Component, Path, PathBuf};

/// The name Ratewise gives every file it owns.
pub const OWN_FILE: &str = "60-ratewise";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME`, usually `~/.config`.
    pub config: PathBuf,
    /// Ratewise's state folder, usually `~/.local/state/ratewise`.
    pub state: PathBuf,
    /// The system PulseAudio config, read (never written) when a user copy is needed.
    pub system_pulse_daemon_conf: PathBuf,
}

impl Paths {
    /// `config_home` and `state_home` are the XDG variables; per the XDG spec,
    /// relative values are ignored.
    pub fn new(home: PathBuf, config_home: Option<PathBuf>, state_home: Option<PathBuf>) -> Self {
        let absolute = |p: Option<PathBuf>| p.filter(|p| p.is_absolute());
        let config = absolute(config_home).unwrap_or_else(|| home.join(".config"));
        let state = absolute(state_home)
            .unwrap_or_else(|| home.join(".local/state"))
            .join("ratewise");
        Self {
            home,
            config,
            state,
            system_pulse_daemon_conf: PathBuf::from("/etc/pulse/daemon.conf"),
        }
    }

    fn own(dir: PathBuf, ext: &str) -> PathBuf {
        dir.join(format!("{OWN_FILE}.{ext}"))
    }

    pub fn pipewire_dropin(&self) -> PathBuf {
        Self::own(self.config.join("pipewire/pipewire.conf.d"), "conf")
    }
    pub fn client_dropin(&self) -> PathBuf {
        Self::own(self.config.join("pipewire/client.conf.d"), "conf")
    }
    /// PipeWire 0.3's config for ALSA-plugin and JACK clients.
    pub fn client_rt_dropin(&self) -> PathBuf {
        Self::own(self.config.join("pipewire/client-rt.conf.d"), "conf")
    }
    pub fn pipewire_pulse_dropin(&self) -> PathBuf {
        Self::own(self.config.join("pipewire/pipewire-pulse.conf.d"), "conf")
    }
    /// WirePlumber 0.5+ (SPA-JSON).
    pub fn wireplumber_05(&self) -> PathBuf {
        Self::own(self.config.join("wireplumber/wireplumber.conf.d"), "conf")
    }
    /// WirePlumber 0.4 (Lua).
    pub fn wireplumber_04(&self) -> PathBuf {
        Self::own(self.config.join("wireplumber/main.lua.d"), "lua")
    }
    pub fn pulse_daemon_conf(&self) -> PathBuf {
        self.config.join("pulse/daemon.conf")
    }
    pub fn pulse_dropin(&self) -> PathBuf {
        Self::own(self.config.join("pulse/daemon.conf.d"), "conf")
    }
    pub fn asoundrc(&self) -> PathBuf {
        self.home.join(".asoundrc")
    }
    /// Ratewise's record of what it has saved.
    pub fn saved(&self) -> PathBuf {
        self.state.join("saved.json")
    }
    pub fn backups(&self) -> PathBuf {
        self.state.join("backups")
    }

    /// Every file a plan may read or write.
    pub fn managed(&self) -> Vec<PathBuf> {
        vec![
            self.pipewire_dropin(),
            self.client_dropin(),
            self.client_rt_dropin(),
            self.pipewire_pulse_dropin(),
            self.wireplumber_05(),
            self.wireplumber_04(),
            self.pulse_daemon_conf(),
            self.pulse_dropin(),
            self.asoundrc(),
            self.saved(),
        ]
    }

    /// Whether Ratewise may write `p`. Purely lexical; the apply step also refuses symlinks.
    pub fn is_allowed(&self, p: &Path) -> bool {
        if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
            return false;
        }
        let roots = [
            self.config.join("pipewire"),
            self.config.join("wireplumber"),
            self.config.join("pulse"),
            self.state.clone(),
        ];
        p == self.asoundrc() || roots.iter().any(|r| p.starts_with(r) && p != r)
    }

    /// `p` with the home folder shown as `~`.
    pub fn display(&self, p: &Path) -> String {
        match p.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => p.display().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths::new("/home/u".into(), None, None)
    }

    #[test]
    fn defaults_follow_xdg() {
        let p = paths();
        assert_eq!(p.config, Path::new("/home/u/.config"));
        assert_eq!(p.state, Path::new("/home/u/.local/state/ratewise"));
        assert_eq!(
            p.wireplumber_04(),
            Path::new("/home/u/.config/wireplumber/main.lua.d/60-ratewise.lua")
        );
        let custom = Paths::new(
            "/home/u".into(),
            Some("/cfg".into()),
            Some("relative/ignored".into()),
        );
        assert_eq!(
            custom.pipewire_dropin(),
            Path::new("/cfg/pipewire/pipewire.conf.d/60-ratewise.conf")
        );
        assert_eq!(custom.state, Path::new("/home/u/.local/state/ratewise"));
    }

    #[test]
    fn every_managed_path_is_allowed() {
        let p = paths();
        for m in p.managed() {
            assert!(p.is_allowed(&m), "{}", m.display());
        }
        assert!(p.is_allowed(&p.backups().join("x/manifest.json")));
    }

    #[test]
    fn refuses_anything_else() {
        let p = paths();
        for bad in [
            "/etc/pulse/daemon.conf",
            "/home/u/.bashrc",
            "/home/u/.config/pipewire",
            "/home/u/.config/pipewire/../../.bashrc",
            "/home/u/.config/pipewire-evil/x",
            "/home/u/.local/state/other/x",
            "relative/.asoundrc",
        ] {
            assert!(!p.is_allowed(Path::new(bad)), "{bad}");
        }
    }

    #[test]
    fn display_uses_tilde() {
        let p = paths();
        assert_eq!(p.display(&p.asoundrc()), "~/.asoundrc");
        assert_eq!(p.display(Path::new("/etc/x")), "/etc/x");
    }
}
