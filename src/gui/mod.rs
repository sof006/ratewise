//! The Slint window (spec §5).
//!
//! Probing and applying run on a worker thread so the window never blocks; the
//! main thread keeps the latest [`Inputs`] and the pending edits, plans with
//! `core::plan` (pure and fast), and sends plans back to the worker to apply.

mod edits;
mod view;

slint::include_modules!();

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::bail;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app::{Env, Inputs, Session, render_outcome};
use crate::core::plan::{self, Plan, render_plan_window};
use crate::core::settings::Mode;
use crate::probe::Probes;
use edits::{Edit, Edits};
use view::Ctx;

pub struct Options {
    pub fixture: Option<PathBuf>,
    pub sandbox: Option<PathBuf>,
    /// Render to this PNG and exit.
    pub screenshot: Option<PathBuf>,
    /// Render one frame, check it isn't blank, and exit 0.
    pub self_test: bool,
}

/// How often "Running now" refreshes. Light refreshes don't open devices.
const POLL: Duration = Duration::from_secs(3);

enum Job {
    Refresh {
        full: bool,
    },
    Execute {
        plan: Box<Plan>,
        restart: bool,
        revert_id: Option<String>,
    },
    Doctor,
}

enum Reply {
    ReadOnly(Option<String>),
    Inputs(Box<Inputs>),
    Executed(Result<String, String>),
    Doctor(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReviewKind {
    Set,
    Revert,
    Reset,
}

struct Review {
    kind: ReviewKind,
    plan: Option<Plan>,
    revert_id: Option<String>,
    view: ReviewView,
}

struct Shot {
    path: PathBuf,
    self_test: bool,
    started: bool,
}

struct State {
    tx: mpsc::Sender<Job>,
    inputs: Option<Inputs>,
    edits: Edits,
    selected: String,
    read_only: Option<String>,
    fixture: Option<PathBuf>,
    errors: Vec<String>,
    busy: bool,
    review: Option<Review>,
    doctor: String,
    /// The quantum the slider was last set to from here, so polling doesn't move it.
    slider_quantum: Option<u32>,
    nav: Rc<VecModel<NavItem>>,
    shot: Option<Shot>,
    shot_result: Option<Result<String, String>>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

pub fn run(opts: Options) -> anyhow::Result<()> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if var("WAYLAND_DISPLAY").is_none() && var("DISPLAY").is_none() {
        bail!(
            "No display was found (neither WAYLAND_DISPLAY nor DISPLAY is set), so the window \
             can't open. The command line works without one: try `ratewise list`, \
             `ratewise doctor` or `ratewise --help`."
        );
    }
    if std::env::var("RATEWISE_RENDERER").as_deref() == Ok("software") {
        slint::BackendSelector::new()
            .renderer_name("software".into())
            .select()
            .map_err(|e| anyhow::anyhow!("Couldn't start the software renderer: {e}."))?;
    }
    crate::crash::set_gui(true);
    let w = AppWindow::new().map_err(|e| {
        anyhow::anyhow!(
            "Couldn't open a window: {e}. The command line works without one: try \
             `ratewise list` or `ratewise --help`."
        )
    })?;

    let (tx, rx) = mpsc::channel();
    let nav = Rc::new(VecModel::default());
    w.set_nav(ModelRc::from(nav.clone()));
    let shot = opts
        .screenshot
        .clone()
        .map(|path| Shot {
            path,
            self_test: false,
            started: false,
        })
        .or_else(|| {
            opts.self_test.then(|| Shot {
                path: std::env::temp_dir().join("ratewise-self-test.png"),
                self_test: true,
                started: false,
            })
        });
    let testing = shot.is_some();
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            tx: tx.clone(),
            inputs: None,
            edits: Edits::default(),
            selected: String::new(),
            read_only: None,
            fixture: opts.fixture.clone(),
            errors: Vec::new(),
            busy: true,
            review: None,
            doctor: String::new(),
            slider_quantum: None,
            nav,
            shot,
            shot_result: None,
        })
    });
    if let Ok(theme) = std::env::var("RATEWISE_THEME") {
        w.invoke_set_theme(theme.into());
    }
    if let Some(mono) = monospace_family() {
        w.global::<Theme>().set_mono(mono.into());
    }

    let weak = w.as_weak();
    let (fixture, sandbox) = (opts.fixture.clone(), opts.sandbox.clone());
    std::thread::Builder::new()
        .name("ratewise-worker".into())
        .spawn(move || worker(rx, fixture, sandbox, weak))?;
    let _ = tx.send(Job::Refresh { full: true });

    connect(&w);
    render(&w);

    // Poll "Running now", without opening devices.
    let poll = slint::Timer::default();
    if !testing {
        poll.start(slint::TimerMode::Repeated, POLL, || {
            with_state(|st| {
                let working = st
                    .review
                    .as_ref()
                    .is_some_and(|r| r.view.state == "working");
                if !st.busy && !working {
                    let _ = st.tx.send(Job::Refresh { full: false });
                }
            });
        });
    }
    // A self-test that never gets data still has to finish.
    let watchdog = slint::Timer::default();
    if testing {
        let weak = w.as_weak();
        watchdog.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(30),
            move || {
                if let Some(w) = weak.upgrade() {
                    take_shot(&w);
                }
            },
        );
    }

    w.run()
        .map_err(|e| anyhow::anyhow!("The window stopped unexpectedly: {e}."))?;
    crate::crash::set_gui(false);
    match with_state(|st| st.shot_result.take()).flatten() {
        Some(Ok(msg)) => {
            println!("{msg}");
            Ok(())
        }
        Some(Err(e)) => bail!(e),
        None => Ok(()),
    }
}

/// The system's monospace family, from fontconfig's `fc-match`. Slint doesn't resolve
/// the generic "monospace" name, so without this the report and diffs use the UI font.
fn monospace_family() -> Option<String> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{family[0]}", "monospace"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let family = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !family.is_empty()).then_some(family)
}

// ---- worker ------------------------------------------------------------------------

fn worker(
    rx: mpsc::Receiver<Job>,
    fixture: Option<PathBuf>,
    sandbox: Option<PathBuf>,
    weak: slint::Weak<AppWindow>,
) {
    let send = |reply: Reply| {
        let _ = weak.upgrade_in_event_loop(move |w| on_reply(&w, reply));
    };
    let probes = match &fixture {
        Some(dir) => Probes::fixture(dir),
        None => Probes::system(),
    };
    let (env, read_only) = match Env::new(fixture.is_some(), sandbox.as_deref(), false) {
        Ok(env) if fixture.is_some() && sandbox.is_none() => (
            env,
            Some(
                "Changes can't be applied to it. Start Ratewise with --sandbox <dir> to try \
                 them safely."
                    .to_string(),
            ),
        ),
        Ok(env) => (env, None),
        Err(e) => (Env::unavailable(), Some(e.0)),
    };
    send(Reply::ReadOnly(read_only));
    let mut session = Session::new(probes, env);
    for job in rx {
        let reply = match job {
            Job::Refresh { full } => {
                let inputs = if full {
                    session.inputs()
                } else {
                    session.inputs_light()
                };
                Reply::Inputs(Box::new(inputs))
            }
            Job::Execute {
                plan,
                restart,
                revert_id,
            } => Reply::Executed(
                session
                    .execute(&plan, restart, revert_id.as_deref())
                    .map(|o| render_outcome(&plan, &o, &session.env.paths, session.env.sandbox))
                    .map_err(|e| e.0),
            ),
            Job::Doctor => Reply::Doctor(crate::cli::doctor::report(&session.probes)),
        };
        send(reply);
    }
}

fn on_reply(w: &AppWindow, reply: Reply) {
    let mut rerender = true;
    with_state(|st| match reply {
        Reply::ReadOnly(r) => st.read_only = r,
        Reply::Inputs(inputs) => {
            let same = st.inputs.as_ref().is_some_and(|old| {
                old.snapshot == inputs.snapshot
                    && old.saved == inputs.saved
                    && old.latest_backup == inputs.latest_backup
            });
            if same && !st.busy {
                rerender = false;
            }
            if st.selected.is_empty() || !selection_exists(&st.selected, &inputs) {
                st.selected = default_selection(&inputs);
            }
            st.inputs = Some(*inputs);
            st.busy = false;
        }
        Reply::Executed(result) => {
            if let Some(r) = &mut st.review {
                match result {
                    Ok(text) => {
                        r.view.state = "done".into();
                        r.view.title = match r.kind {
                            ReviewKind::Set => "Done".into(),
                            ReviewKind::Revert => "Reverted".into(),
                            ReviewKind::Reset => "Ratewise's settings are removed".into(),
                        };
                        r.view.body = text
                            .replace(
                                "`ratewise revert` undoes this.",
                                "\"Revert last change\" undoes this.",
                            )
                            .into();
                        if r.kind == ReviewKind::Set {
                            st.edits.clear();
                            st.slider_quantum = None;
                        }
                    }
                    Err(e) => {
                        r.view.state = "failed".into();
                        r.view.title = "That didn't work".into();
                        r.view.body = e.into();
                    }
                }
            }
            st.busy = true;
            let _ = st.tx.send(Job::Refresh { full: true });
        }
        Reply::Doctor(text) => st.doctor = text,
    });
    if rerender {
        render(w);
    }
    maybe_shoot(w);
}

fn selection_exists(key: &str, inputs: &Inputs) -> bool {
    match key.strip_prefix("device:") {
        Some(id) => inputs.snapshot.devices.iter().any(|d| d.id == id),
        None => matches!(key, "global" | "doctor"),
    }
}

/// The default output, else the first device, else the global page.
fn default_selection(inputs: &Inputs) -> String {
    let devices = &inputs.snapshot.devices;
    devices
        .iter()
        .find(|d| {
            d.pipewire.as_ref().is_some_and(|p| p.is_default)
                && d.direction != crate::core::model::Direction::Capture
        })
        .or(devices.first())
        .map(|d| format!("device:{}", d.id))
        .unwrap_or_else(|| "global".into())
}

// ---- callbacks ---------------------------------------------------------------------

fn connect(w: &AppWindow) {
    let weak = w.as_weak();
    let rerender = move || {
        if let Some(w) = weak.upgrade() {
            render(&w);
        }
    };

    let r = rerender.clone();
    w.on_select(move |key| {
        with_state(|st| {
            st.selected = key.to_string();
            if st.selected == "doctor" && st.doctor.is_empty() {
                let _ = st.tx.send(Job::Doctor);
            }
        });
        r();
    });

    let r = rerender.clone();
    w.on_refresh(move || {
        with_state(|st| {
            st.busy = true;
            st.doctor.clear();
            let _ = st.tx.send(Job::Refresh { full: true });
            if st.selected == "doctor" {
                let _ = st.tx.send(Job::Doctor);
            }
        });
        r();
    });

    let r = rerender.clone();
    w.on_device_option(move |kind, value| {
        with_state(|st| {
            let Some(id) = st.selected.strip_prefix("device:").map(String::from) else {
                return;
            };
            let Some(change) = edits::device_change(&kind, &value) else {
                return;
            };
            let Some(inputs) = &st.inputs else { return };
            let device = inputs.snapshot.devices.iter().find(|d| d.id == id);
            let clock = inputs.snapshot.clock.as_ref().map(|c| &c.value);
            st.edits.set(
                Edit {
                    change,
                    device: Some(id),
                },
                &inputs.saved,
                clock,
                device,
            );
        });
        r();
    });

    let r = rerender.clone();
    w.on_clock_option(move |kind, value| {
        with_state(|st| {
            let Some(inputs) = &st.inputs else { return };
            let allowed = view::effective_allowed(&Ctx {
                inputs,
                edits: &st.edits,
                read_only: None,
            });
            let Some(change) = edits::global_change(&kind, &value, &allowed) else {
                return;
            };
            let clock = inputs.snapshot.clock.as_ref().map(|c| &c.value);
            st.edits.set(
                Edit {
                    change,
                    device: None,
                },
                &inputs.saved,
                clock,
                None,
            );
            if kind == "quantum" {
                // The slider is where the user left it; snap it to what's now in effect.
                st.slider_quantum = None;
            }
        });
        r();
    });

    let r = rerender.clone();
    w.on_discard(move || {
        with_state(|st| {
            st.edits.clear();
            st.slider_quantum = None;
        });
        r();
    });

    let weak = w.as_weak();
    w.on_apply(move |mode| {
        let mode = if mode == "live" {
            Mode::Live
        } else {
            Mode::Save
        };
        let restart = with_state(|st| open_set_review(st, mode)).unwrap_or(false);
        if let Some(w) = weak.upgrade() {
            w.set_restart_now(restart);
            render(&w);
        }
    });

    let weak = w.as_weak();
    w.on_revert(move || {
        let restart = with_state(|st| open_undo_review(st, ReviewKind::Revert)).unwrap_or(false);
        if let Some(w) = weak.upgrade() {
            w.set_restart_now(restart);
            render(&w);
        }
    });

    let weak = w.as_weak();
    w.on_reset(move || {
        let restart = with_state(|st| open_undo_review(st, ReviewKind::Reset)).unwrap_or(false);
        if let Some(w) = weak.upgrade() {
            w.set_restart_now(restart);
            render(&w);
        }
    });

    let weak = w.as_weak();
    w.on_review_confirm(move || {
        let Some(w) = weak.upgrade() else { return };
        let restart = w.get_restart_now();
        with_state(|st| {
            let Some(r) = &mut st.review else { return };
            let Some(plan) = r.plan.take() else { return };
            r.view.state = "working".into();
            let _ = st.tx.send(Job::Execute {
                plan: Box::new(plan),
                restart: restart && r.view.can_restart,
                revert_id: r.revert_id.clone(),
            });
        });
        render(&w);
    });

    let r = rerender;
    w.on_review_close(move || {
        with_state(|st| st.review = None);
        r();
    });
}

/// Opens the review sheet for the pending edits. Returns whether "restart now" should
/// start ticked.
fn open_set_review(st: &mut State, mode: Mode) -> bool {
    let Some(inputs) = &st.inputs else {
        return false;
    };
    let ctx = Ctx {
        inputs,
        edits: &st.edits,
        read_only: st.read_only.as_deref(),
    };
    let items = view::resolved(&ctx);
    let result = plan::plan_batch(&inputs.ctx(), &items, mode);
    st.review = Some(review_for(
        ReviewKind::Set,
        result.map_err(|e| e.0),
        None,
        inputs,
        mode,
    ));
    st.review.as_ref().is_some_and(|r| r.view.can_restart)
}

fn open_undo_review(st: &mut State, kind: ReviewKind) -> bool {
    let Some(inputs) = &st.inputs else {
        return false;
    };
    let (result, revert_id) = match kind {
        ReviewKind::Revert => match &inputs.latest_backup {
            Some(b) => (
                plan::plan_revert(&inputs.ctx(), b).map_err(|e| e.0),
                Some(b.id.clone()),
            ),
            None => (Err("There's nothing to revert yet.".to_string()), None),
        },
        _ => (plan::plan_reset(&inputs.ctx()).map_err(|e| e.0), None),
    };
    st.review = Some(review_for(kind, result, revert_id, inputs, Mode::Save));
    st.review.as_ref().is_some_and(|r| r.view.can_restart)
}

fn review_for(
    kind: ReviewKind,
    result: Result<Plan, String>,
    revert_id: Option<String>,
    inputs: &Inputs,
    mode: Mode,
) -> Review {
    match result {
        Err(why) => Review {
            kind,
            plan: None,
            revert_id: None,
            view: ReviewView {
                open: true,
                state: "refused".into(),
                title: "Ratewise can't make this change".into(),
                body: why.into(),
                ..Default::default()
            },
        },
        Ok(plan) if plan.is_empty() => Review {
            kind,
            plan: None,
            revert_id: None,
            view: ReviewView {
                open: true,
                state: "done".into(),
                title: "Nothing to change".into(),
                body: plan.notes.join("\n").into(),
                ..Default::default()
            },
        },
        Ok(plan) => {
            let restart = plan.restart.as_ref().filter(|r| !r.commands.is_empty());
            let view = ReviewView {
                open: true,
                state: "plan".into(),
                title: plan.title.as_str().into(),
                body: render_plan_window(&plan, &inputs.paths).into(),
                can_restart: restart.is_some(),
                restart_label: restart
                    .map(|r| {
                        format!(
                            "Restart {} afterwards. Sound stops for a moment.",
                            r.services
                        )
                    })
                    .unwrap_or_default()
                    .into(),
                confirm_label: match (kind, mode) {
                    (ReviewKind::Revert, _) => "Revert",
                    (ReviewKind::Reset, _) => "Remove them",
                    (_, Mode::Live) => "Apply live",
                    (_, Mode::Save) => "Save",
                }
                .into(),
            };
            Review {
                kind,
                plan: Some(plan),
                revert_id,
                view,
            }
        }
    }
}

// ---- rendering ---------------------------------------------------------------------

/// Everything the window shows, built from the state in one go so the state isn't
/// borrowed while Slint runs bindings.
struct Frame {
    nav: Vec<NavItem>,
    selected: String,
    page: String,
    empty_text: String,
    device: Option<DeviceView>,
    clock: ClockView,
    status: String,
    banner: Vec<Fact>,
    pending: view::Pending,
    can_revert: bool,
    revert_label: String,
    review: ReviewView,
    doctor: String,
    busy: bool,
    quantum: Option<u32>,
}

fn build(st: &mut State) -> Frame {
    let review = st
        .review
        .as_ref()
        .map(|r| r.view.clone())
        .unwrap_or_default();
    let Some(inputs) = &st.inputs else {
        return Frame {
            nav: Vec::new(),
            selected: String::new(),
            page: "empty".into(),
            empty_text: "Looking for sound devices…".into(),
            device: None,
            clock: ClockView::default(),
            status: String::new(),
            banner: Vec::new(),
            pending: view::Pending {
                count: 0,
                summary: String::new(),
                can_live: false,
                can_save: false,
                why: String::new(),
            },
            can_revert: false,
            revert_label: String::new(),
            review,
            doctor: st.doctor.clone(),
            busy: st.busy,
            quantum: None,
        };
    };
    let ctx = Ctx {
        inputs,
        edits: &st.edits,
        read_only: st.read_only.as_deref(),
    };
    let device = st
        .selected
        .strip_prefix("device:")
        .and_then(|id| inputs.snapshot.devices.iter().find(|d| d.id == id))
        .map(|d| view::device_view(&ctx, d));
    let page = match st.selected.as_str() {
        "global" => "global",
        "doctor" => "doctor",
        _ if device.is_some() => "device",
        _ => "empty",
    };
    let empty_text = if inputs.snapshot.devices.is_empty() {
        crate::core::model::NO_DEVICES_MESSAGE.to_string()
    } else {
        "Pick a device on the left.".into()
    };
    let quantum = view::effective_quantum(&ctx);
    let set_quantum = (st.slider_quantum != Some(quantum)).then_some(quantum);
    st.slider_quantum = Some(quantum);
    let revert_label = match &inputs.latest_backup {
        Some(b) => format!("Last change: {}", b.description.trim_end_matches('.')),
        None => String::new(),
    };
    let doctor = if st.doctor.is_empty() {
        "Gathering the report…".into()
    } else {
        st.doctor.clone()
    };
    Frame {
        nav: view::nav(inputs),
        selected: st.selected.clone(),
        page: page.into(),
        empty_text,
        device,
        clock: view::clock_view(&ctx),
        status: inputs.snapshot.stack.summary(),
        banner: view::banner(&ctx, st.fixture.as_deref(), &st.errors),
        pending: view::pending(&ctx),
        can_revert: inputs.latest_backup.is_some() && st.read_only.is_none(),
        revert_label,
        review,
        doctor,
        busy: st.busy,
        quantum: set_quantum,
    }
}

fn render(w: &AppWindow) {
    let Some(f) = with_state(build) else { return };
    if let Some(nav) = with_state(|st| st.nav.clone()) {
        sync(&nav, f.nav);
    }
    w.set_selected(f.selected.into());
    w.set_page(f.page.into());
    w.set_empty_text(f.empty_text.into());
    if let Some(d) = f.device {
        w.set_device(d);
    }
    w.set_clock(f.clock);
    if let Some(q) = f.quantum {
        w.set_quantum_index(view::quantum_index(q));
    }
    w.set_status(f.status.into());
    w.set_banner(view::model(f.banner));
    w.set_pending_count(f.pending.count as i32);
    w.set_pending_summary(f.pending.summary.into());
    w.set_can_live(f.pending.can_live);
    w.set_can_save(f.pending.can_save);
    w.set_apply_why(f.pending.why.into());
    w.set_can_revert(f.can_revert);
    w.set_revert_label(f.revert_label.into());
    w.set_review(f.review);
    w.set_doctor_text(f.doctor.into());
    w.set_busy(f.busy);
}

/// Updates a model in place, so the sidebar keeps its scroll position and focus.
fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, items: Vec<T>) {
    if model.row_count() != items.len() {
        model.set_vec(items);
        return;
    }
    for (i, item) in items.into_iter().enumerate() {
        if model.row_data(i).as_ref() != Some(&item) {
            model.set_row_data(i, item);
        }
    }
}

// ---- screenshots and self-test -----------------------------------------------------

/// Once data has arrived, picks the page `RATEWISE_PAGE` asks for and takes the shot.
fn maybe_shoot(w: &AppWindow) {
    let start = with_state(|st| match &mut st.shot {
        Some(shot) if !shot.started && st.inputs.is_some() => {
            shot.started = true;
            true
        }
        _ => false,
    })
    .unwrap_or(false);
    if !start {
        return;
    }
    let page = std::env::var("RATEWISE_PAGE").unwrap_or_default();
    // review:<kind>=<value> opens the review sheet for a global edit, e.g.
    // review:quantum=256; apply:<kind>=<value> also confirms it (use --sandbox).
    let (review, confirm) = match (page.strip_prefix("review:"), page.strip_prefix("apply:")) {
        (Some(e), _) => (Some(e), false),
        (_, Some(e)) => (Some(e), true),
        _ => (None, false),
    };
    with_state(|st| {
        if let Some(edit) = review {
            if let Some((kind, value)) = edit.split_once('=')
                && let Some(inputs) = &st.inputs
            {
                let allowed = view::effective_allowed(&Ctx {
                    inputs,
                    edits: &st.edits,
                    read_only: None,
                });
                if let Some(change) = edits::global_change(kind, value, &allowed) {
                    st.edits.set(
                        Edit {
                            change,
                            device: None,
                        },
                        &inputs.saved,
                        None,
                        None,
                    );
                }
            }
            st.selected = "global".into();
            open_set_review(st, Mode::Save);
        } else if !page.is_empty() {
            st.selected = page.clone();
            if page == "doctor" {
                let _ = st.tx.send(Job::Doctor);
            }
        }
    });
    if let Some(tab) = std::env::var("RATEWISE_TAB")
        .ok()
        .and_then(|t| t.parse().ok())
    {
        w.set_device_tab(tab);
    }
    render(w);
    if confirm {
        w.set_restart_now(true);
        w.invoke_review_confirm();
    }
    let weak = w.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1500), move || {
        if let Some(w) = weak.upgrade() {
            take_shot(&w);
        }
    });
}

fn take_shot(w: &AppWindow) {
    let Some((path, self_test)) =
        with_state(|st| st.shot.as_ref().map(|s| (s.path.clone(), s.self_test))).flatten()
    else {
        return;
    };
    let result = w
        .window()
        .take_snapshot()
        .map_err(|e| format!("Couldn't render the window: {e}."))
        .and_then(|img| {
            write_png(&path, img.width(), img.height(), img.as_bytes())?;
            let first = img.as_slice().first().copied();
            let varied = img.as_slice().iter().any(|p| Some(*p) != first);
            if self_test && !varied {
                return Err(format!(
                    "The window rendered as a single colour ({}). Something is wrong with the \
                     renderer; try RATEWISE_RENDERER=software.",
                    path.display()
                ));
            }
            Ok(format!(
                "{} {}×{} to {}.",
                if self_test {
                    "Self-test passed: rendered"
                } else {
                    "Rendered"
                },
                img.width(),
                img.height(),
                path.display()
            ))
        });
    with_state(|st| st.shot_result = Some(result));
    let _ = slint::quit_event_loop();
}

fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("Couldn't write {}: {e}.", path.display());
    let file = std::fs::File::create(path).map_err(|e| fail(&e))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| fail(&e))?;
    writer.write_image_data(rgba).map_err(|e| fail(&e))
}
