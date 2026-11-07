//! `core::plan` against every fixture's stack: golden plans (with diffs), golden
//! generated config files, and the refusals. Pure: files live in a map.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::{assert_golden, fixture};
use ratewise::core::model::{Device, SampleFormat, SoundServer};
use ratewise::core::paths::Paths;
use ratewise::core::plan::{
    self, BackupView, ClockKey, ClockValue, Expect, FileState, Plan, PlanContext, SetRequest, Step,
    render_plan,
};
use ratewise::core::select::select;
use ratewise::core::settings::{Change, LdacQuality, LeAudioLatency, Mode, SavedConfig, Value};
use ratewise::probe::Probes;

const SYSTEM_PULSE: &str =
    "; System daemon.conf\n; default-sample-rate = 44100\nflat-volumes = no\n";

/// A system plus Ratewise's files, with plans applied in memory.
struct World {
    snapshot: ratewise::core::model::Snapshot,
    paths: Paths,
    saved: SavedConfig,
    files: BTreeMap<PathBuf, FileState>,
    tools: Vec<String>,
    pulse_override: bool,
}

impl World {
    fn new(name: &str) -> Self {
        Self {
            snapshot: Probes::fixture(&fixture(name)).snapshot(),
            paths: Paths::new("/home/user".into(), None, None),
            saved: SavedConfig::default(),
            files: BTreeMap::new(),
            tools: ["pw-metadata", "systemctl", "wpctl", "pw-cli"]
                .map(String::from)
                .to_vec(),
            pulse_override: false,
        }
    }

    fn ctx(&self) -> PlanContext<'_> {
        PlanContext {
            snapshot: &self.snapshot,
            paths: &self.paths,
            saved: &self.saved,
            files: &self.files,
            system_pulse_daemon_conf: Some(SYSTEM_PULSE),
            tools: &self.tools,
            pulse_config_override: self.pulse_override,
        }
    }

    fn device(&self, q: &str) -> Device {
        select(&self.snapshot.devices, q).unwrap().clone()
    }

    fn plan(
        &self,
        change: Change,
        device: Option<&str>,
        mode: Option<Mode>,
    ) -> Result<Plan, String> {
        let d = device.map(|q| self.device(q));
        plan::plan_set(
            &self.ctx(),
            &SetRequest {
                change,
                device: d.as_ref(),
                mode,
            },
        )
        .map_err(|e| e.0)
    }

    fn file(&self, p: &Path) -> Option<&str> {
        match self.files.get(p) {
            Some(FileState::Text(t)) => Some(t),
            _ => None,
        }
    }

    /// What the backend would do, in memory.
    fn apply(&mut self, plan: &Plan) {
        for step in &plan.steps {
            match step {
                Step::Write { path, new, .. } => {
                    self.files
                        .insert(path.clone(), FileState::Text(new.clone()));
                }
                Step::Remove { path, .. } => {
                    self.files.insert(path.clone(), FileState::Missing);
                }
                Step::Run { .. } => {}
            }
        }
        self.saved = match self.file(&self.paths.saved()) {
            Some(t) => serde_json::from_str(t).unwrap(),
            None => SavedConfig::default(),
        };
    }

    /// Plans and applies a chain of changes; returns every plan's text.
    fn run(&mut self, changes: &[(Change, Option<&str>, Option<Mode>)]) -> String {
        let mut out = Vec::new();
        for (change, device, mode) in changes {
            let p = self.plan(change.clone(), *device, *mode).unwrap();
            out.push(render_plan(&p, &self.paths));
            self.apply(&p);
        }
        out.join("\n=====\n\n")
    }

    /// Every Ratewise-managed file that exists, for config goldens.
    fn dump_files(&self, scenario: &str) {
        for (path, state) in &self.files {
            if let FileState::Text(t) = state
                && *path != self.paths.saved()
            {
                let rel = path.strip_prefix(&self.paths.home).unwrap();
                let name = rel
                    .to_string_lossy()
                    .trim_start_matches('.')
                    .replace('/', "__");
                assert_golden(&format!("config/{scenario}/{name}"), t);
            }
        }
    }
}

fn save() -> Option<Mode> {
    Some(Mode::Save)
}

const D50S: &str = "alsa_output.usb-Topping_D50s-00.analog-stereo";
const BUILTIN_SINK: &str = "alsa_output.pci-0000_00_1f.3.analog-stereo";
const HEADPHONES: &str = "bluez_output.00_00_00_00_00_01.1";
const SPEAKER: &str = "bluez_card.00_00_00_00_00_02";
const OLD_HEADSET: &str = "bluez_output.00_00_00_00_00_03.headset-head-unit";

#[test]
fn pipewire_1_2_wireplumber_0_5() {
    let mut w = World::new("pw-1.2-desktop");
    let text = w.run(&[
        (Change::Rate(Value::Set(96_000)), None, save()),
        (
            Change::AllowedRates(Value::Set(vec![48_000, 44_100, 96_000, 48_000])),
            None,
            save(),
        ),
        (Change::Quantum(Value::Set(512)), None, save()),
        (
            Change::Format(Value::Set(SampleFormat::S32Le)),
            Some("D50s"),
            save(),
        ),
        (Change::PeriodSize(Value::Set(1024)), Some("D50s"), save()),
        (
            Change::Suspend(Value::Set(false)),
            Some(BUILTIN_SINK),
            save(),
        ),
        (Change::ResampleQuality(Value::Set(10)), None, save()),
    ]);
    assert_golden("plan/pw-1.2-desktop.txt", &text);
    w.dump_files("pw-1.2-desktop");
    assert!(w.file(&w.paths.wireplumber_04()).is_none(), "0.5 only");
    assert!(
        w.file(&w.paths.client_rt_dropin()).is_none(),
        "PipeWire 1.x has no client-rt"
    );

    // Saving the same value again does nothing.
    let again = w
        .plan(Change::Quantum(Value::Set(512)), None, save())
        .unwrap();
    assert!(again.steps.is_empty());
    assert!(
        again.notes.iter().any(|n| n.contains("already saved")),
        "{:?}",
        again.notes
    );

    // Back to defaults one by one empties the record.
    for (c, d) in [
        (Change::Rate(Value::Default), None),
        (Change::AllowedRates(Value::Default), None),
        (Change::Quantum(Value::Default), None),
        (Change::ResampleQuality(Value::Default), None),
        (Change::Format(Value::Default), Some(D50S)),
        (Change::PeriodSize(Value::Default), Some(D50S)),
    ] {
        let p = w.plan(c, d, save()).unwrap();
        w.apply(&p);
    }
    let p = w
        .plan(Change::Suspend(Value::Default), Some(BUILTIN_SINK), save())
        .unwrap();
    w.apply(&p);
    assert!(w.saved.is_empty());
    assert!(
        w.files.values().all(|f| *f == FileState::Missing),
        "every file removed: {:?}",
        w.files.keys().collect::<Vec<_>>()
    );
}

#[test]
fn pipewire_live_changes_and_their_undo() {
    let w = World::new("pw-1.2-desktop");
    let p = w
        .plan(Change::Quantum(Value::Set(256)), None, None)
        .unwrap();
    assert_eq!(p.kind, plan::PlanKind::Set(Mode::Live));
    let Step::Run { cmd, undo, .. } = &p.steps[0] else {
        panic!("{p:?}")
    };
    assert_eq!(cmd.display(), "pw-metadata -n settings 0 clock.quantum 256");
    assert_eq!(
        undo.as_ref().unwrap().display(),
        "pw-metadata -n settings 0 clock.quantum 1024"
    );
    assert_eq!(
        p.expect,
        vec![Expect::Clock {
            key: ClockKey::Quantum,
            want: ClockValue::Number(256)
        }]
    );

    let p = w
        .plan(
            Change::AllowedRates(Value::Set(vec![44_100, 48_000])),
            None,
            Some(Mode::Live),
        )
        .unwrap();
    let Step::Run { cmd, .. } = &p.steps[0] else {
        panic!()
    };
    assert_eq!(
        cmd.display(),
        "pw-metadata -n settings 0 clock.allowed-rates '[ 44100 48000 ]'"
    );

    let p = w
        .plan(Change::Rate(Value::Default), None, Some(Mode::Live))
        .unwrap();
    let Step::Run { cmd, .. } = &p.steps[0] else {
        panic!()
    };
    assert_eq!(cmd.display(), "pw-metadata -n settings -d 0 clock.rate");

    let p = w.plan(Change::ForceRate(96_000), None, None).unwrap();
    let Step::Run { cmd, undo, .. } = &p.steps[0] else {
        panic!()
    };
    assert_eq!(
        cmd.display(),
        "pw-metadata -n settings 0 clock.force-rate 96000"
    );
    assert_eq!(
        undo.as_ref().unwrap().display(),
        "pw-metadata -n settings 0 clock.force-rate 0"
    );

    assert_golden(
        "plan/pw-1.2-desktop-live.txt",
        &render_plan(
            &w.plan(Change::Quantum(Value::Set(256)), None, None)
                .unwrap(),
            &w.paths,
        ),
    );
}

#[test]
fn pipewire_0_3_wireplumber_0_4_writes_lua() {
    let mut w = World::new("pw-0.3-laptop");
    let sink = BUILTIN_SINK;
    let text = w.run(&[
        (
            Change::Format(Value::Set(SampleFormat::S16Le)),
            Some(sink),
            save(),
        ),
        (Change::Headroom(Value::Set(1024)), Some(sink), save()),
        (Change::Suspend(Value::Set(false)), Some(sink), save()),
        (Change::ResampleQuality(Value::Set(6)), None, save()),
    ]);
    assert_golden("plan/pw-0.3-laptop.txt", &text);
    w.dump_files("pw-0.3-laptop");
    let lua = w.file(&w.paths.wireplumber_04()).unwrap();
    assert!(
        lua.contains(r#"{ "node.name", "equals", "alsa_output.pci-0000_00_1f.3.analog-stereo" },"#)
    );
    assert!(lua.contains(r#"["audio.format"] = "S16LE","#));
    assert!(w.file(&w.paths.wireplumber_05()).is_none());
    assert!(
        w.file(&w.paths.client_rt_dropin()).is_some(),
        "PipeWire 0.3 also reads client-rt.conf"
    );
}

#[test]
fn pulseaudio_uses_a_drop_in_and_creates_daemon_conf() {
    let mut w = World::new("pulseaudio-only");
    assert_eq!(w.snapshot.stack.server, SoundServer::PulseAudio);
    let text = w.run(&[
        (Change::Rate(Value::Set(48_000)), None, save()),
        (
            Change::AllowedRates(Value::Set(vec![44_100, 48_000])),
            None,
            save(),
        ),
        (
            Change::Format(Value::Set(SampleFormat::S24Le)),
            None,
            save(),
        ),
    ]);
    assert_golden("plan/pulseaudio-only.txt", &text);
    w.dump_files("pulseaudio-only");
    let copy = w.file(&w.paths.pulse_daemon_conf()).unwrap();
    assert!(copy.ends_with(SYSTEM_PULSE), "a copy of the system file");
    let dropin = w.file(&w.paths.pulse_dropin()).unwrap();
    assert!(dropin.contains("default-sample-format = s24-32le"));

    let reset = plan::plan_reset(&w.ctx()).unwrap();
    assert_golden(
        "plan/pulseaudio-only-reset.txt",
        &render_plan(&reset, &w.paths),
    );
    w.apply(&reset);
    assert!(
        w.file(&w.paths.pulse_daemon_conf()).is_none(),
        "unedited copy removed"
    );
}

#[test]
fn pulseaudio_keeps_a_daemon_conf_that_was_edited_or_already_there() {
    let mut w = World::new("pulseaudio-only");
    let p = w
        .plan(Change::Rate(Value::Set(48_000)), None, save())
        .unwrap();
    w.apply(&p);
    let edited = format!(
        "{}resample-method = soxr-hq\n",
        w.file(&w.paths.pulse_daemon_conf()).unwrap()
    );
    w.files
        .insert(w.paths.pulse_daemon_conf(), FileState::Text(edited));
    let reset = plan::plan_reset(&w.ctx()).unwrap();
    assert!(
        reset
            .steps
            .iter()
            .all(|s| s.path() != Some(w.paths.pulse_daemon_conf().as_path()))
    );
    assert!(reset.notes.iter().any(|n| n.contains("edited since")));

    let mut w = World::new("pulseaudio-only");
    w.files.insert(
        w.paths.pulse_daemon_conf(),
        FileState::Text("; mine\n".into()),
    );
    let p = w
        .plan(Change::Rate(Value::Set(48_000)), None, save())
        .unwrap();
    assert!(
        p.steps
            .iter()
            .all(|s| s.path() != Some(w.paths.pulse_daemon_conf().as_path())),
        "user's file untouched"
    );
}

#[test]
fn alsa_only_edits_a_marked_block_and_keeps_the_rest() {
    let mut w = World::new("alsa-only");
    let user = "# my settings\npcm.!default {\n    type plug\n    slave.pcm \"hw:1,0\"\n}\n";
    w.files
        .insert(w.paths.asoundrc(), FileState::Text(user.into()));
    let text = w.run(&[
        (Change::Rate(Value::Set(48_000)), None, save()),
        (
            Change::Format(Value::Set(SampleFormat::S32Le)),
            None,
            save(),
        ),
        (
            Change::PeriodSize(Value::Set(256)),
            Some("alsa:Device:0:capture"),
            save(),
        ),
    ]);
    assert_golden("plan/alsa-only.txt", &text);
    w.dump_files("alsa-only");
    let rc = w.file(&w.paths.asoundrc()).unwrap();
    assert!(rc.starts_with(user));
    assert!(rc.contains("cards.Device.pcm.dsnoop.period_size 256\n"));

    let reset = plan::plan_reset(&w.ctx()).unwrap();
    w.apply(&reset);
    assert_eq!(w.file(&w.paths.asoundrc()), Some(user), "byte for byte");
}

#[test]
fn reset_releases_forced_live_settings() {
    let mut w = World::new("pw-1.2-desktop");
    if let Some(c) = &mut w.snapshot.clock {
        c.value.force_rate = Some(96_000);
    }
    let reset = plan::plan_reset(&w.ctx()).unwrap();
    let Step::Run { cmd, undo, .. } = &reset.steps[0] else {
        panic!("{reset:?}")
    };
    assert_eq!(
        cmd.display(),
        "pw-metadata -n settings 0 clock.force-rate 0"
    );
    assert_eq!(
        undo.as_ref().unwrap().display(),
        "pw-metadata -n settings 0 clock.force-rate 96000"
    );

    let empty = plan::plan_reset(&World::new("pw-1.2-desktop").ctx()).unwrap();
    assert!(empty.steps.is_empty());
    assert!(empty.notes[0].contains("nothing to reset"));
}

#[test]
fn revert_restores_files_and_live_values() {
    let mut w = World::new("pw-1.2-desktop");
    let p = w
        .plan(Change::Rate(Value::Set(96_000)), None, save())
        .unwrap();
    w.apply(&p);
    let backup = BackupView {
        id: "20261107-120000".into(),
        description: p.title.clone(),
        files: vec![(w.paths.pipewire_dropin(), None), (w.paths.saved(), None)],
        live_undo: vec![plan::Cmd::new(
            "pw-metadata",
            &["-n", "settings", "0", "clock.quantum", "1024"],
        )],
    };
    let r = plan::plan_revert(&w.ctx(), &backup).unwrap();
    assert_golden("plan/revert.txt", &render_plan(&r, &w.paths));
    w.apply(&r);
    assert!(w.file(&w.paths.pipewire_dropin()).is_none());
    assert!(w.saved.is_empty());

    let evil = BackupView {
        files: vec![(PathBuf::from("/home/user/.bashrc"), Some("x".into()))],
        ..backup
    };
    let err = plan::plan_revert(&w.ctx(), &evil).unwrap_err();
    assert!(err.0.contains("outside the folders"), "{err}");
}

#[test]
fn refusals_explain_what_to_do() {
    let pw = World::new("pw-1.2-desktop");
    let mut cases: Vec<(Result<Plan, String>, &str)> = vec![
        (
            pw.plan(Change::Rate(Value::Set(96_000)), Some(D50S), None),
            "Add --save",
        ),
        (
            pw.plan(
                Change::Rate(Value::Set(96_000)),
                Some(D50S),
                Some(Mode::Live),
            ),
            "Use --save instead",
        ),
        (
            pw.plan(Change::ForceRate(96_000), None, save()),
            "live-only",
        ),
        (
            pw.plan(
                Change::Format(Value::Set(SampleFormat::S32Le)),
                None,
                save(),
            ),
            "set per device",
        ),
        (
            pw.plan(Change::Quantum(Value::Set(256)), Some(D50S), save()),
            "global setting",
        ),
        (
            pw.plan(Change::PeriodSize(Value::Set(256)), None, save()),
            "set per device",
        ),
        (
            pw.plan(Change::Rate(Value::Set(22_050)), Some(D50S), save()),
            "can't run at 22.05 kHz",
        ),
        (
            pw.plan(
                Change::Format(Value::Set(SampleFormat::S16Le)),
                Some(D50S),
                save(),
            ),
            "doesn't support S16_LE",
        ),
        (
            pw.plan(Change::Rate(Value::Set(5)), None, save()),
            "isn't a usable sample rate",
        ),
        (
            pw.plan(Change::Quantum(Value::Set(16)), None, save()),
            "below the minimum quantum (32)",
        ),
        (
            pw.plan(Change::Quantum(Value::Set(9000)), None, save()),
            "between 1 and 8192",
        ),
        (
            pw.plan(
                Change::Rate(Value::Set(96_000)),
                Some("bluez_output.00_00_00_00_00_01.1"),
                save(),
            ),
            "Bluetooth",
        ),
        (
            pw.plan(
                Change::Rate(Value::Set(48_000)),
                Some("alsa:NVidia:7:playback"),
                save(),
            ),
            "profile doesn't include it",
        ),
        (
            pw.plan(Change::ResampleQuality(Value::Set(15)), None, save()),
            "0 (fastest) to 14",
        ),
        (
            pw.plan(
                Change::ResampleQuality(Value::Set(8)),
                None,
                Some(Mode::Live),
            ),
            "when a stream starts",
        ),
    ];
    let pa = World::new("pulseaudio-only");
    let mut pa_override = World::new("pulseaudio-only");
    pa_override.pulse_override = true;
    let alsa = World::new("alsa-only");
    let none = World::new("no-sound");
    let mut no_tools = World::new("pw-1.2-desktop");
    no_tools.tools.clear();
    cases.extend([
        (
            pa.plan(Change::Quantum(Value::Set(256)), None, save()),
            "per program",
        ),
        (
            pa.plan(Change::Rate(Value::Set(48_000)), None, Some(Mode::Live)),
            "PulseAudio can't change it while running",
        ),
        (
            pa.plan(
                Change::AllowedRates(Value::Set(vec![44_100, 48_000, 96_000])),
                None,
                save(),
            ),
            "only two rates",
        ),
        (
            pa.plan(
                Change::Rate(Value::Set(48_000)),
                Some("alsa:PCH:3:playback"),
                save(),
            ),
            "one sample rate for every device",
        ),
        (
            pa_override.plan(Change::Rate(Value::Set(48_000)), None, save()),
            "PULSE_CONFIG",
        ),
        (
            alsa.plan(Change::AllowedRates(Value::Set(vec![48_000])), None, save()),
            "Set `rate` instead",
        ),
        (
            alsa.plan(
                Change::Rate(Value::Set(48_000)),
                Some("alsa:Device:0:capture"),
                save(),
            ),
            "one sample rate for every device",
        ),
        (
            none.plan(Change::Rate(Value::Set(48_000)), None, save()),
            "nothing to configure",
        ),
        (
            no_tools.plan(Change::Quantum(Value::Set(256)), None, None),
            "pw-metadata isn't installed",
        ),
    ]);
    for (i, (result, want)) in cases.into_iter().enumerate() {
        match result {
            Err(msg) => assert!(
                msg.contains(want),
                "case {i}: {msg:?} should mention {want:?}"
            ),
            Ok(p) => panic!("case {i} should be refused ({want}), got {}", p.title),
        }
    }
}

#[test]
fn files_ratewise_wont_touch() {
    let mut w = World::new("pw-1.2-desktop");
    w.files
        .insert(w.paths.pipewire_dropin(), FileState::Symlink);
    let err = w
        .plan(Change::Rate(Value::Set(96_000)), None, save())
        .unwrap_err();
    assert!(err.contains("symbolic link"), "{err}");

    let mut w = World::new("alsa-only");
    w.files.insert(
        w.paths.asoundrc(),
        FileState::Text("# >>> ratewise >>>\nhalf a block\n".into()),
    );
    let err = w
        .plan(Change::Rate(Value::Set(48_000)), None, save())
        .unwrap_err();
    assert!(err.contains("without a matching"), "{err}");

    let mut w = World::new("alsa-only");
    w.files.insert(
        w.paths.asoundrc(),
        FileState::Text(
            "# >>> ratewise >>>\n# <<< ratewise <<<\ndefaults.pcm.dmix.rate 44100\n".into(),
        ),
    );
    let p = w
        .plan(Change::Rate(Value::Set(48_000)), None, save())
        .unwrap();
    assert!(
        p.notes.iter().any(|n| n.contains("after Ratewise's block")),
        "{:?}",
        p.notes
    );
}

#[test]
fn warnings_for_unusual_values() {
    let w = World::new("pw-1.2-desktop");
    let p = w
        .plan(Change::Quantum(Value::Set(300)), None, save())
        .unwrap();
    assert!(
        p.notes.iter().any(|n| n.contains("it will use 256")),
        "{:?}",
        p.notes
    );
    let p = w
        .plan(
            Change::AllowedRates(Value::Set(vec![44_100, 96_000])),
            None,
            save(),
        )
        .unwrap();
    assert!(
        p.notes
            .iter()
            .any(|n| n.contains("(48 kHz) isn't in the list")),
        "{:?}",
        p.notes
    );
    let p = w
        .plan(Change::Rate(Value::Set(768_000)), None, save())
        .unwrap();
    assert!(p.notes[0].contains("resample for them"), "{:?}", p.notes);
}

#[test]
fn a_batch_of_edits_is_one_plan_with_one_restart() {
    // The window collects edits and applies them together.
    let mut w = World::new("pw-1.2-desktop");
    let d50s = w.device("D50s");
    let edits = [
        (Change::Quantum(Value::Set(256)), None),
        (Change::Suspend(Value::Set(false)), Some(&d50s)),
        (Change::AllowedRates(Value::Set(vec![96_000, 44_100])), None),
    ];
    let p = plan::plan_batch(&w.ctx(), &edits, Mode::Save).unwrap();
    let text = render_plan(&p, &w.paths);
    assert_golden("plan/pw-1.2-desktop-batch.txt", &text);
    let writes: Vec<_> = p.steps.iter().filter_map(Step::path).collect();
    assert_eq!(
        writes,
        [
            w.paths.pipewire_dropin().as_path(),
            w.paths.wireplumber_05().as_path(),
            w.paths.saved().as_path(),
        ],
        "each file is written once"
    );
    assert!(p.restart.is_some());
    w.apply(&p);
    assert_eq!(w.saved.global.quantum, Some(256));
    assert_eq!(w.saved.global.allowed_rates, Some(vec![44_100, 96_000]));

    // The window's version has no title and no command-line hints.
    let window = plan::render_plan_window(&p, &w.paths);
    assert!(!window.contains("--restart") && !window.contains("ratewise revert"));

    let none = plan::plan_batch(&w.ctx(), &[], Mode::Save).unwrap_err();
    assert!(none.0.contains("no changes"), "{}", none.0);
    // One bad edit refuses the whole batch.
    let bt = w.device(HEADPHONES);
    let bad = [
        (Change::Quantum(Value::Set(512)), None),
        (Change::PeriodSize(Value::Set(256)), Some(&bt)),
    ];
    assert!(plan::plan_batch(&w.ctx(), &bad, Mode::Save).is_err());
}

fn run_cmd(p: &Plan) -> (String, Option<String>) {
    match &p.steps[..] {
        [Step::Run { cmd, undo, .. }] => (cmd.display(), undo.as_ref().map(|u| u.display())),
        s => panic!("expected one command: {s:?}"),
    }
}

fn profile(want: &str) -> Change {
    Change::BluetoothProfile(want.into())
}

#[test]
fn bluetooth_profiles_switch_live_and_are_remembered_not_saved() {
    let mut w = World::new("pw-1.2-desktop");
    // PipeWire 1.x names the best codec's profile without a suffix.
    let p = w.plan(profile("ldac"), Some(HEADPHONES), None).unwrap();
    assert_eq!(p.kind, plan::PlanKind::Set(Mode::Live));
    let (cmd, undo) = run_cmd(&p);
    assert_eq!(
        cmd,
        "pw-cli set-param bluez_card.00_00_00_00_00_01 Profile '{ index: 131080, save: true }'"
    );
    assert_eq!(
        undo.unwrap(),
        "pw-cli set-param bluez_card.00_00_00_00_00_01 Profile '{ index: 131076, save: true }'"
    );
    assert_eq!(
        p.expect,
        vec![Expect::BluetoothProfile {
            card: "bluez_card.00_00_00_00_00_01".into(),
            description: "Bluetooth Headphones".into(),
            profile: "a2dp-sink".into(),
            label: "High quality (LDAC)".into(),
        }]
    );
    assert_golden(
        "plan/pw-1.2-desktop-bluetooth-profile.txt",
        &render_plan(&p, &w.paths),
    );

    // A mode picks the best profile of that mode.
    let p = w.plan(profile("headset"), Some(HEADPHONES), None).unwrap();
    assert!(run_cmd(&p).0.contains("index: 196865"));
    assert!(
        p.notes.iter().any(|n| n.contains("phone-call quality")),
        "{:?}",
        p.notes
    );

    // A switched-off device has a card but no stream; it can be turned on.
    let (cmd, _) = run_cmd(
        &w.plan(profile("high-quality"), Some(SPEAKER), None)
            .unwrap(),
    );
    assert_eq!(
        cmd,
        "pw-cli set-param bluez_card.00_00_00_00_00_02 Profile '{ index: 131076, save: true }'"
    );

    // Already in use: nothing to run.
    let p = w.plan(profile("aac"), Some(HEADPHONES), None).unwrap();
    assert!(p.steps.is_empty());
    assert!(
        p.notes.iter().any(|n| n.contains("already using")),
        "{:?}",
        p.notes
    );

    for (result, want) in [
        (
            w.plan(profile("headset"), Some(HEADPHONES), save()),
            "remembers the profile",
        ),
        (
            w.plan(profile("aptx"), Some(HEADPHONES), None),
            "LDAC, AAC, SBC-XQ, SBC, mSBC, CVSD",
        ),
        (
            w.plan(profile("hq"), Some(D50S), None),
            "isn't a Bluetooth device",
        ),
        (w.plan(profile("hq"), None, None), "--device"),
    ] {
        let e = result.unwrap_err();
        assert!(e.contains(want), "{e}");
    }
    w.tools.retain(|t| t != "pw-cli");
    let e = w.plan(profile("ldac"), Some(HEADPHONES), None).unwrap_err();
    assert!(e.contains("pw-cli isn't installed"), "{e}");
}

#[test]
fn old_pipewire_profiles_and_ids() {
    let mut w = World::new("pw-0.3-laptop");
    let d = w.device(OLD_HEADSET);
    let bt = d.bluetooth.as_ref().unwrap();
    assert_eq!(bt.profile.as_deref(), Some("headset-head-unit-msbc"));
    // PipeWire 0.3 also lists codec-less profiles; a mode picks the best one.
    let p = w
        .plan(profile("high-quality"), Some(OLD_HEADSET), None)
        .unwrap();
    assert_eq!(
        run_cmd(&p).0,
        "pw-cli set-param bluez_card.00_00_00_00_00_03 Profile '{ index: 131080, save: true }'"
    );
    assert!(
        p.notes.iter().any(|n| n.contains("microphone stops")),
        "{:?}",
        p.notes
    );
    // pw-cli finds objects by name only from 0.3.44.
    w.snapshot.stack.pipewire_version = Some("0.3.40".into());
    let p = w.plan(profile("sbc"), Some(OLD_HEADSET), None).unwrap();
    assert_eq!(
        run_cmd(&p).0,
        "pw-cli set-param 57 Profile '{ index: 131073, save: true }'"
    );
}

#[test]
fn headset_autoswitch_live_on_wireplumber_0_5() {
    let mut w = World::new("pw-1.2-desktop");
    assert_eq!(w.snapshot.wireplumber.headset_autoswitch, Some(true));
    let off = Change::HeadsetAutoswitch(Value::Set(false));
    let p = w.plan(off.clone(), None, None).unwrap();
    let (cmd, undo) = run_cmd(&p);
    assert_eq!(
        cmd,
        "wpctl settings bluetooth.autoswitch-to-headset-profile false"
    );
    assert_eq!(
        undo.unwrap(),
        "wpctl settings bluetooth.autoswitch-to-headset-profile true"
    );
    assert!(
        p.title.contains("until WirePlumber restarts"),
        "{}",
        p.title
    );
    let p = w
        .plan(Change::HeadsetAutoswitch(Value::Default), None, None)
        .unwrap();
    assert_eq!(
        run_cmd(&p).0,
        "wpctl settings --reset bluetooth.autoswitch-to-headset-profile"
    );
    let same = w
        .plan(Change::HeadsetAutoswitch(Value::Set(true)), None, None)
        .unwrap();
    assert!(same.steps.is_empty());

    let e = w.plan(off.clone(), Some(HEADPHONES), None).unwrap_err();
    assert!(e.contains("Leave out --device"), "{e}");

    // A value stored with `wpctl settings --save` beats config files.
    w.snapshot.wireplumber.headset_autoswitch_saved = Some(true);
    let p = w.plan(off, None, save()).unwrap();
    assert!(
        p.notes
            .iter()
            .any(|n| n.contains("wpctl settings --delete")),
        "{:?}",
        p.notes
    );
}

#[test]
fn bluetooth_settings_saved_for_wireplumber_0_5() {
    let mut w = World::new("pw-1.2-desktop");
    let text = w.run(&[
        (Change::HeadsetAutoswitch(Value::Set(false)), None, save()),
        (
            Change::LdacQuality(Value::Set(LdacQuality::Hq)),
            Some(HEADPHONES),
            save(),
        ),
        (
            Change::Suspend(Value::Set(false)),
            Some(BUILTIN_SINK),
            save(),
        ),
    ]);
    assert_golden("plan/pw-1.2-desktop-bluetooth.txt", &text);
    w.dump_files("pw-1.2-desktop-bluetooth");
    let conf = w.file(&w.paths.wireplumber_05()).unwrap();
    assert!(conf.contains("monitor.alsa.rules") && conf.contains("monitor.bluez.rules"));

    let ldac = Change::LdacQuality(Value::Set(LdacQuality::Sq));
    for (result, want) in [
        (
            w.plan(ldac.clone(), Some(HEADPHONES), Some(Mode::Live)),
            "--save",
        ),
        (
            w.plan(ldac.clone(), Some(SPEAKER), save()),
            "doesn't offer LDAC",
        ),
        (w.plan(ldac, Some(D50S), save()), "isn't a Bluetooth device"),
    ] {
        let e = result.unwrap_err();
        assert!(e.contains(want), "{e}");
    }

    let reset = plan::plan_reset(&w.ctx()).unwrap();
    w.apply(&reset);
    assert!(w.saved.is_empty());
    assert!(w.files.values().all(|f| *f == FileState::Missing));
}

#[test]
fn le_audio_latency_saved_for_pipewire_1_6() {
    let mut w = World::new("pw-1.6-le-audio");
    let low = Change::LeAudioLatency(Value::Set(LeAudioLatency::Low));
    let text = w.run(&[(low.clone(), Some(HEADPHONES), save())]);
    assert_golden("plan/pw-1.6-le-audio-bluetooth.txt", &text);
    w.dump_files("pw-1.6-le-audio-bluetooth");
    let conf = w.file(&w.paths.wireplumber_05()).unwrap();
    assert!(
        conf.contains(r#"bluez5.bap.force-target-latency = "low-latency""#),
        "{conf}"
    );

    for (result, want) in [
        (
            w.plan(low.clone(), Some(HEADPHONES), Some(Mode::Live)),
            "--save",
        ),
        (
            w.plan(low.clone(), Some(SPEAKER), save()),
            "doesn't offer LE Audio",
        ),
        (
            w.plan(low.clone(), Some(D50S), save()),
            "isn't a Bluetooth device",
        ),
    ] {
        let e = result.unwrap_err();
        assert!(e.contains(want), "{e}");
    }

    // Switching to reliable rewrites the rule; balanced removes it.
    let p = w
        .plan(
            Change::LeAudioLatency(Value::Set(LeAudioLatency::Reliable)),
            Some(HEADPHONES),
            save(),
        )
        .unwrap();
    w.apply(&p);
    let conf = w.file(&w.paths.wireplumber_05()).unwrap();
    assert!(conf.contains(r#""high-reliability""#), "{conf}");
    let p = w
        .plan(
            Change::LeAudioLatency(Value::Default),
            Some(HEADPHONES),
            save(),
        )
        .unwrap();
    w.apply(&p);
    assert!(w.saved.is_empty());
    assert!(w.files.values().all(|f| *f == FileState::Missing));
}

#[test]
fn le_audio_latency_needs_pipewire_1_6() {
    let w = World::new("pw-1.2-desktop");
    let e = w
        .plan(
            Change::LeAudioLatency(Value::Set(LeAudioLatency::Low)),
            Some(HEADPHONES),
            save(),
        )
        .unwrap_err();
    assert!(e.contains("PipeWire 1.6 or newer"), "{e}");
}

#[test]
fn bluetooth_settings_saved_for_wireplumber_0_4() {
    let mut w = World::new("pw-0.3-laptop");
    let e = w
        .plan(
            Change::HeadsetAutoswitch(Value::Set(false)),
            None,
            Some(Mode::Live),
        )
        .unwrap_err();
    assert!(e.contains("WirePlumber 0.4"), "{e}");
    let text = w.run(&[
        (Change::HeadsetAutoswitch(Value::Set(false)), None, save()),
        (
            Change::LdacQuality(Value::Set(LdacQuality::Mq)),
            Some(OLD_HEADSET),
            save(),
        ),
    ]);
    assert_golden("plan/pw-0.3-laptop-bluetooth.txt", &text);
    w.dump_files("pw-0.3-laptop-bluetooth");
    assert!(w.file(&w.paths.wireplumber_05()).is_none());
    let lua = w.file(&w.paths.wireplumber_bluetooth_04()).unwrap();
    assert!(
        lua.contains(r#"["bluez5.a2dp.ldac.quality"] = "mq","#),
        "{lua}"
    );

    // Back to the defaults removes both files.
    for (c, d) in [
        (Change::HeadsetAutoswitch(Value::Default), None),
        (Change::LdacQuality(Value::Default), Some(OLD_HEADSET)),
    ] {
        let p = w.plan(c, d, save()).unwrap();
        w.apply(&p);
    }
    assert!(w.saved.is_empty());
    assert!(w.files.values().all(|f| *f == FileState::Missing));
}
