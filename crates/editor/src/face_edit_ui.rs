use crate::session::{Actor, Session};
use map_core::surface::FaceId;

#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) faces: Vec<FaceId>,
    generation: u64,
    revision: u64,
}

impl Context {
    pub(crate) fn capture(session: &Session, faces: Vec<FaceId>) -> Self {
        Self {
            faces,
            generation: session.document_generation(),
            revision: session.document().revision(),
        }
    }
    pub(crate) fn check(&self, session: &Session, message: &str) -> Result<(), String> {
        if self.generation != session.document_generation()
            || self.revision != session.document().revision()
            || self.faces != session.selected_faces()
        {
            return Err(message.into());
        }
        Ok(())
    }
}

pub(crate) fn foreign_preview(session: &Session, token: Option<u64>) -> bool {
    session.face_preview_token().is_some() && session.face_preview_token() != token
}

pub(crate) fn cancel_preview(session: &mut Session, token: &mut Option<u64>) {
    if token.is_some() && session.face_preview_token() == *token {
        session.cancel_face_preview();
    }
    *token = None;
}

pub(crate) fn commit_preview(
    session: &mut Session,
    token: Option<u64>,
    preview: bool,
    message: &str,
) -> Result<Option<usize>, String> {
    if !preview && foreign_preview(session, token) {
        return Err(message.into());
    }
    if !preview && token.is_some() {
        session.check_face_preview_token(token)?;
        return session.commit_face_preview(Actor::User).map(Some);
    }
    Ok(None)
}
