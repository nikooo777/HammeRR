use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UiRetired;

pub(super) struct DocumentWindow {
    app: Option<AppWindow>,
    ui: Option<SharedUi>,
    frame_timer: Option<slint::Timer>,
    contents: Option<Rc<instance_contents::Controller>>,
    reference: Option<Rc<instance_reference::Controller>>,
    extraction: Option<Rc<instance_extraction::Controller>>,
    retired: Option<Result<UiRetired, String>>,
}

pub(crate) fn hide_window(label: &str, window: &slint::Window, errors: &mut Vec<String>) {
    #[cfg(test)]
    if tests::fail_hide(label) {
        errors.push(format!("{label}: injected hide failure"));
        return;
    }
    if let Err(error) = window.hide() {
        errors.push(format!("{label}: {error}"));
    }
}

impl DocumentWindow {
    pub(super) fn new(
        app: AppWindow,
        ui: SharedUi,
        frame_timer: slint::Timer,
        contents: Option<Rc<instance_contents::Controller>>,
        reference: Option<Rc<instance_reference::Controller>>,
        extraction: Option<Rc<instance_extraction::Controller>>,
    ) -> Self {
        Self {
            app: Some(app),
            ui: Some(ui),
            frame_timer: Some(frame_timer),
            contents,
            reference,
            extraction,
            retired: None,
        }
    }

    pub(super) fn app(&self) -> &AppWindow {
        self.app.as_ref().expect("DocumentWindow is retired")
    }

    pub(super) fn finish(
        &mut self,
        primary: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match (primary, self.retire()) {
            (Ok(()), Ok(_)) => Ok(()),
            (Err(error), Ok(_)) => Err(error),
            (Ok(()), Err(error)) => Err(format!("UI retirement failed: {error}").into()),
            (Err(primary), Err(error)) => {
                Err(format!("{primary}; UI retirement failed: {error}").into())
            }
        }
    }

    pub(super) fn retire(&mut self) -> Result<UiRetired, String> {
        if let Some(result) = &self.retired {
            return result.clone();
        }
        if let Some(timer) = &self.frame_timer {
            timer.stop();
        }
        if let Some(controller) = &self.contents {
            controller.stop_timer();
        }
        if let Some(controller) = &self.reference {
            controller.stop_timer();
        }
        if let Some(controller) = &self.extraction {
            controller.stop_timer();
        }
        let mut errors = Vec::new();
        if let Some(ui) = &self.ui {
            match ui.try_borrow_mut() {
                Ok(mut ui) => {
                    ui.viewport.release_all();
                    ui.brush_drag = None;
                    ui.clip_drag = None;
                    ui.interaction = interaction::Interaction::default();
                    ui.object_cycle = object_picking::Gesture::default();
                    ui.pick_release = false;
                    ui.hide_children(&mut errors);
                }
                Err(error) => {
                    errors.push(format!("UI ownership is still borrowed: {error}"));
                    if let Ok(ui) = ui.try_borrow() {
                        ui.hide_children(&mut errors);
                    }
                }
            }
        }
        if let Some(controller) = &self.contents {
            controller.hide_window(&mut errors);
        }
        if let Some(controller) = &self.reference {
            controller.hide_window(&mut errors);
        }
        if let Some(controller) = &self.extraction {
            controller.hide_window(&mut errors);
        }
        if let Some(app) = &self.app {
            app.set_flying(false);
            app.set_frame(slint::Image::default());
        }
        self.frame_timer.take();
        self.contents.take();
        self.reference.take();
        self.extraction.take();
        self.ui.take();
        if let Some(app) = &self.app {
            hide_window("Editor", app.window(), &mut errors);
        }
        self.app.take();
        let result = if errors.is_empty() {
            Ok(UiRetired)
        } else {
            Err(errors.join("; "))
        };
        self.retired = Some(result.clone());
        result
    }
}

impl Drop for DocumentWindow {
    fn drop(&mut self) {
        if self.retired.is_none()
            && let Err(error) = self.retire()
        {
            eprintln!("UI retirement failed: {error}");
        }
    }
}

impl Ui {
    fn hide_children(&self, errors: &mut Vec<String>) {
        if let Some(pickers) = &self.property_pickers {
            pickers.hide_windows(errors);
        }
        if let Some(source) = &self.material_source {
            match source.try_borrow() {
                Ok(source) => {
                    if let Some(source) = source.as_ref() {
                        hide_window("Material Source", source.window.window(), errors);
                    }
                }
                Err(error) => errors.push(format!("Material Source is still borrowed: {error}")),
            }
        }
        macro_rules! hide_raw {
            ($($field:ident),+ $(,)?) => {
                $(if let Some(window) = &self.$field {
                    hide_window(stringify!($field), window.window(), errors);
                })+
            };
        }
        macro_rules! hide_controller {
            ($($field:ident),+ $(,)?) => {
                $(if let Some(controller) = &self.$field {
                    hide_window(stringify!($field), controller.window.window(), errors);
                })+
            };
        }
        hide_raw!(
            game_log_window,
            entity_window,
            connection_window,
            properties_window,
            report_window,
            statistics_window,
            pointfile_window,
            components_window,
            hollow_window,
            carve_window,
            merge_window,
            recovery_window,
            recent_maps_window,
            checks_window,
            context_window,
            primitive_window,
            rename_window,
            color_window,
            material_window,
            surface_window,
        );
        hide_controller!(
            polygon,
            linked_path,
            model,
            model_browser,
            sounds,
            particles,
            placement,
            entity_placement,
            repeat,
            clipboard,
            object_groups,
            layers,
            compile_region,
            fog,
            working_region,
            overlay,
            cubemap,
            atlas,
            lighting,
            terrain,
            terrain_triangles,
            material_replace,
            surface_group,
            volume,
            saved_views,
            history,
            measurements,
            grid_defaults,
            preview_defaults,
            navigation_defaults,
            shortcuts,
        );
        if let Some(compile) = &self.compile_status {
            compile.hide_windows(errors);
        }
        if let Some(prefabs) = &self.prefabs {
            prefabs.hide_windows(errors);
        }
    }
}

#[cfg(test)]
mod tests;
