use map_core::query::Ray;
use render::Camera;

use crate::{
    session::{Actor, Pointer, Session},
    viewport::CLICK_SLOP,
};

struct Press {
    pointer: Pointer,
    camera: Camera,
    generation: u64,
    revision: u64,
    scene: u64,
    view: u64,
    filters: u64,
    dragged: bool,
}

#[derive(Default)]
pub(super) struct Gesture {
    press: Option<Press>,
    awaiting_release: bool,
}

impl Gesture {
    pub fn begin(
        &mut self,
        session: &Session,
        camera: Camera,
        pointer: Pointer,
        control: bool,
        alt: bool,
    ) -> Result<bool, String> {
        *self = Self::default();
        if !alt
            || control
            || session.face_mode()
            || session.component_picking().is_some()
            || !session.component_selection().is_empty()
            || session.paint_enabled()
            || session.clip_tool()
            || session.spatial().draw_block
            || session.partition_preview_token().is_some()
        {
            return Ok(false);
        }
        self.awaiting_release = true;
        pointer.validate()?;
        if pointer.position.min_element() < 0.0
            || pointer.position.x >= f64::from(pointer.size.0)
            || pointer.position.y >= f64::from(pointer.size.1)
        {
            return Err("Click inside the viewport to cycle overlapping objects".into());
        }
        self.press = Some(Press {
            pointer,
            camera,
            generation: session.document_generation(),
            revision: session.document().revision(),
            scene: session.scene_serial(),
            view: session.view_serial(),
            filters: session.selection_filter_revision(),
            dragged: false,
        });
        Ok(true)
    }

    pub fn active(&self) -> bool {
        self.awaiting_release
    }

    pub fn moved(&mut self, pointer: Pointer) -> bool {
        if let Some(press) = &mut self.press {
            press.dragged |= pointer.validate().is_err()
                || pointer.size != press.pointer.size
                || pointer.position.distance(press.pointer.position) > f64::from(CLICK_SLOP);
        }
        self.active()
    }

    pub fn cancel(&mut self) {
        self.press = None;
    }

    pub fn release(
        &mut self,
        session: &mut Session,
        camera: Camera,
        pointer: Pointer,
    ) -> Result<Option<String>, String> {
        self.awaiting_release = false;
        let Some(press) = self.press.take() else {
            return Ok(None);
        };
        if press.dragged
            || pointer.validate().is_err()
            || pointer.position.min_element() < 0.0
            || pointer.position.x >= f64::from(pointer.size.0)
            || pointer.position.y >= f64::from(pointer.size.1)
            || pointer.size != press.pointer.size
            || pointer.position.distance(press.pointer.position) > f64::from(CLICK_SLOP)
            || camera != press.camera
            || session.camera() != press.camera
            || session.document_generation() != press.generation
            || session.document().revision() != press.revision
            || session.scene_serial() != press.scene
            || session.view_serial() != press.view
            || session.selection_filter_revision() != press.filters
        {
            return Ok(None);
        }
        let ndc = pointer.ndc();
        let (origin, direction) = camera.ray(
            ndc.x as f32,
            ndc.y as f32,
            pointer.size.0 as f32 / pointer.size.1 as f32,
        );
        let report = session.cycle_object_hit(
            Actor::User,
            &Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            },
        )?;
        let message = report["message"]
            .as_str()
            .ok_or("Missing object hit feedback")?;
        Ok(Some(message.to_owned()))
    }
}
