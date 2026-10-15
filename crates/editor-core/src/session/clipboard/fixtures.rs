use super::*;
use glam::DVec3;

#[derive(Default)]
pub struct Memory {
    pub text: String,
    pub fail: bool,
}

pub struct MemoryTransport(pub Arc<std::sync::Mutex<Memory>>);
impl Transport for MemoryTransport {
    fn read(&mut self) -> Result<String, String> {
        let state = self.0.lock().unwrap();
        if state.fail {
            Err("Native read failed".into())
        } else {
            Ok(state.text.clone())
        }
    }
    fn write(&mut self, text: &str) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        if state.fail {
            return Err("Native publication failed".into());
        }
        state.text = text.into();
        Ok(())
    }
}

pub fn fixture() -> (Session, ObjectId, Arc<std::sync::Mutex<Memory>>) {
    let mut session = Session::new(Err("No game needed".into()));
    let id = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    session.select(&[id], SelectMode::Replace).unwrap();
    let memory = Arc::new(std::sync::Mutex::new(Memory::default()));
    session.geometry.clipboard.transport = Box::new(MemoryTransport(memory.clone()));
    (session, id, memory)
}
