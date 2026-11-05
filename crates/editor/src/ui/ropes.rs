use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::Value;
use slint::{ComponentHandle, Model as _, ModelRc, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation, DocumentLifecycle};
use crate::session::ropes::{Command, Settings};
use crate::session::{Actor, Session};
use crate::{RopePreviewRow, RopePreviewWindow};

const REPORT_INTERVAL: Duration = Duration::from_millis(125);

#[derive(Default)]
struct Form {
    snapshot: String,
    seen: Option<(String, u64)>,
    refreshed: Option<Instant>,
    selected: String,
    report: Value,
    row_key: Vec<(String, String, String)>,
}

pub(super) struct Controller {
    pub(super) window: RopePreviewWindow,
    form: Rc<RefCell<Form>>,
}

#[derive(Clone, Copy)]
enum Action {
    Playback,
    Reset,
    Seek,
    Scrub(f64),
    Environment,
    Cancel,
}

fn number(text: &str, label: &str) -> Result<f64, String> {
    text.trim()
        .parse()
        .map_err(|_| format!("{label} must be a number"))
}

fn command(window: &RopePreviewWindow, action: Action) -> Result<Command, String> {
    Ok(match action {
        Action::Playback if window.get_playing() => Command::Pause,
        Action::Playback if window.get_ready() => Command::Play,
        Action::Playback => Command::Prepare,
        Action::Reset => Command::Reset,
        Action::Seek => Command::Seek {
            time_seconds: number(&window.get_seek_time(), "Seek time")?,
        },
        Action::Scrub(time_seconds) => Command::Seek { time_seconds },
        Action::Environment => Command::Settings(Settings {
            gravity: number(&window.get_gravity(), "Gravity")?,
            wind: [
                number(&window.get_wind_x(), "Wind X")?,
                number(&window.get_wind_y(), "Wind Y")?,
                number(&window.get_wind_z(), "Wind Z")?,
            ],
        }),
        Action::Cancel => Command::Cancel,
    })
}

fn status_label(status: &str) -> &str {
    match status {
        "not_prepared" => "Not prepared",
        "preparing" => "Preparing",
        "ready" => "Paused",
        "advancing" => "Advancing",
        "seeking" => "Seeking",
        "resetting" => "Resetting",
        "verifying" => "Checking sources",
        "cancelling" => "Cancelling",
        "cancelled" => "Cancelled",
        "pausing" => "Pausing",
        "failed" => "Failed",
        "stale" => "Source changed",
        "closed" => "Document closed",
        _ => status,
    }
}

fn unavailable(window: &RopePreviewWindow, error: &str) {
    window.set_ready(false);
    window.set_published(false);
    window.set_playing(false);
    window.set_busy(false);
    window.set_can_control(false);
    window.set_can_play(false);
    window.set_can_cancel(false);
    window.set_summary("Rope preview is unavailable".into());
    window.set_rows(ModelRc::new(VecModel::<RopePreviewRow>::default()));
    window.set_selected_id("".into());
    window.set_empty_text("Refresh to inspect the current rope preview.".into());
    window.set_detail_title("Preview details".into());
    window.set_detail("".into());
    window.set_material_status("".into());
    window.set_attachment_status("".into());
    window.set_physics_status("".into());
    window.set_scope("".into());
    window.set_issues("".into());
    window.set_has_issues(true);
    window.set_playback_status("".into());
    window.set_time_label("Pose unavailable".into());
    window.set_time_limits("".into());
    window.set_environment_status("Settings unavailable".into());
    if !window.get_environment_dirty() {
        window.set_gravity("".into());
        window.set_wind_x("".into());
        window.set_wind_y("".into());
        window.set_wind_z("".into());
    }
    window.set_message(error.into());
}

fn target_label(link: &Value) -> &str {
    match link.get("target") {
        Some(Value::String(target)) => target,
        Some(Value::Null) => "World-origin end seed",
        _ => "Target not reported",
    }
}

fn material_text(link: &Value) -> String {
    let report = &link["material_status"];
    if !report.is_object() {
        return "No admitted material report".into();
    }
    let mut lines = vec![
        report["material"]
            .as_str()
            .unwrap_or("Material not reported")
            .to_owned(),
        format!(
            "{} · {}",
            report["shader"].as_str().unwrap_or("Shader not reported"),
            report["texture"].as_str().unwrap_or("Texture not reported")
        ),
        format!(
            "{} × {} · {} · opacity {}",
            report["dimensions"][0],
            report["dimensions"][1],
            if report["alpha_test"] == true {
                "Alpha test"
            } else if report["translucent"] == true {
                "Translucent"
            } else {
                "Opaque"
            },
            report["opacity"]
        ),
    ];
    let excluded = report["not_rendered"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if !excluded.is_empty() {
        lines.push(format!("Not rendered: {}", excluded.join(", ")));
    }
    lines.join("\n")
}

fn coordinates(value: &Value) -> Option<String> {
    let values = value.as_array()?;
    let xyz = [
        values.first()?.as_f64()?,
        values.get(1)?.as_f64()?,
        values.get(2)?.as_f64()?,
    ];
    Some(format!("{:.2}, {:.2}, {:.2}", xyz[0], xyz[1], xyz[2]))
}

fn attachment_text(link: &Value) -> String {
    let mut lines = vec![
        link["attachment_status"]
            .as_str()
            .unwrap_or("Attachment status has not been reported")
            .to_owned(),
    ];
    let anchors = &link["anchors"];
    if let Some(position) = coordinates(&anchors["start"]) {
        lines.push(format!("Resolved start: {position}"));
    }
    if anchors["binding"] == "native_absent_end_world_zero" {
        lines.push("Initial end seed: world origin".into());
    } else if let Some(position) = coordinates(&anchors["end"]) {
        lines.push(format!("Resolved end: {position}"));
    }
    lines.join("\n")
}

fn physics_text(link: &Value) -> String {
    let mut lines = vec![
        link["physics_status"]
            .as_str()
            .unwrap_or("Physics status has not been reported")
            .to_owned(),
    ];
    if let Some(nodes) = link["physical_nodes"].as_u64() {
        lines.push(format!(
            "{nodes} physical nodes · subdivision {}",
            link["subdiv"]
        ));
    }
    if let Some(collide) = link["collision"].as_bool() {
        lines.push(format!(
            "World collision: {}",
            if collide { "enabled" } else { "disabled" }
        ));
    }
    if let Some(no_wind) = link["no_wind"].as_bool() {
        lines.push(format!(
            "Authored wind: {}",
            if no_wind { "disabled" } else { "allowed" }
        ));
    }
    let inert = link["inert_keys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if !inert.is_empty() {
        lines.push(format!("Inert authored fields: {}", inert.join(", ")));
    }
    lines.join("\n")
}

fn details(window: &RopePreviewWindow, form: &Form) {
    let report = &form.report;
    let link = report["links"]
        .as_array()
        .and_then(|links| links.iter().find(|link| link["source"] == form.selected));
    let Some(link) = link else {
        window.set_detail_title("Preview details".into());
        window.set_detail("Choose a span to inspect its reported state.".into());
        window.set_material_status("".into());
        window.set_attachment_status("".into());
        window.set_physics_status("".into());
        return;
    };
    window.set_detail_title(
        format!(
            "{} → {}",
            link["source"].as_str().unwrap_or("Unavailable source"),
            target_label(link)
        )
        .into(),
    );
    let mut description = Vec::new();
    for (key, label) in [("width", "Width"), ("slack", "Slack"), ("length", "Length")] {
        if let Some(value) = link[key].as_f64() {
            description.push(format!("{label} {value:.2}"));
        }
    }
    window.set_detail(description.join(" · ").into());
    window.set_material_status(material_text(link).into());
    window.set_attachment_status(attachment_text(link).into());
    window.set_physics_status(physics_text(link).into());
}

fn present(
    window: &RopePreviewWindow,
    form: &RefCell<Form>,
    session: &mut Session,
) -> Result<(), String> {
    let report = session.rope_preview_state();
    let clock = session.rope_preview_clock();
    let snapshot = report["snapshot"]
        .as_str()
        .ok_or("Rope preview state has no source snapshot")?;
    let summary = report["summary"]
        .as_str()
        .ok_or("Rope preview state has no summary")?;
    let links = report["links"]
        .as_array()
        .ok_or("Rope preview state has no span report")?;
    let settings = &report["settings"];
    let gravity = settings["gravity"]
        .as_f64()
        .ok_or("Rope preview state has no gravity setting")?;
    let wind = [
        settings["wind"][0]
            .as_f64()
            .ok_or("Rope preview state has no wind X setting")?,
        settings["wind"][1]
            .as_f64()
            .ok_or("Rope preview state has no wind Y setting")?,
        settings["wind"][2]
            .as_f64()
            .ok_or("Rope preview state has no wind Z setting")?,
    ];
    let mut form = form.borrow_mut();
    let rows = links
        .iter()
        .map(|link| {
            let source = link["source"].as_str().unwrap_or("Unavailable source");
            let target = target_label(link);
            RopePreviewRow {
                id: source.into(),
                label: format!("{source} → {target}").into(),
                detail: link["material"]
                    .as_str()
                    .unwrap_or("Inspect span details")
                    .into(),
            }
        })
        .collect::<Vec<_>>();
    if !rows
        .iter()
        .any(|row| row.id.as_str() == form.selected.as_str())
    {
        form.selected = rows
            .first()
            .map_or_else(String::new, |row| row.id.to_string());
    }
    window.set_document_label(session.title().into());
    let issue_count = report["issues"].as_array().map_or(0, Vec::len);
    let omitted = report["omitted_nodes"].as_u64().unwrap_or(0);
    let mut summary = summary.to_owned();
    if issue_count > 0 {
        summary.push_str(&format!(" · {issue_count} issues"));
    }
    if omitted > 0 {
        summary.push_str(&format!(" · {omitted} omitted nodes"));
    }
    window.set_summary(summary.into());
    window.set_has_issues(issue_count > 0 || omitted > 0 || !report["error"].is_null());
    window.set_preview_enabled(report["enabled"] == true);
    window.set_ready(report["ready"] == true);
    window.set_published(report["published"] == true);
    window.set_playing(clock.playing);
    window.set_busy(clock.busy);
    window.set_can_control(true);
    window.set_can_play(report["ready"] == true && report["enabled"] == true);
    window.set_can_cancel(clock.busy);
    window.set_empty_text(
        if clock.status == "failed" {
            "No current pose. Inspect the reported issues."
        } else if report["enabled"] != true {
            "Enable rope preview, then prepare a pose."
        } else if report["ready"] != true {
            "Prepare preview to inspect admitted rope spans."
        } else {
            "No admitted rope spans."
        }
        .into(),
    );
    window.set_max_time_seconds(
        report["max_time_seconds"]
            .as_f64()
            .ok_or("Rope preview state has no time bound")? as f32,
    );
    window.set_time_label(
        if let Some(seconds) = report["published_time_seconds"].as_f64() {
            format!("Current pose {seconds:.2} s")
        } else {
            "No prepared pose".to_owned()
        }
        .into(),
    );
    let mut time_limits = format!("0–{} s", report["max_time_seconds"]);
    if let Some(step) = report["limits"]["fixed_step_seconds"]
        .as_f64()
        .filter(|step| *step > 0.0)
    {
        time_limits.push_str(&format!(" · {:.0} Hz", step.recip()));
    }
    window.set_time_limits(time_limits.into());
    let mut playback = if clock.busy {
        format!(
            "{} · target {:.2} s",
            status_label(clock.status),
            clock.requested_time_seconds
        )
    } else if clock.playing {
        "Playing".into()
    } else {
        status_label(clock.status).to_owned()
    };
    if clock.busy
        && let Some(actor) = report["worker_actor"].as_str()
    {
        playback.push_str(match actor {
            "user" => " · User",
            "agent" => " · Agent",
            "system" => " · System",
            _ => " · actor unavailable",
        });
    }
    window.set_playback_status(playback.into());
    if !window.get_scrub_dirty() {
        window.set_time_value(clock.time_seconds as f32);
    }
    if !window.get_time_dirty() {
        window.set_seek_time(clock.requested_time_seconds.to_string().into());
    }
    if !window.get_environment_dirty() {
        window.set_gravity(gravity.to_string().into());
        for (index, setter) in [
            RopePreviewWindow::set_wind_x,
            RopePreviewWindow::set_wind_y,
            RopePreviewWindow::set_wind_z,
        ]
        .into_iter()
        .enumerate()
        {
            setter(window, wind[index].to_string().into());
        }
    }
    window.set_environment_status("Editor acceleration · native env_wind is not evaluated".into());
    window.set_scope(report["motion"].as_str().unwrap_or_default().into());
    let mut issues = report["issues"]
        .as_array()
        .map(|issues| {
            issues
                .iter()
                .map(|issue| {
                    format!(
                        "{} · {}",
                        issue["entity"].as_str().unwrap_or("Rope preview"),
                        issue["message"]
                            .as_str()
                            .unwrap_or("Unavailable diagnostic")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default();
    if omitted > 0 {
        if !issues.is_empty() {
            issues.push_str("\n\n");
        }
        issues.push_str(&format!("{omitted} rope nodes exceed the preview limit."));
    }
    window.set_issues(issues.into());
    if let Some(error) = report["error"].as_str() {
        window.set_message(error.into());
    }
    let row_key = rows
        .iter()
        .map(|row| {
            (
                row.id.to_string(),
                row.label.to_string(),
                row.detail.to_string(),
            )
        })
        .collect::<Vec<_>>();
    if form.row_key != row_key || window.get_rows().row_count() != rows.len() {
        window.set_rows(ModelRc::new(VecModel::from(rows)));
        form.row_key = row_key;
    }
    window.set_selected_id(form.selected.clone().into());
    form.snapshot = snapshot.to_owned();
    form.seen = Some((clock.snapshot, clock.serial));
    form.refreshed = Some(Instant::now());
    form.report = report;
    details(window, &form);
    Ok(())
}

fn refresh(
    window: &RopePreviewWindow,
    form: &RefCell<Form>,
    target: &DocumentContinuation,
) -> Result<(), String> {
    target
        .upgrade()?
        .with_session(|session| present(window, form, session))
}

fn run(
    window: &RopePreviewWindow,
    form: &RefCell<Form>,
    target: &DocumentContinuation,
    action: Action,
) -> bool {
    let result = command(window, action).and_then(|command| {
        target
            .upgrade()?
            .rope_preview(Actor::User, &form.borrow().snapshot, command, true)
    });
    if result.is_ok() {
        window.set_message("".into());
        if matches!(action, Action::Reset | Action::Seek | Action::Scrub(_)) {
            window.set_time_dirty(false);
            window.set_scrub_dirty(false);
        }
        if matches!(action, Action::Environment) {
            window.set_environment_dirty(false);
        }
    }
    if let Err(error) = refresh(window, form, target) {
        unavailable(window, &error);
        return false;
    }
    match result {
        Ok(_) => true,
        Err(error) => {
            window.set_message(error.into());
            false
        }
    }
}

fn retired(access: &DocumentAccess) -> bool {
    access
        .document_lifecycle()
        .is_some_and(|lifecycle| lifecycle != DocumentLifecycle::Live)
}

fn hide(window: &RopePreviewWindow, context: &str) {
    if let Err(error) = window.hide() {
        window.set_message(format!("{context}: {error}").into());
    }
}

fn close(window: &RopePreviewWindow, form: &RefCell<Form>, target: &DocumentContinuation) {
    let Ok(access) = target.upgrade() else {
        hide(window, "Could not hide the closed document's rope preview");
        return;
    };
    if retired(&access) {
        hide(window, "Could not hide the closed document's rope preview");
        return;
    }
    let result = access
        .with_session(|session| {
            let _ = session.rope_preview_state();
            Ok(session.rope_preview_clock().snapshot)
        })
        .and_then(|snapshot| access.rope_preview(Actor::User, &snapshot, Command::Pause, true));
    match result {
        Ok(_) => {
            if let Err(error) = refresh(window, form, target) {
                unavailable(window, &error);
            }
            hide(
                window,
                "Playback paused, but the window could not be hidden",
            );
        }
        Err(_) if retired(&access) => {
            hide(window, "Could not hide the closed document's rope preview");
        }
        Err(error) => {
            if let Err(refresh_error) = refresh(window, form, target) {
                unavailable(window, &refresh_error);
            }
            window.set_message(error.into());
        }
    }
}

impl Controller {
    pub(super) fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = RopePreviewWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        let callback = |action| {
            let (window, form, target) = (window.as_weak(), form.clone(), access.downgrade());
            move || {
                if let Some(window) = window.upgrade() {
                    run(&window, &form, &target, action);
                }
            }
        };
        window.on_play_pause(callback(Action::Playback));
        window.on_reset(callback(Action::Reset));
        window.on_seek(callback(Action::Seek));
        window.on_apply_environment(callback(Action::Environment));
        window.on_cancel_work(callback(Action::Cancel));
        window.on_scrub({
            let (window, form, target) = (window.as_weak(), form.clone(), access.downgrade());
            move |seconds| {
                if let Some(window) = window.upgrade() {
                    run(&window, &form, &target, Action::Scrub(f64::from(seconds)));
                }
            }
        });
        window.on_enable_preview({
            let (window, form, target) = (window.as_weak(), form.clone(), access.downgrade());
            move |enabled| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                let result = target.upgrade().and_then(|access| {
                    access.with_session(|session| {
                        if session.rope_preview_clock().snapshot != form.borrow().snapshot {
                            return Err(
                                "Rope preview changed. Refresh before changing visibility.".into(),
                            );
                        }
                        let mut view = session.view();
                        view.rope_preview = enabled;
                        session.set_view(view)
                    })
                });
                if result.is_ok() {
                    window.set_message("".into());
                }
                if let Err(error) = refresh(&window, &form, &target) {
                    unavailable(&window, &error);
                } else if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_choose({
            let (window, form) = (window.as_weak(), form.clone());
            move |id| {
                if let Some(window) = window.upgrade() {
                    form.borrow_mut().selected = id.to_string();
                    window.set_selected_id(id);
                    details(&window, &form.borrow());
                }
            }
        });
        window.on_refresh({
            let (window, form, target) = (window.as_weak(), form.clone(), access.downgrade());
            move || {
                if let Some(window) = window.upgrade() {
                    window.set_message("".into());
                    match refresh(&window, &form, &target) {
                        Ok(()) => {}
                        Err(error) => unavailable(&window, &error),
                    }
                }
            }
        });
        window.on_close_dialog({
            let (window, form, target) = (window.as_weak(), form.clone(), access.downgrade());
            move || {
                if let Some(window) = window.upgrade() {
                    close(&window, &form, &target);
                }
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_close_dialog();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        Ok(Self { window, form })
    }

    pub(super) fn show(&self, session: &mut Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("".into());
        if let Err(error) = present(&self.window, &self.form, session) {
            unavailable(&self.window, &error);
        }
        self.window.show()
    }

    pub(super) fn refresh(&self, session: &mut Session) {
        if !self.window.window().is_visible() {
            return;
        }
        let clock = session.rope_preview_clock();
        {
            let form = self.form.borrow();
            if form.seen.as_ref() == Some(&(clock.snapshot.clone(), clock.serial))
                || (!matches!(clock.status, "failed" | "stale")
                    && form
                        .refreshed
                        .is_some_and(|at| at.elapsed() < REPORT_INTERVAL))
            {
                return;
            }
        }
        if let Err(error) = present(&self.window, &self.form, session) {
            unavailable(&self.window, &error);
        }
    }

    pub(super) fn set_active(&self, active: bool) {
        self.window.set_document_active(active);
        self.form.borrow_mut().refreshed = None;
    }
}
