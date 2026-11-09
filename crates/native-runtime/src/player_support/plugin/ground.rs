use std::io;

use crate::util::invalid;
use crate::{PlayerSupportEntity, PlayerSupportGroundState};

use super::super::compiled_supports;
use super::{State, ordered};

pub(super) struct GroundBinding {
    pub handle: u32,
    pub entity: Option<PlayerSupportEntity>,
    pub state: Option<PlayerSupportGroundState>,
}

pub(super) struct RetainedGround {
    pub edict: u64,
    entity: PlayerSupportEntity,
    hammer_id: i32,
    model_name: String,
    bsp_model_index: u32,
    model_index: u16,
}

impl GroundBinding {
    pub fn resolution(&self) -> &'static str {
        match &self.state {
            Some(PlayerSupportGroundState::World { .. }) => "serial_resolved_world",
            Some(PlayerSupportGroundState::FuncMovelinear { .. }) => {
                "serial_resolved_compiled_func_movelinear"
            }
            None if self.handle == 0 => "null_handle",
            None => "invalid_handle_resolves_null",
        }
    }

    fn same_bits(&self, other: &Self) -> bool {
        self.handle == other.handle
            && self.entity == other.entity
            && match (&self.state, &other.state) {
                (None, None) => true,
                (Some(first), Some(second)) => first.same_bits(second),
                _ => false,
            }
    }
}

impl State {
    fn support_u16(&self, address: u64) -> io::Result<u16> {
        let bytes = self.memory.bytes(address, 2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(super) fn ground_binding(
        &mut self,
        player: &PlayerSupportEntity,
    ) -> io::Result<GroundBinding> {
        let handle = self.memory.u32(player.address + 0x330)?;
        let entity = self.resolve_handle(handle)?;
        let Some(ground) = &entity else {
            return Ok(GroundBinding {
                handle,
                entity,
                state: None,
            });
        };
        let (edict, object) = self.edict(ground.index as i32)?;
        if edict == 0 || object != ground.address {
            return Err(invalid("Observer ground entity/edict roundtrip differs"));
        }
        let state = if ground.index == 0 {
            let world = self
                .world
                .as_ref()
                .ok_or_else(|| invalid("Observer world identity is missing"))?;
            self.object(ground.address, self.server + 0xc10d98)?;
            if ground != world || edict != self.world_edict {
                return Err(invalid("Observer ground world identity differs"));
            }
            PlayerSupportGroundState::World { edict }
        } else {
            self.object(ground.address, self.server + 0xbacdd0).map_err(|error| invalid(format!("Observer ground class/vtable is outside func_movelinear: index={}, classname={:?}, address={:#x}: {error}", ground.index, ground.classname, ground.address)))?;
            let hammer_id = self.memory.i32(ground.address + 0xc0)?;
            let model_name = self
                .memory
                .string(self.memory.u64(ground.address + 0x338)?, 32)?;
            let declaration = compiled_supports::matching(
                &self.request.compiled_supports,
                &ground.classname,
                hammer_id,
                &model_name,
            ).map_err(|error| invalid(format!("Observer ground declaration mismatch: index={}, hammer_id={hammer_id}, model_name={model_name:?}: {error}", ground.index)))?;
            let bsp_model_index = declaration.bsp_model_index;
            let move_parent_handle = self.memory.u32(ground.address + 0x258)?;
            if self.resolve_handle(move_parent_handle)?.is_some() {
                return Err(invalid("Observer moving ground has a native move parent"));
            }
            let eflags = self.memory.u32(ground.address + 0x210)?;
            if eflags & 0x1800 != 0 {
                return Err(invalid(format!(
                    "Observer moving ground has dirty absolute pose/velocity caches: index={}, eflags={eflags:#x}",
                    ground.index
                )));
            }
            let model_index = self.support_u16(ground.address + 0xca)?;
            let collision_mins = self.memory.vector(ground.address + 0x290)?;
            let collision_maxs = self.memory.vector(ground.address + 0x29c)?;
            ordered(collision_mins, collision_maxs)?;
            let state = PlayerSupportGroundState::FuncMovelinear {
                edict,
                hammer_id,
                model_name,
                bsp_model_index,
                model_index,
                move_parent_handle,
                move_type: self.memory.u8(ground.address + 0x256)?,
                solid_type: self.memory.u8(ground.address + 0x2b1)?,
                solid_flags: self.support_u16(ground.address + 0x2ac)?,
                origin: self.memory.vector(ground.address + 0x3b4)?,
                local_origin: self.memory.vector(ground.address + 0x45c)?,
                angles: self.memory.vector(ground.address + 0x450)?,
                local_angles: self.memory.vector(ground.address + 0x468)?,
                velocity: self.memory.vector(ground.address + 0x3c0)?,
                absolute_velocity: self.memory.vector(ground.address + 0x34c)?,
                base_velocity: self.memory.vector(ground.address + 0x340)?,
                collision_mins,
                collision_maxs,
                eflags,
            };
            if self.memory.u32(ground.address + 0x210)? != eflags {
                return Err(invalid(
                    "Observer moving-ground cache state changed during its copy",
                ));
            }
            self.retain_ground(ground, &state)?;
            state
        };
        Ok(GroundBinding {
            handle,
            entity,
            state: Some(state),
        })
    }

    fn retain_ground(
        &mut self,
        entity: &PlayerSupportEntity,
        state: &PlayerSupportGroundState,
    ) -> io::Result<()> {
        let PlayerSupportGroundState::FuncMovelinear {
            edict,
            hammer_id,
            model_name,
            bsp_model_index,
            model_index,
            ..
        } = state
        else {
            return Err(invalid(
                "Observer retained moving-ground state has another kind",
            ));
        };
        if let Some(retained) = self.retained_supports.get(&entity.index) {
            if retained.entity != *entity
                || retained.edict != *edict
                || retained.hammer_id != *hammer_id
                || retained.model_name != *model_name
                || retained.bsp_model_index != *bsp_model_index
                || retained.model_index != *model_index
            {
                return Err(invalid(
                    "Observer moving-ground identity or compiled association changed",
                ));
            }
            return Ok(());
        }
        if self.retained_supports.len() >= self.request.profile.settings.max_samples
            || self.retained_supports.values().any(|retained| {
                retained.edict == *edict
                    || retained.entity.address == entity.address
                    || retained.hammer_id == *hammer_id
            })
        {
            return Err(invalid(
                "Observer moving-ground native identity/declaration was reused or its sample bound was exceeded",
            ));
        }
        self.retained_supports.insert(
            entity.index,
            RetainedGround {
                edict: *edict,
                entity: entity.clone(),
                hammer_id: *hammer_id,
                model_name: model_name.clone(),
                bsp_model_index: *bsp_model_index,
                model_index: *model_index,
            },
        );
        Ok(())
    }

    pub(super) fn check_ground_binding(
        &mut self,
        player: &PlayerSupportEntity,
        retained: &GroundBinding,
    ) -> io::Result<()> {
        let current = self.ground_binding(player)?;
        if !retained.same_bits(&current) {
            return Err(invalid(
                "Observer ground handle, owner or actual support state changed around the native query/row commit",
            ));
        }
        Ok(())
    }
}
