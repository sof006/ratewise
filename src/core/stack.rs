//! Works out which sound stack is running from the gathered facts (spec §3.4). Pure.

use crate::core::facts::{EnvFacts, PwDump};
use crate::core::model::{SessionManager, SoundServer, StackReport, WirePlumberConfig};

pub fn detect(pw: Option<&PwDump>, env: &EnvFacts, have_alsa_cards: bool) -> StackReport {
    // `/proc/<pid>/comm` is cut to 15 characters, so a 15-character prefix also counts.
    let running = |name: &str| {
        env.processes.as_ref().is_some_and(|p| {
            p.iter()
                .any(|n| n == name || (n.len() == 15 && name.starts_with(n.as_str())))
        })
    };
    let clients = pw.map(|p| p.clients.as_slice()).unwrap_or_default();
    let pulse_name = env.pulse_server_name.as_deref().unwrap_or_default();
    let pulse_on_pipewire = pulse_name.contains("on PipeWire");

    let pipewire_pulse = clients
        .iter()
        .any(|c| c.config_name.as_deref() == Some("pipewire-pulse.conf"))
        || running("pipewire-pulse")
        || pulse_on_pipewire;

    let session_manager = if let Some(wp) = clients
        .iter()
        .find(|c| c.application_name.as_deref() == Some("WirePlumber"))
    {
        let version = wp
            .application_version
            .clone()
            .or_else(|| env.wireplumber_version.clone());
        Some(wireplumber(version))
    } else if clients
        .iter()
        .any(|c| c.application_name.as_deref() == Some("pipewire-media-session"))
        || running("pipewire-media-session")
    {
        Some(SessionManager::PipewireMediaSession)
    } else if running("wireplumber") || (pw.is_some() && env.wireplumber_version.is_some()) {
        Some(wireplumber(env.wireplumber_version.clone()))
    } else {
        None
    };

    let real_pulseaudio = running("pulseaudio")
        || (!pulse_name.is_empty()
            && !pulse_on_pipewire
            && pulse_name.eq_ignore_ascii_case("pulseaudio"));
    let jack_running = running("jackd") || running("jackdbus");

    let server = if pw.is_some() {
        SoundServer::PipeWire
    } else if real_pulseaudio {
        SoundServer::PulseAudio
    } else if jack_running {
        SoundServer::Jack
    } else if have_alsa_cards {
        SoundServer::Alsa
    } else {
        SoundServer::None
    };

    StackReport {
        server,
        pipewire_version: pw.and_then(|p| p.core_version.clone()),
        pipewire_pulse: pw.is_some() && pipewire_pulse,
        session_manager: if pw.is_some() { session_manager } else { None },
        pulseaudio_version: if server == SoundServer::PulseAudio {
            env.pulse_server_version.clone()
        } else {
            None
        },
        jack_running,
        systemd_user_session: env.systemd_user_session,
    }
}

fn wireplumber(version: Option<String>) -> SessionManager {
    let config = version.as_deref().and_then(parse_version).map(|v| {
        if v < (0, 5, 0) {
            WirePlumberConfig::Lua04
        } else {
            WirePlumberConfig::SpaJson05
        }
    });
    SessionManager::WirePlumber { version, config }
}

/// `"0.5.18"` → `(0, 5, 18)`. Missing parts are 0; trailing junk (`-rc1`) is ignored.
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.trim().split('.').map(|p| {
        p.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u32>()
            .ok()
    });
    let major = parts.next()??;
    let minor = parts.next().flatten().unwrap_or(0);
    let patch = parts.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// The library version from `<tool> --version` output, e.g.
/// `"wireplumber | Compiled with libwireplumber 0.5.6 | Linked with libwireplumber 0.5.6"`.
/// Prefers the linked version (what actually runs).
pub fn tool_version(output: &str) -> Option<String> {
    let find = |prefix: &str| {
        output.split(['|', '\n']).find_map(|part| {
            let rest = part.trim().strip_prefix(prefix)?;
            let v = rest.split_whitespace().nth(1)?;
            parse_version(v).map(|_| v.to_string())
        })
    };
    find("Linked with").or_else(|| find("Compiled with"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::facts::PwClient;

    fn pw_with(clients: Vec<PwClient>) -> PwDump {
        PwDump {
            core_version: Some("1.6.10".into()),
            clients,
            ..Default::default()
        }
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("0.5.18"), Some((0, 5, 18)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("17.0-134-g77d25"), Some((17, 0, 0)));
        assert_eq!(parse_version("x"), None);
        assert_eq!(
            tool_version(
                "wireplumber | Compiled with libwireplumber 0.5.6 | Linked with libwireplumber 0.5.7"
            ),
            Some("0.5.7".into())
        );
        assert_eq!(tool_version("not installed"), None);
    }

    #[test]
    fn pipewire_from_clients() {
        let pw = pw_with(vec![
            PwClient {
                application_name: Some("WirePlumber".into()),
                application_version: Some("0.5.18".into()),
                config_name: Some("wireplumber.conf".into()),
            },
            PwClient {
                application_name: Some("pipewire".into()),
                config_name: Some("pipewire-pulse.conf".into()),
                ..Default::default()
            },
        ]);
        let r = detect(Some(&pw), &EnvFacts::default(), true);
        assert_eq!(r.server, SoundServer::PipeWire);
        assert!(r.pipewire_pulse);
        assert_eq!(
            r.session_manager,
            Some(SessionManager::WirePlumber {
                version: Some("0.5.18".into()),
                config: Some(WirePlumberConfig::SpaJson05)
            })
        );
        assert_eq!(
            r.summary(),
            "PipeWire 1.6.10 · WirePlumber 0.5.18 · pipewire-pulse"
        );
    }

    #[test]
    fn old_wireplumber_from_cli_version() {
        let env = EnvFacts {
            wireplumber_version: Some("0.4.8".into()),
            pulse_server_name: Some("PulseAudio (on PipeWire 0.3.48)".into()),
            ..Default::default()
        };
        let r = detect(Some(&pw_with(vec![])), &env, true);
        assert!(r.pipewire_pulse);
        assert_eq!(
            r.session_manager,
            Some(SessionManager::WirePlumber {
                version: Some("0.4.8".into()),
                config: Some(WirePlumberConfig::Lua04)
            })
        );
    }

    #[test]
    fn real_pulseaudio_and_jack_and_bare_alsa() {
        let env = EnvFacts {
            pulse_server_name: Some("pulseaudio".into()),
            pulse_server_version: Some("16.1".into()),
            ..Default::default()
        };
        let r = detect(None, &env, true);
        assert_eq!(r.server, SoundServer::PulseAudio);
        assert_eq!(r.summary(), "PulseAudio 16.1");

        let jack = EnvFacts {
            processes: Some(vec!["jackd".into()]),
            ..Default::default()
        };
        assert_eq!(detect(None, &jack, true).server, SoundServer::Jack);
        assert_eq!(
            detect(None, &EnvFacts::default(), true).server,
            SoundServer::Alsa
        );
        assert_eq!(
            detect(None, &EnvFacts::default(), false).server,
            SoundServer::None
        );
    }

    #[test]
    fn truncated_process_names_match() {
        // /proc/<pid>/comm is cut to 15 characters.
        let env = EnvFacts {
            processes: Some(vec!["pipewire-media-".into()]),
            ..Default::default()
        };
        let r = detect(Some(&pw_with(vec![])), &env, true);
        assert_eq!(
            r.session_manager,
            Some(SessionManager::PipewireMediaSession)
        );
    }
}
