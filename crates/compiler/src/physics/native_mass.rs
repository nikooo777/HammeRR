use std::collections::BTreeMap;
use std::ffi::CString;

use super::*;
use crate::physics::{
    SurfaceData,
    surface_data::{self, Properties},
};

type Parse = unsafe extern "C" fn(Pointer, *const c_char, *const c_char) -> i32;
type Index = unsafe extern "C" fn(Pointer, *const c_char) -> i32;
type PhysicsProperties = unsafe extern "C" fn(Pointer, i32, *mut f32, *mut f32, *mut f32, *mut f32);

pub(super) struct MassApi {
    pointer: Pointer,
    count: Count,
    index: Index,
    properties: PhysicsProperties,
    expected: BTreeMap<String, Properties>,
}

impl MassApi {
    pub(super) fn load(api: &Api, files: &[SurfaceData]) -> Result<Self, String> {
        let expected = surface_data::resolve(files)?;
        let factory = unsafe { api._physics.get::<Factory>(b"CreateInterface\0") }
            .map_err(|error| error.to_string())?;
        let mut status = -1;
        let pointer = unsafe { factory(c"VPhysicsSurfaceProps001".as_ptr(), &mut status) };
        if pointer.is_null() || status != 0 {
            return Err("The installed VPhysicsSurfaceProps001 interface is unavailable".into());
        }
        let parse: Parse = unsafe { member(pointer, api.base, 2, 0x34940)? };
        let result = Self {
            pointer,
            count: unsafe { member(pointer, api.base, 3, 0x339b0)? },
            index: unsafe { member(pointer, api.base, 4, 0x33af0)? },
            properties: unsafe { member(pointer, api.base, 5, 0x34200)? },
            expected,
        };
        for file in files {
            let name = CString::new(file.name.as_str()).map_err(|error| error.to_string())?;
            let text = CString::new(file.text.as_str()).map_err(|error| error.to_string())?;
            let parsed = unsafe { parse(pointer, name.as_ptr(), text.as_ptr()) };
            let count = unsafe { (result.count)(pointer) };
            if parsed <= 0 || parsed != count || count as usize > surface_data::MAX_DEFINITIONS + 1
            {
                return Err(format!(
                    "Native physics rejected surface definitions {:?}",
                    file.name
                ));
            }
        }
        Ok(result)
    }

    pub(super) fn mass(&self, name: &str, volume: f32, area: f32) -> Result<f32, String> {
        let expected = self
            .expected
            .get(&name.to_ascii_lowercase())
            .ok_or_else(|| format!("Mass surface property {name:?} has no mounted definition"))?;
        expected.validate(name)?;
        let name_c = CString::new(name).map_err(|error| error.to_string())?;
        let index = unsafe { (self.index)(self.pointer, name_c.as_ptr()) };
        let count = unsafe { (self.count)(self.pointer) };
        if index < 0 || index >= count || index > super::super::MAX_SURFACE_PROPERTIES as i32 {
            return Err(format!(
                "Mass surface property {name:?} has no supported native surface index"
            ));
        }
        let mut density = f32::NAN;
        let mut thickness = f32::NAN;
        unsafe {
            (self.properties)(
                self.pointer,
                index,
                &mut density,
                &mut thickness,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
        if density != expected.density || thickness != expected.thickness {
            return Err(format!(
                "Native surface property {name:?} density/thickness disagrees with its mounted inheritance"
            ));
        }
        Properties { density, thickness }.mass(volume, area)
    }
}
