use std::ffi::{CStr, c_char, c_void};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::Serialize;
use serde_json::{Value, json};

use super::memory::Memory;
use super::{MAX_OUTPUT, OUTPUT, PROFILE, PluginRequest, Publication, SERVER_SHA, VPHYSICS_SHA};
use crate::player_support_types::{PLAYER_SUPPORT_COLLISION_GROUP, PLAYER_SUPPORT_MOVEMENT_MASK};
use crate::terrain_snapshot::EngineImage;
use crate::util::{self, invalid};
use crate::{
    PlayerSupportEntity, PlayerSupportInput, PlayerSupportProbe, PlayerSupportProbePlane,
    PlayerSupportProbeStatus, PlayerSupportSample, PlayerSupportSurface,
};

type Factory = unsafe extern "C" fn(*const c_char, *mut i32) -> *mut c_void;
type Object = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeVector {
    x: f32,
    y: f32,
    z: f32,
}

impl NativeVector {
    fn array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
}

impl From<[f32; 3]> for NativeVector {
    fn from([x, y, z]: [f32; 3]) -> Self {
        Self { x, y, z }
    }
}

#[repr(C, align(8))]
struct NativeTrace([u8; 104]);

impl NativeTrace {
    fn u16(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.0[offset], self.0[offset + 1]])
    }
    fn u32(&self, offset: usize) -> u32 {
        u32::from_le_bytes(std::array::from_fn(|axis| self.0[offset + axis]))
    }
    fn u64(&self, offset: usize) -> u64 {
        u64::from_le_bytes(std::array::from_fn(|axis| self.0[offset + axis]))
    }
    fn f32(&self, offset: usize) -> f32 {
        f32::from_bits(self.u32(offset))
    }
    fn vector(&self, offset: usize) -> [f32; 3] {
        std::array::from_fn(|axis| self.f32(offset + axis * 4))
    }
}

#[repr(C)]
struct Plugin {
    vtable: &'static VTable,
}

#[repr(C)]
struct VTable {
    load: unsafe extern "C" fn(*mut Plugin, Factory, Factory) -> bool,
    unload: unsafe extern "C" fn(*mut Plugin),
    pause: unsafe extern "C" fn(*mut Plugin),
    unpause: unsafe extern "C" fn(*mut Plugin),
    description: unsafe extern "C" fn(*mut Plugin) -> *const c_char,
    level_init: unsafe extern "C" fn(*mut Plugin, *const c_char),
    server_activate: unsafe extern "C" fn(*mut Plugin, Object, i32, i32),
    game_frame: unsafe extern "C" fn(*mut Plugin, bool),
    level_shutdown: unsafe extern "C" fn(*mut Plugin),
    client_active: unsafe extern "C" fn(*mut Plugin, Object),
    client_disconnect: unsafe extern "C" fn(*mut Plugin, Object),
    client_put: unsafe extern "C" fn(*mut Plugin, Object, *const c_char),
    command_client: unsafe extern "C" fn(*mut Plugin, i32),
    client_settings: unsafe extern "C" fn(*mut Plugin, Object),
    client_connect: unsafe extern "C" fn(
        *mut Plugin,
        *mut bool,
        Object,
        *const c_char,
        *const c_char,
        *mut c_char,
        i32,
    ) -> i32,
    client_command: unsafe extern "C" fn(*mut Plugin, Object, Object) -> i32,
    network_validated: unsafe extern "C" fn(*mut Plugin, *const c_char, *const c_char) -> i32,
    cvar_finished:
        unsafe extern "C" fn(*mut Plugin, i32, Object, i32, *const c_char, *const c_char),
    edict_allocated: unsafe extern "C" fn(*mut Plugin, Object),
    edict_freed: unsafe extern "C" fn(*mut Plugin, Object),
}

static VTABLE: VTable = VTable {
    load,
    unload,
    pause,
    unpause,
    description,
    level_init,
    server_activate,
    game_frame,
    level_shutdown,
    client_active,
    client_disconnect,
    client_put,
    command_client,
    client_settings,
    client_connect,
    client_command,
    network_validated,
    cvar_finished,
    edict_allocated,
    edict_freed,
};
static PLUGIN: Plugin = Plugin { vtable: &VTABLE };

#[repr(C)]
struct Listener {
    vtable: &'static ListenerVTable,
}
#[repr(C)]
struct ListenerVTable {
    destructor: unsafe extern "C" fn(*mut Listener),
    deleting_destructor: unsafe extern "C" fn(*mut Listener),
    event: unsafe extern "C" fn(*mut Listener, Object),
}
static LISTENER_VTABLE: ListenerVTable = ListenerVTable {
    destructor: listener_destructor,
    deleting_destructor: listener_destructor,
    event,
};
static LISTENER: Listener = Listener {
    vtable: &LISTENER_VTABLE,
};
static STATE: Mutex<Option<State>> = Mutex::new(None);
static CALLBACK_THREAD: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicBool = AtomicBool::new(false);
static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

fn mark_failed(reason: &str) {
    let first = !FAILED.swap(true, Ordering::AcqRel);
    eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {reason}");
    if first
        && let Some(directory) = DIRECTORY.get()
        && let Err(error) = util::atomic_write(
            &directory.join("player-support-fatal.txt"),
            reason.as_bytes(),
        )
    {
        eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR fatal receipt: {error}");
    }
}

struct Previous {
    callback: u64,
    tick: i32,
    time: f32,
    generation: u64,
}

#[derive(Clone, Copy)]
struct SetupCall {
    function: usize,
    engine: u64,
    edict: u64,
    kind: SetupKind,
    generation: u64,
    player_handle: u32,
    world_handle: u32,
    client_object: u64,
    user_id: i32,
}

#[derive(Clone, Copy)]
enum SetupKind {
    Team(i32),
    Input(PlayerSupportInput),
}

#[derive(Clone, Copy, Debug, Serialize)]
struct ClientRow {
    row: i32,
    slot: i32,
    object: u64,
    interface: u64,
    state: i32,
    connected: bool,
    spawned: bool,
    active: bool,
    fake: bool,
    hltv: bool,
    replay: bool,
    user_id: i32,
    native_edict: u64,
    edict: u64,
    entity: u64,
}

struct SoleClient {
    native: ClientRow,
    player: PlayerSupportEntity,
    catalog: Value,
}

struct State {
    directory: PathBuf,
    request: PluginRequest,
    memory: Memory,
    engine: u64,
    server: u64,
    vphysics: u64,
    engine_interface: u64,
    entity_interface: u64,
    event_manager: u64,
    physics_interface: u64,
    surface_props: u64,
    movement: u64,
    trace_interface: u64,
    _physics_library: libloading::Library,
    modules: Value,
    callback: u64,
    thread: u64,
    activated: bool,
    map_initialized: bool,
    setup_retired: bool,
    setup_attempted: bool,
    input_attempted: bool,
    input_retired: bool,
    input_receipt: Value,
    input_acknowledgement: Value,
    pending_client: Option<u64>,
    setup_receipt: Value,
    native_client: Option<ClientRow>,
    client_catalog: Value,
    generation: u64,
    sampled_generation: Option<u64>,
    previous: Option<Previous>,
    player: Option<PlayerSupportEntity>,
    world: Option<PlayerSupportEntity>,
    player_edict: u64,
    world_edict: u64,
    first_time: Option<f32>,
    last_time: Option<f32>,
    last_tickbase: Option<i32>,
    last_tickbase_advance: Option<f32>,
    samples: Vec<PlayerSupportSample>,
    closed: bool,
    listeners_registered: bool,
}

fn lock_state() -> Option<MutexGuard<'static, Option<State>>> {
    let thread = unsafe { libc::syscall(libc::SYS_gettid) } as u64;
    match CALLBACK_THREAD.compare_exchange(0, thread, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {}
        Err(retained) if retained == thread => {}
        Err(_) => {
            mark_failed("Observer callback thread changed");
            return None;
        }
    }
    match STATE.try_lock() {
        Ok(state) => Some(state),
        Err(error) => {
            mark_failed(&format!("Observer callback lock/reentrancy: {error}"));
            None
        }
    }
}

fn listener_pointer() -> Object {
    (&raw const LISTENER).cast_mut().cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn CreateInterface(name: *const c_char, status: *mut i32) -> Object {
    if !name.is_null() && unsafe { CStr::from_ptr(name) }.to_bytes() == b"ISERVERPLUGINCALLBACKS003"
    {
        if !status.is_null() {
            unsafe {
                *status = 0;
            }
        }
        (&raw const PLUGIN).cast_mut().cast()
    } else {
        if !status.is_null() {
            unsafe {
                *status = 1;
            }
        }
        std::ptr::null_mut()
    }
}

fn factory_object(factory: Factory, name: &CStr) -> io::Result<u64> {
    let mut status = -1;
    let object = unsafe { factory(name.as_ptr(), &mut status) } as u64;
    if status != 0 || object == 0 {
        return Err(invalid(format!(
            "Observer interface unavailable: {}",
            name.to_string_lossy()
        )));
    }
    Ok(object)
}

impl State {
    fn new(directory: PathBuf, engine_factory: Factory, game_factory: Factory) -> io::Result<Self> {
        let request: PluginRequest = util::json_read(
            &directory.join("player-support-observer-request.json"),
            64 * 1024,
        )?;
        request.profile.validate()?;
        let mut biases = Vec::new();
        let mut modules = Vec::new();
        for (path, sha) in [
            (request.root.join("bin/linux64/engine.so"), util::ENGINE_SHA),
            (
                request.root.join("cstrike/bin/linux64/server.so"),
                SERVER_SHA,
            ),
            (request.root.join("bin/linux64/vphysics.so"), VPHYSICS_SHA),
        ] {
            let image = EngineImage::open_at(&path, sha)?;
            let (bias, receipt) = image.verify_local()?;
            biases.push(bias);
            modules.push(receipt);
        }
        let observer_image =
            EngineImage::open_at(&request.observer_path, &request.observer_sha256)?;
        let (_, observer_receipt) = observer_image.verify_local()?;
        modules.push(observer_receipt);
        let memory = Memory::open()?;
        let engine_interface = factory_object(engine_factory, c"VEngineServer023")?;
        let entity_interface = factory_object(game_factory, c"ServerGameEnts001")?;
        let event_manager = factory_object(engine_factory, c"GAMEEVENTSMANAGER002")?;
        let movement = factory_object(game_factory, c"GameMovement001")?;
        let trace_interface = factory_object(engine_factory, c"EngineTraceServer003")?;
        let physics_library: libloading::Library = unsafe {
            libloading::os::unix::Library::open(
                Some(request.root.join("bin/linux64/vphysics.so")),
                libc::RTLD_NOW | libc::RTLD_NOLOAD,
            )
        }
        .map_err(io::Error::other)?
        .into();
        let physics_factory = unsafe { physics_library.get::<Factory>(b"CreateInterface\0") }
            .map_err(io::Error::other)?;
        let physics_interface = factory_object(*physics_factory, c"VPhysics031")?;
        let surface_props = factory_object(*physics_factory, c"VPhysicsSurfaceProps001")?;
        let sample_capacity = request.profile.settings.max_samples;
        let input_retired = request.profile.settings.input == PlayerSupportInput::Idle;
        let value = Self {
            directory,
            request,
            memory,
            engine: biases[0],
            server: biases[1],
            vphysics: biases[2],
            engine_interface,
            entity_interface,
            event_manager,
            physics_interface,
            surface_props,
            movement,
            trace_interface,
            _physics_library: physics_library,
            modules: json!(modules),
            callback: 0,
            thread: 0,
            activated: false,
            map_initialized: false,
            setup_retired: false,
            setup_attempted: false,
            input_attempted: false,
            input_retired,
            input_receipt: Value::Null,
            input_acknowledgement: Value::Null,
            pending_client: None,
            setup_receipt: Value::Null,
            native_client: None,
            client_catalog: Value::Null,
            generation: 0,
            sampled_generation: None,
            previous: None,
            player: None,
            world: None,
            player_edict: 0,
            world_edict: 0,
            first_time: None,
            last_time: None,
            last_tickbase: None,
            last_tickbase_advance: None,
            samples: Vec::with_capacity(sample_capacity),
            closed: false,
            listeners_registered: false,
        };
        value.object(value.engine_interface, value.engine + 0x8e1770)?;
        if value.engine_interface != value.engine + 0xa012b0
            || value.entity_interface != value.server + 0xc92e90
            || value.event_manager != value.engine + 0xc563a0
            || value.movement != value.server + 0xdcfe40
            || value.trace_interface != value.engine + 0xc54de0
        {
            return Err(invalid("Observer public interface singleton differs"));
        }
        value.object(value.entity_interface, value.server + 0xb54ef8)?;
        value.object(value.event_manager, value.engine + 0x8e2608)?;
        value.object(value.physics_interface, value.vphysics + 0x193c98)?;
        value.object(value.movement, value.server + 0xc23ec8)?;
        value.object(value.trace_interface, value.engine + 0x8e23a0)?;
        Ok(value)
    }

    fn note_client_active(&mut self, edict: u64) -> io::Result<()> {
        if self.closed
            || !self.map_initialized
            || !self.activated
            || self.pending_client.is_some()
            || self.setup_attempted
            || edict == 0
        {
            return Err(invalid(
                "Observer ClientActive was outside its one private activated startup",
            ));
        }
        self.pending_client = Some(edict);
        self.previous = None;
        Ok(())
    }

    fn prepare_gameframe_setup(&mut self) -> io::Result<Option<SetupCall>> {
        if !self.closed && self.input_attempted && !self.input_retired {
            return Err(invalid(
                "Observer GameFrame reentered while controlled input was in flight",
            ));
        }
        if self.closed || self.setup_retired {
            return Ok(None);
        }
        if self.setup_attempted {
            return Err(invalid(
                "Observer GameFrame reentered while client setup was in flight",
            ));
        }
        self.pending_client
            .map(|edict| self.prepare_client_setup(edict))
            .transpose()
    }

    fn prepare_client_setup(&mut self, edict: u64) -> io::Result<SetupCall> {
        if self.closed
            || !self.map_initialized
            || !self.activated
            || self.setup_attempted
            || self.first_time.is_some()
            || FAILED.load(Ordering::Acquire)
        {
            return Err(invalid(
                "Observer ClientActive setup was outside its one private activated startup",
            ));
        }
        self.memory.refresh()?;
        self.plugins_only()?;
        let sole = self.sole_active_client(edict)?;
        let player = sole.player;
        self.native_client = Some(sole.native);
        self.client_catalog = sole.catalog;
        let (world_edict, world_object) = self.edict(0)?;
        self.object(world_object, self.server + 0xc10d98)?;
        let world = self.entity(world_object, 0)?;
        if world.classname != "worldspawn" {
            return Err(invalid("Observer ClientActive world identity differs"));
        }
        let function = self.function(self.engine_interface, 0x130, self.engine + 0x49cad0)?;
        let call = SetupCall {
            function,
            engine: self.engine_interface,
            edict,
            kind: SetupKind::Team(i32::from(self.request.profile.settings.team)),
            generation: self.generation,
            player_handle: player.reference_handle,
            world_handle: world.reference_handle,
            client_object: sole.native.object,
            user_id: sole.native.user_id,
        };
        self.player = Some(player);
        self.player_edict = edict;
        self.world = Some(world);
        self.world_edict = world_edict;
        self.previous = None;
        self.setup_attempted = true;
        Ok(call)
    }

    fn finish_client_setup(&mut self, call: SetupCall) -> io::Result<()> {
        let eligible = match call.kind {
            SetupKind::Team(_) => {
                self.setup_attempted
                    && !self.setup_retired
                    && self.pending_client == Some(call.edict)
            }
            SetupKind::Input(_) => {
                self.setup_retired
                    && self.input_attempted
                    && !self.input_retired
                    && self.pending_client.is_none()
            }
        };
        if !eligible
            || self.closed
            || self.generation != call.generation
            || self.first_time.is_some()
            || FAILED.load(Ordering::Acquire)
        {
            return Err(invalid(
                "Observer ClientActive setup lifetime changed during queueing",
            ));
        }
        self.memory.refresh()?;
        self.plugins_only()?;
        let sole = self.sole_active_client(call.edict)?;
        let current_player = sole.player;
        let edict = sole.native.edict;
        let object = current_player.address;
        if sole.native.object != call.client_object || sole.native.user_id != call.user_id {
            return Err(invalid(format!(
                "Observer setup client changed: row={}, slot={}, state={}, client={:#x}, expected_client={:#x}, user_id={}, expected_user_id={}, edict={:#x}, pending={:#x}, entity={:#x}",
                sole.native.row,
                sole.native.slot,
                sole.native.state,
                sole.native.object,
                call.client_object,
                sole.native.user_id,
                call.user_id,
                edict,
                call.edict,
                object
            )));
        }
        self.client_catalog = sole.catalog;
        let (world_edict, world_object) = self.edict(0)?;
        let current_world = self.entity(world_object, 0)?;
        if edict != call.edict
            || edict != self.player_edict
            || self
                .player
                .as_ref()
                .is_none_or(|player| object != player.address)
            || current_player.reference_handle != call.player_handle
            || world_edict != self.world_edict
            || current_world.reference_handle != call.world_handle
            || self
                .world
                .as_ref()
                .is_none_or(|world| world.address != world_object)
        {
            return Err(invalid(
                "Observer ClientActive player/world binding changed during queueing",
            ));
        }
        let commands = match call.kind {
            SetupKind::Team(team) => format!("jointeam {team}; joinclass 1"),
            SetupKind::Input(input) => input_commands(input).to_string_lossy().into_owned(),
        };
        let receipt = json!({"callback":"GameFrame","method":"VEngineServer023::ClientCommand","slot":38,"slot_byte_offset":0x130,"function":call.function,"edict":call.edict,"player":current_player,"world":current_world,"commands":commands,"native_client":sole.native,"queue_count":1,"callback_thread":CALLBACK_THREAD.load(Ordering::Acquire),"generation_at_queue":call.generation,"queued_before_measurement":true,"returned":true,"return_is_input_acknowledgement":false});
        match call.kind {
            SetupKind::Team(_) => {
                self.setup_receipt = receipt;
                self.setup_retired = true;
                self.pending_client = None;
            }
            SetupKind::Input(_) => {
                self.input_receipt = receipt;
                self.input_retired = true;
            }
        }
        self.previous = None;
        Ok(())
    }

    fn prepare_input_setup(&mut self, player: &PlayerSupportEntity) -> io::Result<SetupCall> {
        if !self.setup_retired
            || self.input_retired
            || self.input_attempted
            || self.first_time.is_some()
            || self.closed
            || FAILED.load(Ordering::Acquire)
        {
            return Err(invalid(
                "Observer input queue was outside its one completed startup state",
            ));
        }
        let world = self
            .world
            .as_ref()
            .ok_or_else(|| invalid("Observer input setup has no world identity"))?;
        let native_client = self
            .native_client
            .ok_or_else(|| invalid("Observer input setup has no native client"))?;
        let call = SetupCall {
            function: self.function(self.engine_interface, 0x130, self.engine + 0x49cad0)?,
            engine: self.engine_interface,
            edict: self.player_edict,
            kind: SetupKind::Input(self.request.profile.settings.input),
            generation: self.generation,
            player_handle: player.reference_handle,
            world_handle: world.reference_handle,
            client_object: native_client.object,
            user_id: native_client.user_id,
        };
        self.input_attempted = true;
        self.previous = None;
        Ok(call)
    }

    fn refuse(&mut self, error: io::Error) {
        mark_failed(&error.to_string());
        if let Err(publication) = self.publish(Some(error.to_string())) {
            mark_failed(&format!("{error}; publication: {publication}"));
        }
    }

    fn client_i32(&self, interface: u64, slot: u64, target: u64) -> io::Result<i32> {
        let getter: unsafe extern "C" fn(Object) -> i32 =
            unsafe { std::mem::transmute(self.function(interface, slot, self.engine + target)?) };
        Ok(unsafe { getter(interface as Object) })
    }

    fn client_bool(&self, interface: u64, slot: u64, target: u64) -> io::Result<bool> {
        let getter: unsafe extern "C" fn(Object) -> bool =
            unsafe { std::mem::transmute(self.function(interface, slot, self.engine + target)?) };
        Ok(unsafe { getter(interface as Object) })
    }

    fn sole_active_client(&self, expected_edict: u64) -> io::Result<SoleClient> {
        let server = self.engine + 0xbdb900;
        let globals = self.memory.u64(self.server + 0xd53080)?;
        let maximum = self.memory.i32(globals + 0x14)?;
        let count = self.memory.i32(server + 0x190)?;
        let array = self.memory.u64(server + 0x180)?;
        if !(1..=64).contains(&maximum) || !(1..=maximum).contains(&count) || expected_edict == 0 {
            return Err(invalid(format!(
                "Observer native client catalog bounds differ: count={count}, maximum={maximum}, array={array:#x}, expected_edict={expected_edict:#x}"
            )));
        }
        self.memory
            .range(array, count as usize * size_of::<u64>())?;
        let mut catalog = Vec::with_capacity(count as usize);
        let mut present: Option<ClientRow> = None;
        for row in 0..count {
            let object = self.memory.u64(array + row as u64 * 8)?;
            self.object(object,self.engine+0x9555b0).map_err(|error|invalid(format!("Observer native client row={row}, client={object:#x}, expected_edict={expected_edict:#x}: {error}")))?;
            let interface = object + 8;
            self.object(interface, self.engine + 0x955818)?;
            let slot = self.client_i32(interface, 0x78, 0x3a6950)?;
            let state = self.memory.i32(object + 0xe8)?;
            let connected = self.client_bool(interface, 0x100, 0x3a69f0)?;
            let spawned = self.client_bool(interface, 0x108, 0x3a6a10)?;
            let active = self.client_bool(interface, 0x110, 0x3a6a30)?;
            let fake = self.client_bool(interface, 0x118, 0x3a6a50)?;
            let hltv = self.client_bool(interface, 0x120, 0x3a6a70)?;
            let replay = self.client_bool(interface, 0x128, 0x3a6a90)?;
            let user_id = self.client_i32(interface, 0x80, 0x3a6970)?;
            let native_edict = self.memory.u64(object + 0x27450)?;
            let (edict, entity) = self.edict(row + 1)?;
            let record = ClientRow {
                row,
                slot,
                object,
                interface,
                state,
                connected,
                spawned,
                active,
                fake,
                hltv,
                replay,
                user_id,
                native_edict,
                edict,
                entity,
            };
            let context = || {
                format!(
                    "row={row}, slot={slot}, state={state}, client={object:#x}, native_edict={native_edict:#x}, edict={edict:#x}, entity={entity:#x}, expected_edict={expected_edict:#x}, prior_match={:?}, user_id={user_id}, connected={connected}, spawned={spawned}, active={active}, fake={fake}, hltv={hltv}, replay={replay}",
                    present.as_ref().map(|client| client.row)
                )
            };
            if slot != row
                || connected != (state > 1)
                || spawned != (state > 2)
                || active != (state == 6)
            {
                return Err(invalid(format!(
                    "Observer native client slot/predicate disagrees: {}",
                    context()
                )));
            }
            if state == 0 {
                if connected
                    || spawned
                    || active
                    || fake
                    || hltv
                    || replay
                    || user_id != 0
                    || native_edict != 0
                    || entity != 0
                {
                    return Err(invalid(format!(
                        "Observer state0 client violates completed idle/Clear state: {}",
                        context()
                    )));
                }
                catalog.push(record);
                continue;
            }
            if !(2..=6).contains(&state) {
                return Err(invalid(format!(
                    "Observer native client has an unknown/connecting transition state: {}",
                    context()
                )));
            }
            if present.is_some() {
                return Err(invalid(format!(
                    "Observer found a second connected native client: {}",
                    context()
                )));
            }
            present = Some(record);
            catalog.push(record);
        }
        let native = present.ok_or_else(||invalid(format!("Observer native client catalog has no present target: expected_edict={expected_edict:#x}, count={count}, catalog={catalog:?}")))?;
        let context = || {
            format!(
                "row={}, slot={}, state={}, client={:#x}, native_edict={:#x}, edict={:#x}, entity={:#x}, expected_edict={expected_edict:#x}, prior_match=true, user_id={}, fake={}, hltv={}, replay={}",
                native.row,
                native.slot,
                native.state,
                native.object,
                native.native_edict,
                native.edict,
                native.entity,
                native.user_id,
                native.fake,
                native.hltv,
                native.replay
            )
        };
        if !native.connected
            || !native.spawned
            || !native.active
            || native.state != 6
            || native.fake
            || native.hltv
            || native.replay
            || native.user_id <= 0
            || native.native_edict != native.edict
            || native.edict != expected_edict
            || native.entity == 0
        {
            return Err(invalid(format!(
                "Observer sole native client is not the bound active ordinary target: {}",
                context()
            )));
        }
        if self.native_client.as_ref().is_some_and(|previous| {
            previous.object != native.object
                || previous.slot != native.slot
                || previous.user_id != native.user_id
                || previous.edict != native.edict
        }) {
            return Err(invalid(format!(
                "Observer retained native client identity changed: {}",
                context()
            )));
        }
        let user_id: unsafe extern "C" fn(Object, Object) -> i32 = unsafe {
            std::mem::transmute(self.function(
                self.engine_interface,
                0x78,
                self.engine + 0x49b050,
            )?)
        };
        if unsafe { user_id(self.engine_interface as Object, native.edict as Object) }
            != native.user_id
        {
            return Err(invalid(format!(
                "Observer public player user ID differs from native client: {}",
                context()
            )));
        }
        self.object(native.entity, self.server + 0xc25ca8)?;
        let player = self.entity(native.entity, (native.slot + 1) as u32)?;
        if player.classname != "player" {
            return Err(invalid(format!(
                "Observer native client entity classname is {:?}: {}",
                player.classname,
                context()
            )));
        }
        Ok(SoleClient {
            native,
            player,
            catalog: json!({"server":server,"array":array,"count":count,"maximum":maximum,"rows":catalog}),
        })
    }

    fn object(&self, object: u64, vtable: u64) -> io::Result<()> {
        if self.memory.u64(object)? != vtable {
            return Err(invalid("Observer native object vtable differs"));
        }
        Ok(())
    }
    fn function(&self, object: u64, slot: u64, expected: u64) -> io::Result<usize> {
        self.memory
            .function(self.memory.u64(self.memory.u64(object)? + slot)?, expected)
    }
    fn register(&mut self) -> io::Result<()> {
        let add: unsafe extern "C" fn(Object, Object, *const c_char, bool) -> bool = unsafe {
            std::mem::transmute(self.function(self.event_manager, 0x20, self.engine + 0x513b60)?)
        };
        let find: unsafe extern "C" fn(Object, Object, *const c_char) -> bool = unsafe {
            std::mem::transmute(self.function(self.event_manager, 0x28, self.engine + 0x5118a0)?)
        };
        self.listeners_registered = true;
        for event in [c"player_spawn", c"player_death", c"player_team"] {
            if !unsafe {
                add(
                    self.event_manager as Object,
                    listener_pointer(),
                    event.as_ptr(),
                    true,
                )
            } || !unsafe {
                find(
                    self.event_manager as Object,
                    listener_pointer(),
                    event.as_ptr(),
                )
            } {
                return Err(invalid("Observer event registration was refused"));
            }
        }
        Ok(())
    }
    fn remove_listeners(&mut self) -> io::Result<()> {
        if self.listeners_registered {
            let remove: unsafe extern "C" fn(Object, Object) = unsafe {
                std::mem::transmute(self.function(
                    self.event_manager,
                    0x30,
                    self.engine + 0x5137a0,
                )?)
            };
            unsafe { remove(self.event_manager as Object, listener_pointer()) };
            self.listeners_registered = false;
        }
        Ok(())
    }
    fn publish(&mut self, error: Option<String>) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        let error = if FAILED.load(Ordering::Acquire) {
            Some(error.unwrap_or_else(|| "Observer callback failure latch is set".into()))
        } else {
            error
        };
        let registration = if error.is_none() {
            self.plugins_only()?
        } else {
            Value::Null
        };
        self.closed = true;
        let publication = Publication {
            profile: PROFILE.into(),
            map_name: self.request.profile.map_name.clone(),
            bsp_sha256: self.request.profile.bsp_sha256.clone(),
            samples: std::mem::take(&mut self.samples),
            observer: json!({"api":"ISERVERPLUGINCALLBACKS003","phase":"pre_server_gameframe_after_completed_step","modules":self.modules,"plugin_registration":registration,"callback_sequence":self.callback,"callback_thread":self.thread,"spawn_generation":self.generation,"listeners_registered":self.listeners_registered,"sampling_closed":true,"no_commands_during_measurement":true,"setup_retired":self.setup_retired,"setup":self.setup_receipt,"input":self.request.profile.settings.input,"input_setup":self.input_receipt,"input_acknowledgement":self.input_acknowledgement,"movement":{"object":self.movement,"vtable":self.server+0xc23ec8,"mins_slot":27,"maxs_slot":28,"selector":"m_Local.m_bDucked","probe_route":"GameMovement001::TracePlayerBBox","probe_distance":self.request.profile.settings.probe_distance,"collision_surface_name":"unavailable_borrowed_lifetime"},"native_client":self.native_client,"client_catalog":self.client_catalog,"world":self.world,"player":self.player,"publication_pid":std::process::id()}),
            error,
            sampling_closed: true,
        };
        let bytes = serde_json::to_vec(&publication).map_err(io::Error::other)?;
        if bytes.len() > MAX_OUTPUT {
            return Err(invalid("Observer publication exceeds four MiB"));
        }
        util::atomic_write(&self.directory.join(OUTPUT), &bytes)
    }
    fn entity(&self, object: u64, expected_index: u32) -> io::Result<PlayerSupportEntity> {
        self.memory.range(object, 0x478)?;
        let handle = self.memory.u32(object + 0x474)?;
        let index = handle & 0xffff;
        let serial = handle >> 16;
        let base = self.memory.u64(self.server + 0xc83cf0)?;
        if base != self.server + 0xd09e60
            || index >= 8192
            || handle == u32::MAX
            || index != expected_index
        {
            return Err(invalid("Observer native entity handle/base differs"));
        }
        let row = base + u64::from(index) * 32;
        if self.memory.u64(row + 8)? != object || self.memory.u32(row + 0x10)? != serial {
            return Err(invalid("Observer native entity serial is stale"));
        }
        let classname = self.memory.string(self.memory.u64(object + 0xa8)?, 128)?;
        Ok(PlayerSupportEntity {
            index,
            serial,
            reference_handle: handle,
            address: object,
            classname,
        })
    }

    fn resolve_handle(&self, handle: u32) -> io::Result<Option<PlayerSupportEntity>> {
        if handle == 0 {
            return Ok(None);
        }
        let base = self.memory.u64(self.server + 0xc83cf0)?;
        if base != self.server + 0xd09e60 {
            return Err(invalid("Observer native handle table identity differs"));
        }
        if handle == u32::MAX {
            if self.memory.u32(base + 8191 * 32 + 0x10)? == 0xffff {
                return Err(invalid("Observer invalid handle does not resolve null"));
            }
            return Ok(None);
        }
        let index = handle & 0xffff;
        if index >= 8192 {
            return Err(invalid("Observer handle index exceeds native table"));
        }
        let object = self.memory.u64(base + u64::from(index) * 32 + 8)?;
        let entity = self.entity(object, index)?;
        if entity.reference_handle != handle {
            return Err(invalid("Observer native handle serial differs"));
        }
        Ok(Some(entity))
    }
    fn edict(&self, index: i32) -> io::Result<(u64, u64)> {
        let get: unsafe extern "C" fn(Object, i32) -> Object = unsafe {
            std::mem::transmute(self.function(
                self.engine_interface,
                0x98,
                self.engine + 0x49aee0,
            )?)
        };
        let index_of: unsafe extern "C" fn(Object, Object) -> i32 = unsafe {
            std::mem::transmute(self.function(
                self.engine_interface,
                0x90,
                self.engine + 0x49a430,
            )?)
        };
        let base: unsafe extern "C" fn(Object, Object) -> Object = unsafe {
            std::mem::transmute(self.function(
                self.entity_interface,
                0x30,
                self.server + 0x59cdc0,
            )?)
        };
        let reverse: unsafe extern "C" fn(Object, Object) -> Object = unsafe {
            std::mem::transmute(self.function(
                self.entity_interface,
                0x28,
                self.server + 0x59b5b0,
            )?)
        };
        let edict = unsafe { get(self.engine_interface as Object, index) } as u64;
        if edict == 0 {
            return Ok((0, 0));
        }
        if unsafe { index_of(self.engine_interface as Object, edict as Object) } != index {
            return Err(invalid("Observer public edict index roundtrip differs"));
        }
        let object = unsafe { base(self.entity_interface as Object, edict as Object) } as u64;
        if object == 0 {
            return Ok((edict, 0));
        }
        if unsafe { reverse(self.entity_interface as Object, object as Object) } as u64 != edict {
            return Err(invalid("Observer public entity/edict roundtrip differs"));
        }
        Ok((edict, object))
    }
    fn plugins_only(&self) -> io::Result<Value> {
        let manager = self.memory.u64(self.engine + 0xa012d8)?;
        if manager != self.engine + 0xc38720 || self.memory.i32(manager + 0x18)? != 1 {
            return Err(invalid(
                "Observer requires exactly one loaded server plugin",
            ));
        }
        let entries = self.memory.u64(manager + 8)?;
        let entry = self.memory.u64(entries)?;
        self.memory.range(entry, 0xa0)?;
        if self.memory.u8(entry + 0x80)? != 0
            || self.memory.u64(entry + 0x88)? != (&raw const PLUGIN) as u64
            || self.memory.i32(entry + 0x90)? != 3
            || self.memory.u64(entry + 0x98)? == 0
        {
            return Err(invalid("Observer plugin registration identity differs"));
        }
        Ok(
            json!({"manager":manager,"count":1,"entry":entry,"callback":(&raw const PLUGIN) as u64,"version":3,"disabled":false,"module_handle":self.memory.u64(entry+0x98)?}),
        )
    }
    fn physics_idle(&self) -> io::Result<()> {
        let get: unsafe extern "C" fn(Object, i32) -> Object = unsafe {
            std::mem::transmute(self.function(
                self.physics_interface,
                0x38,
                self.vphysics + 0x1cc80,
            )?)
        };
        for index in 0..=16 {
            let environment = unsafe { get(self.physics_interface as Object, index) } as u64;
            if environment == 0 {
                return Ok(());
            }
            if index == 16 {
                return Err(invalid("Observer exceeds sixteen VPhysics environments"));
            }
            self.object(environment, self.vphysics + 0x198288)?;
            let simulating: unsafe extern "C" fn(Object) -> bool = unsafe {
                std::mem::transmute(self.function(environment, 0x120, self.vphysics + 0x2bb80)?)
            };
            if unsafe { simulating(environment as Object) } {
                return Err(invalid("Observer callback overlaps VPhysics simulation"));
            }
        }
        Err(invalid("Observer VPhysics enumeration has no sentinel"))
    }
    fn surface(&self, player: u64) -> io::Result<(i32, u64, Option<PlayerSupportSurface>)> {
        let index = self.memory.i32(player + 0x1280)?;
        let data = self.memory.u64(player + 0x1288)?;
        if data == 0 {
            return Ok((index, data, None));
        }
        let mut surface = self.surface_record(index, Some(data))?;
        let friction = self.memory.f32(player + 0x1290)?;
        let material = self.memory.u8(player + 0x1294)?;
        if material != surface.material || !(0.0..=1.0).contains(&friction) {
            return Err(invalid("Observer cached surface material/friction differs"));
        }
        surface.friction = friction;
        Ok((index, data, Some(surface)))
    }

    fn surface_record(
        &self,
        index: i32,
        cached_data: Option<u64>,
    ) -> io::Result<PlayerSupportSurface> {
        let helper = self.memory.u64(self.server + 0xd59440)?;
        if helper != self.server + 0xd59460 {
            return Err(invalid("Observer movement helper singleton differs"));
        }
        self.object(helper, self.server + 0xb563e8)?;
        self.function(helper, 0x50, self.server + 0x5df420)?;
        if self.memory.u64(self.server + 0xd5ee88)? != self.surface_props {
            return Err(invalid("Observer movement/surface API identity differs"));
        }
        let count = self.memory.i32(self.surface_props + 0x190)?;
        let count_fn: unsafe extern "C" fn(Object) -> i32 = unsafe {
            std::mem::transmute(self.function(self.surface_props, 0x18, self.vphysics + 0x339b0)?)
        };
        if count <= 0 || count > 4096 || unsafe { count_fn(self.surface_props as Object) } != count
        {
            return Err(invalid("Observer native surface table count is invalid"));
        }
        if index < 0 || index >= count.min(128) {
            return Err(invalid(
                "Observer cached surface uses native fallback/special index",
            ));
        }
        let records = self.memory.u64(self.surface_props + 0x180)?;
        let data = records
            .checked_add(index as u64 * 0x78 + 0x14)
            .ok_or_else(|| invalid("Observer surface record address overflows"))?;
        if cached_data.is_some_and(|cached| cached != data) {
            return Err(invalid("Observer cached surface data identity differs"));
        }
        self.memory.range(data, 0x64)?;
        let get_data: unsafe extern "C" fn(Object, i32) -> Object = unsafe {
            std::mem::transmute(self.function(self.surface_props, 0x30, self.vphysics + 0x33ee0)?)
        };
        let get_name: unsafe extern "C" fn(Object, i32) -> *const c_char = unsafe {
            std::mem::transmute(self.function(self.surface_props, 0x40, self.vphysics + 0x33fb0)?)
        };
        if unsafe { get_data(self.surface_props as Object, index) } as u64 != data {
            return Err(invalid("Observer surface API resolved a fallback"));
        }
        let name_pointer = unsafe { get_name(self.surface_props as Object, index) } as u64;
        let name = self.memory.string(name_pointer, 256)?;
        if name.is_empty() {
            return Err(invalid("Observer support surface name is empty"));
        }
        let get_physics: unsafe extern "C" fn(Object, i32, *mut f32, *mut f32, *mut f32, *mut f32) = unsafe {
            std::mem::transmute(self.function(self.surface_props, 0x28, self.vphysics + 0x34200)?)
        };
        let mut friction = f32::NAN;
        unsafe {
            get_physics(
                self.surface_props as Object,
                index,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut friction,
                std::ptr::null_mut(),
            )
        };
        let material = self.memory.u8(data + 0x48)?;
        if !friction.is_finite() || friction < 0.0 {
            return Err(invalid(
                "Observer native surface friction is not finite and nonnegative",
            ));
        }
        Ok(PlayerSupportSurface {
            index,
            name,
            data_address: data,
            friction,
            material,
        })
    }

    fn movement_ready(&self, player: &PlayerSupportEntity, probe: bool) -> io::Result<bool> {
        if self.movement != self.server + 0xdcfe40 {
            return Err(invalid("Observer movement singleton differs"));
        }
        self.object(self.movement, self.server + 0xc23ec8)?;
        self.function(self.movement, 0xd8, self.server + 0x5a1650)?;
        self.function(self.movement, 0xe0, self.server + 0x5a1750)?;
        if self.memory.u64(self.movement + 8)? != player.address {
            return Ok(false);
        }
        if probe {
            self.function(self.movement, 0x48, self.server + 0x5a63c0)?;
            self.function(self.movement, 0x58, self.server + 0x812210)?;
            self.object(self.trace_interface, self.engine + 0x8e23a0)?;
            self.function(self.trace_interface, 0x20, self.engine + 0x50b210)?;
            if self.memory.u64(self.server + 0xce3c00)? != self.movement
                || self.memory.u64(self.server + 0xce5f78)? != self.server + 0xe0e380
                || self.memory.u64(self.server + 0xd53300)? != self.trace_interface
            {
                return Err(invalid("Observer movement/trace static binding differs"));
            }
            let move_data = self.memory.u64(self.movement + 0x10)?;
            if move_data != self.server + 0xe0e380 {
                return Ok(false);
            }
            let handle = self.memory.u32(move_data + 4)?;
            if handle != player.reference_handle {
                return Ok(false);
            }
            let resolved = self
                .resolve_handle(handle)?
                .ok_or_else(|| invalid("Observer movement skip handle resolves null"))?;
            if resolved.address != player.address {
                return Err(invalid(
                    "Observer movement skip handle resolves another entity",
                ));
            }
        }
        Ok(true)
    }

    fn hull(&self, player: &PlayerSupportEntity) -> io::Result<(f32, bool, [f32; 3], [f32; 3])> {
        if !self.movement_ready(player, false)? {
            return Err(invalid(
                "Observer movement hull has no current player context",
            ));
        }
        let rules = self.memory.u64(self.server + 0xd54730)?;
        self.object(rules, self.server + 0xc24758)?;
        self.function(rules, 0x100, self.server + 0x816520)?;
        let table = self.server + 0xdd3420;
        self.memory.range(table, 9 * 12)?;
        let observer = self.memory.u8(player.address + 0xdfc)?;
        let ducked = self.memory.u8(player.address + 0xa7c)?;
        if ducked > 1 {
            return Err(invalid("Observer movement duck Boolean is invalid"));
        }
        let scale = self.memory.f32(player.address + 0x4d0)?;
        if scale <= 0.0 || scale > 1024.0 {
            return Err(invalid(
                "Observer player scale is outside its finite positive profile",
            ));
        }
        let mins_fn: unsafe extern "C" fn(Object) -> NativeVector = unsafe {
            std::mem::transmute(self.function(self.movement, 0xd8, self.server + 0x5a1650)?)
        };
        let maxs_fn: unsafe extern "C" fn(Object) -> NativeVector = unsafe {
            std::mem::transmute(self.function(self.movement, 0xe0, self.server + 0x5a1750)?)
        };
        let mins = unsafe { mins_fn(self.movement as Object) }.array();
        let maxs = unsafe { maxs_fn(self.movement as Object) }.array();
        if !self.movement_ready(player, false)?
            || self.memory.u64(self.server + 0xd54730)? != rules
            || self.memory.u8(player.address + 0xdfc)? != observer
            || self.memory.u8(player.address + 0xa7c)? != ducked
            || self.memory.f32(player.address + 0x4d0)?.to_bits() != scale.to_bits()
        {
            return Err(invalid(
                "Observer movement hull context/selectors changed during getters",
            ));
        }
        ordered(mins, maxs)?;
        Ok((scale, ducked != 0, mins, maxs))
    }

    fn probe(
        &self,
        player: &PlayerSupportEntity,
        origin: [f32; 3],
        distance: u16,
    ) -> io::Result<PlayerSupportProbe> {
        if !self.movement_ready(player, true)? {
            return Err(invalid(
                "Observer diagnostic sweep has no completed movement context",
            ));
        }
        let before_hull = self.hull(player)?;
        let mask_fn: unsafe extern "C" fn(Object, bool) -> u32 = unsafe {
            std::mem::transmute(self.function(self.movement, 0x58, self.server + 0x812210)?)
        };
        let mask = unsafe { mask_fn(self.movement as Object, false) };
        if mask != PLAYER_SUPPORT_MOVEMENT_MASK {
            return Err(invalid("Observer ordinary player movement mask differs"));
        }
        let requested_end = [origin[0], origin[1], origin[2] - f32::from(distance)];
        if [origin, requested_end].into_iter().flatten().any(|value| {
            !value.is_finite() || value.abs() > crate::MAX_PLAYER_SUPPORT_PROBE_COORDINATE
        }) || requested_end[2] >= origin[2]
        {
            return Err(invalid(
                "Observer diagnostic sweep requires finite downward endpoints within ±1048576 world units",
            ));
        }
        let start = NativeVector::from(origin);
        let end = NativeVector::from(requested_end);
        let mut trace = NativeTrace([0; 104]);
        let trace_fn: unsafe extern "C" fn(
            Object,
            *const NativeVector,
            *const NativeVector,
            u32,
            i32,
            *mut NativeTrace,
        ) = unsafe {
            std::mem::transmute(self.function(self.movement, 0x48, self.server + 0x5a63c0)?)
        };
        unsafe {
            trace_fn(
                self.movement as Object,
                &start,
                &end,
                mask,
                PLAYER_SUPPORT_COLLISION_GROUP,
                &mut trace,
            )
        };
        if !self.movement_ready(player, true)?
            || self.hull(player)? != before_hull
            || self
                .memory
                .vector(player.address + 0x3b4)?
                .map(f32::to_bits)
                != origin.map(f32::to_bits)
            || self.entity(player.address, player.index)?.reference_handle
                != player.reference_handle
        {
            return Err(invalid(
                "Observer diagnostic sweep changed its player/movement binding",
            ));
        }
        let all_solid = trace.0[0x36];
        let start_solid = trace.0[0x37];
        if all_solid > 1 || start_solid > 1 {
            return Err(invalid(
                "Observer diagnostic sweep solid Boolean is invalid",
            ));
        }
        let fraction = trace.f32(0x2c);
        let status =
            PlayerSupportProbeStatus::from_native(fraction, start_solid != 0, all_solid != 0);
        let hit_entity_address = trace.u64(0x58);
        let hit_entity = if hit_entity_address == 0 {
            None
        } else {
            let handle = self.memory.u32(hit_entity_address + 0x474)?;
            let entity = self
                .resolve_handle(handle)?
                .ok_or_else(|| invalid("Observer sweep hit has a null reference handle"))?;
            let (_, roundtrip) = self.edict(entity.index as i32)?;
            if entity.address != hit_entity_address || roundtrip != entity.address {
                return Err(invalid("Observer sweep hit entity/edict identity differs"));
            }
            Some(entity)
        };
        let surface_properties = trace.u16(0x48) as i16;
        let surface = if status == PlayerSupportProbeStatus::Impact {
            Some(self.surface_record(i32::from(surface_properties), None)?)
        } else {
            None
        };
        let probe = PlayerSupportProbe {
            distance,
            start: origin,
            requested_end,
            mask,
            collision_group: PLAYER_SUPPORT_COLLISION_GROUP,
            native_start: trace.vector(0),
            native_end: trace.vector(0x0c),
            fraction,
            fraction_left_solid: trace.f32(0x38),
            all_solid: all_solid != 0,
            start_solid: start_solid != 0,
            status,
            plane: PlayerSupportProbePlane {
                available: status == PlayerSupportProbeStatus::Impact,
                normal: trace.vector(0x18),
                distance: trace.f32(0x24),
                plane_type: trace.0[0x28],
                sign_bits: trace.0[0x29],
            },
            contents: trace.u32(0x30) as i32,
            displacement_flags: trace.u16(0x34),
            hit_group: trace.u32(0x50) as i32,
            physics_bone: trace.u16(0x54) as i16,
            hitbox: trace.u32(0x60) as i32,
            hit_entity_address,
            hit_entity,
            surface_name_address: trace.u64(0x40),
            collision_surface_name_available: false,
            surface_properties,
            surface_flags: trace.u16(0x4a),
            surface,
        };
        crate::player_support_types::validate_probe(&probe, distance, origin)?;
        if let Some(hit) = &probe.hit_entity {
            let current = self.entity(hit.address, hit.index)?;
            if current.reference_handle != hit.reference_handle {
                return Err(invalid(
                    "Observer sweep hit entity retired before publication",
                ));
            }
        }
        self.physics_idle()?;
        Ok(probe)
    }

    fn frame(&mut self, simulating: bool) -> io::Result<Option<SetupCall>> {
        if self.closed {
            return Ok(None);
        }
        if FAILED.load(Ordering::Acquire) {
            return Err(invalid("Observer callback failure latch is set"));
        }
        self.callback += 1;
        let thread = unsafe { libc::syscall(libc::SYS_gettid) } as u64;
        if self.thread == 0 {
            self.thread = thread;
        } else if self.thread != thread {
            return Err(invalid("Observer GameFrame thread changed"));
        }
        if !simulating || !self.activated || !self.map_initialized {
            self.previous = None;
            return Ok(None);
        }
        self.memory.refresh()?;
        self.plugins_only()?;
        if !self.setup_retired {
            self.previous = None;
            return Ok(None);
        }
        let globals = self.memory.u64(self.server + 0xd53080)?;
        let tick = self.memory.i32(globals + 0x18)?;
        let time = self.memory.f32(globals + 0x0c)?;
        let server_frame_time = self.memory.f32(globals + 0x10)?;
        let server_tick_interval = self.memory.f32(globals + 0x1c)?;
        if server_frame_time < 0.0 || server_tick_interval <= 0.0 || server_tick_interval > 1.0 {
            return Err(invalid("Observer native frame/tick interval is invalid"));
        }
        let sole = self.sole_active_client(self.player_edict)?;
        let edict = sole.native.edict;
        let player = sole.player;
        self.client_catalog = sole.catalog;
        self.memory.range(player.address, 0x1298)?;
        let health = self.memory.i32(player.address + 0x18c)?;
        let team = self.memory.i32(player.address + 0x324)?;
        let move_type = self.memory.u8(player.address + 0x256)?;
        let flags = self.memory.u32(player.address + 0x214)?;
        let buttons = self.memory.i32(player.address + 0xd88)?;
        if health <= 0
            || team != i32::from(self.request.profile.settings.team)
            || move_type != 2
            || flags & 0x40 != 0
            || self.memory.u8(player.address + 0x190)? != 0
        {
            if self.first_time.is_some() {
                return Err(invalid("Observed player left living WALK state"));
            }
            self.previous = None;
            return Ok(None);
        }
        let expected_buttons = if self.input_retired {
            self.request.profile.settings.input.expected_buttons()
        } else {
            0
        };
        if buttons != expected_buttons {
            if self.first_time.is_some() || !self.input_retired {
                return Err(invalid(format!(
                    "Observer native player buttons differ: observed={buttons:#x}, expected={expected_buttons:#x}"
                )));
            }
            self.previous = None;
            return Ok(None);
        }
        if self.memory.u8(player.address + 0xdfc)? & 8 != 0 {
            return Err(invalid("Observer player is an observer"));
        }
        if self.generation == 0 {
            return Ok(None);
        }
        if let Some(identity) = &self.player {
            if identity.address != player.address
                || identity.reference_handle != player.reference_handle
                || self.player_edict != edict
            {
                return Err(invalid("Observed player identity changed"));
            }
        } else {
            self.player = Some(player.clone());
            self.player_edict = edict;
        }
        let (world_edict, world_object) = self.edict(0)?;
        self.object(world_object, self.server + 0xc10d98)?;
        let world = self.entity(world_object, 0)?;
        if world.classname != "worldspawn" {
            return Err(invalid("Observer world entity name differs"));
        }
        if let Some(identity) = &self.world {
            if identity.address != world.address
                || identity.reference_handle != world.reference_handle
                || self.world_edict != world_edict
            {
                return Err(invalid("Observed world identity changed"));
            }
        } else {
            self.world = Some(world);
            self.world_edict = world_edict;
        }
        let previous = self.previous.take();
        self.previous = Some(Previous {
            callback: self.callback,
            tick,
            time,
            generation: self.generation,
        });
        let Some(previous) = previous else {
            return Ok(None);
        };
        if previous.generation != self.generation {
            return Ok(None);
        }
        if tick <= previous.tick || time <= previous.time {
            return Err(invalid(
                "Observer native server time/tick failed to advance",
            ));
        }
        if time - previous.time > 1.0 {
            return Err(invalid("Observer native step exceeds one second"));
        }
        let simulation_tick = self.memory.i32(player.address + 0x208)?;
        if simulation_tick != previous.tick {
            return Err(invalid(
                "Observer player did not complete the preceding server step",
            ));
        }
        if self.memory.u32(player.address + 0x210)? & 0x1800 != 0
            || self.memory.u64(player.address + 0x10c8)? != 0
            || self.memory.u64(self.server + 0xcf9ad8)? != 0
        {
            return Err(invalid(
                "Observer callback has dirty absolute caches or active player command/prediction",
            ));
        }
        self.physics_idle()?;
        if !self.movement_ready(
            &player,
            self.request.profile.settings.probe_distance.is_some(),
        )? {
            if self.first_time.is_some() {
                return Err(invalid(
                    "Observer completed movement context changed during measurement",
                ));
            }
            self.previous = None;
            return Ok(None);
        }
        if !self.input_retired {
            return self.prepare_input_setup(&player).map(Some);
        }
        if self.input_acknowledgement.is_null() {
            self.input_acknowledgement = json!({"input":self.request.profile.settings.input,"expected_buttons":expected_buttons,"observed_buttons":buttons,"callback_sequence":self.callback,"server_tick":tick,"server_time":time,"spawn_generation":self.generation,"player":player,"acknowledged_before_measurement":true});
            self.previous = None;
            return Ok(None);
        }
        let first = *self.first_time.get_or_insert(time);
        if self
            .sampled_generation
            .is_some_and(|generation| generation != self.generation)
        {
            return Err(invalid(
                "Observed player spawn/round/team generation changed",
            ));
        }
        self.sampled_generation = Some(self.generation);
        let tickbase = self.memory.i32(player.address + 0x113c)?;
        if self.last_tickbase.is_some_and(|last| tickbase < last) {
            return Err(invalid("Observer player tickbase regressed"));
        }
        if self.last_tickbase != Some(tickbase) {
            self.last_tickbase_advance = Some(time);
        }
        if self
            .last_tickbase_advance
            .is_some_and(|last| time - last > 2.0)
        {
            return Err(invalid(
                "Observer player tickbase stalled for two native seconds",
            ));
        }
        self.last_tickbase = Some(tickbase);
        if self.last_time.is_some_and(|last| {
            (time - last) * 1000.0 < (self.request.profile.settings.interval_ms as f32)
        }) {
            return Ok(None);
        }
        if self.samples.len() == self.request.profile.settings.max_samples {
            return Err(invalid(
                "Observer exhausted sample capacity before requested duration",
            ));
        }
        let ground_handle = self.memory.u32(player.address + 0x330)?;
        let ground = self.resolve_handle(ground_handle)?;
        if let Some(ground) = &ground {
            let world = self
                .world
                .as_ref()
                .ok_or_else(|| invalid("Observer world identity missing"))?;
            if ground.address != world.address || ground.reference_handle != world.reference_handle
            {
                return Err(invalid(
                    "Observer support is outside the static world profile",
                ));
            }
        }
        let (cached_surface_index, cached_surface_data_address, surface) =
            self.surface(player.address)?;
        if ground.is_some() && surface.is_none() {
            return Err(invalid(
                "Observer current ground has no bound support surface",
            ));
        }
        let (model_scale, ducked, hull_mins, hull_maxs) = self.hull(&player)?;
        let collision_mins = self.memory.vector(player.address + 0x290)?;
        let collision_maxs = self.memory.vector(player.address + 0x29c)?;
        ordered(collision_mins, collision_maxs)?;
        let origin = self.memory.vector(player.address + 0x3b4)?;
        let velocity = self.memory.vector(player.address + 0x3c0)?;
        let absolute_velocity = self.memory.vector(player.address + 0x34c)?;
        let base_velocity = self.memory.vector(player.address + 0x340)?;
        let probe = self
            .request
            .profile
            .settings
            .probe_distance
            .map(|distance| self.probe(&player, origin, distance))
            .transpose()?;
        self.physics_idle()?;
        let row = PlayerSupportSample {
            phase: "pre_server_gameframe_after_completed_step".into(),
            callback_sequence: self.callback,
            prior_simulating_callback_sequence: previous.callback,
            prior_server_tick: previous.tick,
            prior_server_time: previous.time,
            simulating,
            callback_thread: thread,
            spawn_generation: self.generation,
            player_simulation_tick: simulation_tick,
            ordinal: self.samples.len(),
            server_tick: tick,
            server_time: time,
            server_frame_time,
            server_tick_interval,
            player_tickbase: tickbase,
            origin,
            velocity,
            absolute_velocity,
            base_velocity,
            collision_mins,
            collision_maxs,
            hull_mins,
            hull_maxs,
            model_scale,
            ground_handle,
            ground_resolution: if ground_handle == 0 {
                "null_handle"
            } else if ground_handle == u32::MAX {
                "invalid_handle_resolves_null"
            } else {
                "serial_resolved_world"
            }
            .into(),
            surface_is_cached: ground.is_none(),
            ground,
            surface,
            cached_surface_index,
            cached_surface_data_address,
            flags,
            buttons,
            input: self.request.profile.settings.input,
            expected_buttons,
            input_acknowledged: !self.input_acknowledgement.is_null(),
            health,
            team,
            move_type,
            ducked,
            probe,
            player,
        };
        let (current_edict, current_player) = self.edict(row.player.index as i32)?;
        let (current_world_edict, current_world) = self.edict(0)?;
        let current_player_identity = self.entity(current_player, row.player.index)?;
        let current_world_identity = self.entity(current_world, 0)?;
        let current_hull = self.hull(&row.player)?;
        if current_edict != self.player_edict
            || current_player_identity.reference_handle != row.player.reference_handle
            || current_player != row.player.address
            || current_world_edict != self.world_edict
            || self.world.as_ref().is_none_or(|world| {
                world.address != current_world_identity.address
                    || world.reference_handle != current_world_identity.reference_handle
            })
            || self.memory.u64(self.server + 0xd53080)? != globals
            || !self.movement_ready(&row.player, row.probe.is_some())?
            || self.memory.u8(row.player.address + 0xa7c)? != u8::from(row.ducked)
            || self.memory.i32(row.player.address + 0xd88)? != row.buttons
            || current_hull.0.to_bits() != row.model_scale.to_bits()
            || current_hull.1 != row.ducked
            || current_hull.2.map(f32::to_bits) != row.hull_mins.map(f32::to_bits)
            || current_hull.3.map(f32::to_bits) != row.hull_maxs.map(f32::to_bits)
        {
            return Err(invalid(
                "Observer player/world/clock identity changed before row commit",
            ));
        }
        self.client_catalog = self.sole_active_client(self.player_edict)?.catalog;
        self.plugins_only()?;
        if FAILED.load(Ordering::Acquire) {
            return Err(invalid("Observer callback failed before row commit"));
        }
        self.samples.push(row);
        self.last_time = Some(time);
        if (time - first) * 1000.0 >= self.request.profile.settings.duration_ms as f32 {
            self.publish(None)?;
        }
        Ok(None)
    }
}

fn ordered(mins: [f32; 3], maxs: [f32; 3]) -> io::Result<()> {
    if (0..3)
        .any(|axis| !mins[axis].is_finite() || !maxs[axis].is_finite() || mins[axis] >= maxs[axis])
    {
        return Err(invalid(
            "Observer hull/collision bounds are not finite and ordered",
        ));
    }
    Ok(())
}

fn input_commands(input: PlayerSupportInput) -> &'static CStr {
    match input {
        PlayerSupportInput::Idle => c"",
        PlayerSupportInput::Forward => c"+forward",
        PlayerSupportInput::Crouch => c"+duck",
        PlayerSupportInput::ForwardCrouch => c"+forward; +duck",
    }
}

unsafe extern "C" fn load(_this: *mut Plugin, engine: Factory, game: Factory) -> bool {
    std::panic::catch_unwind(|| {
        let Some(directory) = std::env::var_os("HAMMERR_PLAYER_SUPPORT_DIRECTORY") else {
            return false;
        };
        if DIRECTORY.set(PathBuf::from(&directory)).is_err() {
            mark_failed("Observer loaded twice");
            return false;
        }
        let Some(mut guard) = lock_state() else {
            return false;
        };
        if guard.is_some() {
            return false;
        }
        let mut state = match State::new(PathBuf::from(directory), engine, game) {
            Ok(state) => state,
            Err(error) => {
                eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}");
                return false;
            }
        };
        if let Err(error) = state.register() {
            if let Err(cleanup) = state.remove_listeners() {
                eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}; listener cleanup: {cleanup}");
            } else {
                eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}");
            }
            return false;
        }
        *guard = Some(state);
        true
    })
    .unwrap_or(false)
}
unsafe extern "C" fn unload(_this: *mut Plugin) {
    if let Some(mut guard) = lock_state()
        && let Some(state) = guard.as_mut()
    {
        if let Err(error) = state.remove_listeners() {
            eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}");
        }
        if !state.closed
            && let Err(error) = state.publish(Some("Observer unloaded before completion".into()))
        {
            eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}");
        }
    }
}
unsafe extern "C" fn pause(_this: *mut Plugin) {
    invalidate("Observer plugin paused");
}
unsafe extern "C" fn unpause(_this: *mut Plugin) {
    invalidate("Observer plugin unpaused");
}
unsafe extern "C" fn description(_this: *mut Plugin) -> *const c_char {
    c"HammeRR player support observer".as_ptr()
}
unsafe extern "C" fn level_init(_this: *mut Plugin, name: *const c_char) {
    if let Some(mut guard) = lock_state()
        && let Some(state) = guard.as_mut()
    {
        if name.is_null()
            || unsafe { CStr::from_ptr(name) }.to_bytes()
                != state.request.profile.map_name.as_bytes()
        {
            if let Err(error) = state.publish(Some("Observer private map identity differs".into()))
            {
                eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}");
            }
            return;
        }
        if state.map_initialized {
            if let Err(error) = state.publish(Some("Observer map initialized twice".into())) {
                eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}");
            }
            return;
        }
        state.map_initialized = true;
    }
}
unsafe extern "C" fn server_activate(_this: *mut Plugin, _edicts: Object, _count: i32, _max: i32) {
    if let Some(mut guard) = lock_state()
        && let Some(state) = guard.as_mut()
    {
        state.activated = true;
        state.previous = None;
    }
}
unsafe extern "C" fn game_frame(_this: *mut Plugin, simulating: bool) {
    let result = std::panic::catch_unwind(|| {
        let call = {
            let Some(mut guard) = lock_state() else {
                return;
            };
            let Some(state) = guard.as_mut() else {
                mark_failed("Observer GameFrame state is unavailable");
                return;
            };
            let prepared = match state.prepare_gameframe_setup() {
                Ok(Some(call)) => Ok(Some(call)),
                Ok(None) => state.frame(simulating),
                Err(error) => Err(error),
            };
            match prepared {
                Ok(Some(call)) => call,
                Ok(None) => return,
                Err(error) => {
                    state.refuse(error);
                    return;
                }
            }
        };
        let command: unsafe extern "C" fn(Object, Object, *const c_char, ...) =
            unsafe { std::mem::transmute(call.function) };
        unsafe {
            match call.kind {
                SetupKind::Team(team) => command(
                    call.engine as Object,
                    call.edict as Object,
                    c"jointeam %i; joinclass 1".as_ptr(),
                    team,
                ),
                SetupKind::Input(input) => command(
                    call.engine as Object,
                    call.edict as Object,
                    input_commands(input).as_ptr(),
                ),
            }
        }
        let Some(mut guard) = lock_state() else {
            return;
        };
        let Some(state) = guard.as_mut() else {
            mark_failed("Observer setup state disappeared after queueing");
            return;
        };
        if let Err(error) = state.finish_client_setup(call) {
            state.refuse(error);
        }
    });
    if result.is_err() {
        mark_failed("Observer callback panicked");
    }
}

fn invalidate(reason: &str) {
    if let Some(mut guard) = lock_state()
        && let Some(state) = guard.as_mut()
        && !state.closed
        && let Err(error) = state.publish(Some(reason.into()))
    {
        eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}");
    }
}
unsafe extern "C" fn level_shutdown(_this: *mut Plugin) {
    if let Some(mut guard) = lock_state()
        && let Some(state) = guard.as_mut()
        && !state.closed
        && (state.map_initialized || state.activated)
        && let Err(error) = state.publish(Some("Observed level shut down".into()))
    {
        mark_failed(&error.to_string());
    }
}
unsafe extern "C" fn client_disconnect(_this: *mut Plugin, edict: Object) {
    retired(edict);
}
unsafe extern "C" fn edict_freed(_this: *mut Plugin, edict: Object) {
    retired(edict);
}
fn retired(edict: Object) {
    if let Some(mut guard) = lock_state()
        && let Some(state) = guard.as_mut()
        && !state.closed
        && (state.pending_client == Some(edict as u64)
            || ((edict as u64 == state.player_edict || edict as u64 == state.world_edict)
                && state.player_edict != 0))
        && let Err(error) = state.publish(Some("Observed player/world edict retired".into()))
    {
        eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}");
    }
}
unsafe extern "C" fn listener_destructor(_this: *mut Listener) {}
unsafe extern "C" fn event(_this: *mut Listener, _event: Object) {
    if let Some(mut guard) = lock_state()
        && let Some(state) = guard.as_mut()
        && !state.closed
    {
        state.generation = state.generation.saturating_add(1);
        state.previous = None;
        state.input_acknowledgement = Value::Null;
        if (state.first_time.is_some() || state.input_attempted)
            && let Err(error) = state.publish(Some(
                "Observed player spawn/death/team/round generation changed".into(),
            ))
        {
            eprintln!("HAMMERR_PLAYER_SUPPORT_ERROR {error}");
        }
    }
}
unsafe extern "C" fn client_active(_this: *mut Plugin, edict: Object) {
    let result = std::panic::catch_unwind(|| {
        if let Some(mut guard) = lock_state()
            && let Some(state) = guard.as_mut()
            && let Err(error) = state.note_client_active(edict as u64)
        {
            state.refuse(error);
        }
    });
    if result.is_err() {
        mark_failed("Observer ClientActive callback panicked");
    }
}

unsafe extern "C" fn client_put(_this: *mut Plugin, _edict: Object, _name: *const c_char) {}
unsafe extern "C" fn command_client(_this: *mut Plugin, _index: i32) {}
unsafe extern "C" fn client_settings(_this: *mut Plugin, _edict: Object) {}
unsafe extern "C" fn client_connect(
    _this: *mut Plugin,
    _allow: *mut bool,
    _edict: Object,
    _name: *const c_char,
    _address: *const c_char,
    _reject: *mut c_char,
    _length: i32,
) -> i32 {
    0
}
unsafe extern "C" fn client_command(_this: *mut Plugin, _edict: Object, _command: Object) -> i32 {
    0
}
unsafe extern "C" fn network_validated(
    _this: *mut Plugin,
    _name: *const c_char,
    _network: *const c_char,
) -> i32 {
    0
}
unsafe extern "C" fn cvar_finished(
    _this: *mut Plugin,
    _cookie: i32,
    _edict: Object,
    _status: i32,
    _name: *const c_char,
    _value: *const c_char,
) {
}
unsafe extern "C" fn edict_allocated(_this: *mut Plugin, _edict: Object) {}
