use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use slint::ComponentHandle;

use crate::{
    SoundBrowserWindow, SoundChoiceRow,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
};

const PAGE_SIZE: usize = 60;
type Choice = (u64, String, usize);

#[derive(Default)]
struct Catalog {
    groups: Vec<String>,
    offset: usize,
}

pub struct Controller {
    pub window: SoundBrowserWindow,
    token: Rc<Cell<Option<u64>>>,
    seen: Rc<RefCell<Option<Choice>>>,
    catalog: Rc<RefCell<Catalog>>,
    owned_playback: Rc<Cell<Option<u64>>>,
    displayed_playback: Rc<Cell<Option<u64>>>,
}

fn choice(report: &serde_json::Value) -> Option<Choice> {
    let sound = &report["sound"];
    (!sound.is_null()).then(|| {
        (
            report["token"].as_u64().unwrap(),
            sound["value"].as_str().unwrap().into(),
            sound["variant"].as_u64().unwrap() as usize,
        )
    })
}

fn present(
    window: &SoundBrowserWindow,
    report: &serde_json::Value,
    seen: &RefCell<Option<Choice>>,
) {
    let Some(key) = choice(report) else {
        return;
    };
    let sound = &report["sound"];
    window.set_value(key.1.clone().into());
    window.set_variant(key.2 as i32);
    window.set_waves(slint::ModelRc::new(slint::VecModel::from(
        sound["waves"]
            .as_array()
            .unwrap()
            .iter()
            .map(|wave| wave.as_str().unwrap().into())
            .collect::<Vec<slint::SharedString>>(),
    )));
    let mut details = vec![sound["codec"].as_str().unwrap().to_owned()];
    if let Some(channels) = sound["channels"].as_u64() {
        details.push(match channels {
            1 => "Mono".into(),
            2 => "Stereo".into(),
            _ => format!("{channels} channels"),
        });
    }
    if let Some(rate) = sound["sample_rate"].as_u64() {
        details.push(format!("{:.1} kHz", rate as f64 / 1000.0));
    }
    if let Some(duration) = sound["duration"].as_f64() {
        details.push(format!("{duration:.2} s"));
    }
    details.push(format!(
        "{:.1} KiB",
        sound["encoded_bytes"].as_u64().unwrap() as f64 / 1024.0
    ));
    window.set_details(details.join(" · ").into());
    let script = [
        ("source", "Script"),
        ("channel", "Game channel"),
        ("volume", "Game volume"),
        ("pitch", "Game pitch"),
        ("sound_level", "Game level"),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        sound[key]
            .as_str()
            .map(|value| format!("{label} · {value}"))
    })
    .collect::<Vec<_>>()
    .join("\n");
    window.set_script_info(script.into());
    window.set_can_apply(true);
    window.set_message(if sound["kind"]=="script" {"Use sound applies the script name. The audition variant is a preview choice; the game selects its own variant.".into()}else{"Inspect and Play leave the map unchanged. Use sound applies this file to the captured field.".into()});
    *seen.borrow_mut() = Some(key);
}

fn inspect(
    window: &SoundBrowserWindow,
    session: &mut Session,
    token: Option<u64>,
    seen: &RefCell<Option<Choice>>,
) {
    let result = token
        .ok_or("Reopen the sound browser".to_owned())
        .and_then(|token| {
            session.choose_sound(
                token,
                &window.get_value(),
                window.get_variant().max(0) as usize,
            )
        });
    match result {
        Ok(report) => present(window, &report, seen),
        Err(error) => {
            window.set_can_apply(false);
            window.set_details("".into());
            window.set_script_info("".into());
            window.set_waves(Default::default());
            window.set_message(error.into());
        }
    }
}

fn catalog(window: &SoundBrowserWindow, session: &mut Session, state: &mut Catalog) {
    let group = state
        .groups
        .get(window.get_group_index().max(0) as usize)
        .map_or("all", String::as_str)
        .to_owned();
    let result = session.browse_sounds(&window.get_query(), &group, state.offset, PAGE_SIZE);
    let report = match result {
        Ok(report) => report,
        Err(error) => {
            window.set_rows(slint::ModelRc::new(slint::VecModel::from(Vec::<
                SoundChoiceRow,
            >::new(
            ))));
            window.set_can_previous(false);
            window.set_can_next(false);
            window.set_page_info("Sound catalog unavailable".into());
            window.set_message(error.into());
            return;
        }
    };
    state.groups = report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().into())
        .collect();
    window.set_groups(slint::ModelRc::new(slint::VecModel::from(
        report["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| format!("{} ({})", row["label"].as_str().unwrap(), row["count"]).into())
            .collect::<Vec<slint::SharedString>>(),
    )));
    let index = state
        .groups
        .iter()
        .position(|candidate| candidate == &group)
        .unwrap_or(0);
    window.set_group_index(index as i32);
    if state.groups[index] != group {
        state.offset = 0;
        catalog(window, session, state);
        return;
    }
    window.set_rows(slint::ModelRc::new(slint::VecModel::from(
        report["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                let value = row["value"].as_str().unwrap();
                SoundChoiceRow {
                    value: value.into(),
                    name: if row["kind"] == "file" {
                        value.rsplit('/').next().unwrap_or(value).into()
                    } else {
                        value.into()
                    },
                    detail: row["source"].as_str().unwrap_or(value).into(),
                }
            })
            .collect::<Vec<_>>(),
    )));
    let total = report["total"].as_u64().unwrap() as usize;
    window.set_can_previous(state.offset > 0);
    window.set_can_next(state.offset.saturating_add(PAGE_SIZE) < total);
    window.set_page_info(if total == 0 {
        "No sounds".into()
    } else {
        format!(
            "{}–{} of {total}",
            state.offset + 1,
            (state.offset + PAGE_SIZE).min(total)
        )
        .into()
    });
    if let Some(warning) = report["warnings"]
        .as_array()
        .unwrap()
        .first()
        .and_then(|warning| warning.as_str())
    {
        window.set_message(warning.into());
    }
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = SoundBrowserWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(960.0, 700.0));
        let token = Rc::new(Cell::new(None));
        let seen = Rc::new(RefCell::new(None));
        let catalog_state = Rc::new(RefCell::new(Catalog::default()));
        let owned_playback = Rc::new(Cell::new(None::<u64>));
        let displayed_playback = Rc::new(Cell::new(None::<u64>));
        window.on_search({
            let (window, shared, state) = (window.as_weak(), shared.clone(), catalog_state.clone());
            move || {
                let mut state = state.borrow_mut();
                state.offset = 0;
                catalog(&window.unwrap(), &mut lock(&shared), &mut state);
            }
        });
        window.on_group_changed({
            let window = window.as_weak();
            move || window.unwrap().invoke_search()
        });
        window.on_page({
            let (window, shared, state) = (window.as_weak(), shared.clone(), catalog_state.clone());
            move |direction| {
                let mut state = state.borrow_mut();
                state.offset = if direction < 0 {
                    state.offset.saturating_sub(PAGE_SIZE)
                } else {
                    state.offset + PAGE_SIZE
                };
                catalog(&window.unwrap(), &mut lock(&shared), &mut state);
            }
        });
        window.on_inspect({
            let (window, shared, token, seen) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                seen.clone(),
            );
            move || inspect(&window.unwrap(), &mut lock(&shared), token.get(), &seen)
        });
        window.on_variant_changed({
            let window = window.as_weak();
            move || window.unwrap().invoke_inspect()
        });
        window.on_choose({
            let window = window.as_weak();
            move |value| {
                let window = window.unwrap();
                window.set_value(value);
                window.set_variant(0);
                window.invoke_inspect();
            }
        });
        window.on_invalidate({
            let window = window.as_weak();
            move || {
                let window = window.unwrap();
                window.set_can_apply(false);
                window.set_message("Inspect the sound before applying it.".into());
            }
        });
        window.on_volume_changed({
            let (window, shared) = (window.as_weak(), shared.clone());
            move || {
                let window = window.unwrap();
                if let Err(error) =
                    lock(&shared).set_sound_preview_volume(window.get_volume() / 100.0)
                {
                    window.set_message(error.into());
                }
            }
        });
        window.on_play({
            let (window, shared, owned) =
                (window.as_weak(), shared.clone(), owned_playback.clone());
            move || {
                let window = window.unwrap();
                match lock(&shared).play_sound_preview(
                    &window.get_value(),
                    window.get_variant().max(0) as usize,
                    None,
                ) {
                    Ok(report) => owned.set(report["token"].as_u64()),
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_stop({
            let (window, shared, displayed) =
                (window.as_weak(), shared.clone(), displayed_playback.clone());
            move || {
                if let Some(token) = displayed.take()
                    && let Err(error) = lock(&shared).stop_sound_preview(token)
                {
                    window.unwrap().set_message(error.into());
                }
            }
        });
        window.on_reload({
            let (window, shared) = (window.as_weak(), shared.clone());
            move || {
                lock(&shared).reload_sound_assets();
                window
                    .unwrap()
                    .set_message("Sounds reloaded. Cancel and reopen this choice.".into());
            }
        });
        window.on_apply({
            let (window, shared, token, seen) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                seen.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = token
                    .get()
                    .ok_or("Reopen the sound browser".to_owned())
                    .and_then(|token| {
                        let mut session = lock(&shared);
                        let report = session.sound_choice(token)?;
                        if *seen.borrow() != choice(&report) {
                            present(&window, &report, &seen);
                            return Ok(false);
                        }
                        session.apply_sound_choice(Actor::User, token)?;
                        Ok(true)
                    });
                match result {
                    Ok(true) => window.invoke_cancel(),
                    Ok(false) => window
                        .set_message("Sound choice changed. Review it before Use sound.".into()),
                    Err(error) => {
                        window.set_can_apply(false);
                        window.set_message(error.into());
                    }
                }
            }
        });
        window.on_cancel({
            let (window, shared, token, owned) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                owned_playback.clone(),
            );
            move || {
                let mut session = lock(&shared);
                if let Some(token) = token.take() {
                    let _ = session.cancel_sound_choice(token);
                }
                if let Some(token) = owned.take() {
                    let _ = session.stop_sound_preview(token);
                }
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_cancel();
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self {
            window,
            token,
            seen,
            catalog: catalog_state,
            owned_playback,
            displayed_playback,
        })
    }

    pub fn initialize(&self, session: &mut Session, id: u32, key: &str) -> Result<(), String> {
        if let Some(token) = self.owned_playback.take() {
            let _ = session.stop_sound_preview(token);
        }
        let report = session.begin_sound_choice(id, key, true)?;
        self.token.set(report["token"].as_u64());
        *self.seen.borrow_mut() = None;
        self.window.set_target(
            format!(
                "{} · {}",
                report["title"].as_str().unwrap(),
                report["label"].as_str().unwrap()
            )
            .into(),
        );
        self.window.set_query("".into());
        self.window.set_group_index(0);
        *self.catalog.borrow_mut() = Catalog::default();
        self.window
            .set_value(report["value"].as_str().unwrap_or("").into());
        self.window.set_variant(0);
        self.window.set_waves(Default::default());
        self.window.set_details("".into());
        self.window.set_script_info("".into());
        self.window.set_can_apply(false);
        self.window
            .set_message("Choose a sound; nothing plays until Play.".into());
        catalog(&self.window, session, &mut self.catalog.borrow_mut());
        if !report["sound"].is_null() {
            present(&self.window, &report, &self.seen);
        } else if !self.window.get_value().is_empty() {
            inspect(&self.window, session, self.token.get(), &self.seen);
        }
        self.refresh_playback(session);
        Ok(())
    }

    pub fn refresh(&self, session: &mut Session) {
        self.window
            .set_volume(session.sound_preview_volume() * 100.0);
        if let Some(token) = self.token.get() {
            match session.sound_choice(token) {
                Ok(report) => {
                    if *self.seen.borrow() != choice(&report) {
                        present(&self.window, &report, &self.seen);
                    }
                }
                Err(error) => {
                    self.window.set_can_apply(false);
                    self.window.set_message(error.into());
                }
            }
        }
    }

    pub fn refresh_playback(&self, session: &mut Session) {
        let report = session.sound_preview_status();
        self.displayed_playback.set(report["token"].as_u64());
        self.window.set_can_stop(!report["token"].is_null());
        self.window
            .set_playback(match report["state"].as_str().unwrap() {
                "idle" => "Not playing".into(),
                state => format!(
                    "{} · {}",
                    state,
                    report["sound"]["value"].as_str().unwrap_or("")
                )
                .into(),
            });
        self.window
            .set_volume(session.sound_preview_volume() * 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use std::sync::{Arc, Mutex};

    #[test]
    fn sound_browser_shares_named_variants_handoff_apply_and_cancel() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (_directory, session, id) = crate::test_assets::sounds();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let token = lock(&shared)
            .begin_sound_choice(id, "message", false)
            .unwrap()["token"]
            .as_u64()
            .unwrap();
        lock(&shared).choose_sound(token, "Test.Rain", 0).unwrap();
        let controller = Controller::new(&shared).unwrap();
        controller
            .initialize(&mut lock(&shared), id, "message")
            .unwrap();
        assert_eq!(controller.token.get(), Some(token));
        assert!(controller.window.get_target().contains("Sound Name"));
        controller.window.set_variant(1);
        controller.window.invoke_variant_changed();
        assert_eq!(
            lock(&shared).sound_choice(token).unwrap()["sound"]["variant"],
            1
        );
        controller.window.set_volume(25.0);
        controller.window.invoke_volume_changed();
        assert_eq!(lock(&shared).sound_preview_volume(), 0.25);
        controller.window.set_query("rain".into());
        controller.window.invoke_search();
        assert_eq!(controller.window.get_rows().row_count(), 1);
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "sound-browser");
        lock(&shared)
            .choose_sound(token, "ambient/one.wav", 0)
            .unwrap();
        controller.window.invoke_apply();
        assert!(controller.window.window().is_visible());
        assert_eq!(controller.window.get_value(), "ambient/one.wav");
        assert_eq!(lock(&shared).document().vmf(), &source);
        controller.window.invoke_apply();
        assert!(!controller.window.window().is_visible());
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &source);
        controller
            .initialize(&mut lock(&shared), id, "message")
            .unwrap();
        let replacement = lock(&shared)
            .begin_sound_choice(id, "message", false)
            .unwrap()["token"]
            .as_u64()
            .unwrap();
        controller.window.invoke_cancel();
        assert!(lock(&shared).sound_choice(replacement).is_ok());
        if std::env::var_os("HAMMERR_UI_SNAPSHOTS").is_some()
            && let Ok(game) = editor_core::startup::mount_game(None)
        {
            let mut session = Session::new(Ok(game));
            session.set_definitions(
                Actor::User,
                source_formats::fgd::Catalog::parse(
                    "@PointClass = ambient_generic [ message(sound) : \"Sound Name\" : \"\" ]",
                )
                .unwrap(),
            );
            let map_core::ObjectId::Entity(id) = session
                .create_entity(
                    Actor::User,
                    "ambient_generic",
                    Some(glam::DVec3::ZERO),
                    std::collections::BTreeMap::new(),
                )
                .unwrap()
            else {
                unreachable!()
            };
            let shared = Arc::new(Mutex::new(session));
            let controller = Controller::new(&shared).unwrap();
            controller
                .initialize(&mut lock(&shared), id, "message")
                .unwrap();
            controller.window.set_query("DoSpark".into());
            controller.window.invoke_search();
            controller.window.invoke_choose("DoSpark".into());
            controller.window.set_variant(1);
            controller.window.invoke_variant_changed();
            assert!(
                controller.window.get_can_apply(),
                "{}",
                controller.window.get_message()
            );
            controller.window.show().unwrap();
            crate::ui::tests::save_ui_snapshot(controller.window.window(), "sound-browser");
            controller.window.invoke_cancel();
        }
    }
}
