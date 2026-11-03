use std::{fmt::Display, time::Instant};

use map_core::transfer::MAX_BYTES;
use x11rb::{
    COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE,
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ConnectionExt, CreateWindowAux, EventMask, GetPropertyReply, Property,
            Window, WindowClass,
        },
    },
    rust_connection::RustConnection,
};

const MAX_TARGETS: usize = 256;
const MAX_EVENTS: usize = 32768;
const MAX_CHUNKS: usize = 8192;
const TEXT_TARGETS: [&str; 5] = [
    "UTF8_STRING",
    "text/plain;charset=utf-8",
    "text/plain;charset=UTF-8",
    "text/plain",
    "STRING",
];

struct Reader {
    connection: RustConnection,
    deadline: Instant,
    window: Window,
    clipboard: Atom,
    property: Atom,
    incr: Atom,
    events: usize,
}

fn error(error: impl Display) -> String {
    format!("Cannot read configured X11 clipboard: {error}")
}

fn atom(connection: &RustConnection, name: &str) -> Result<Atom, String> {
    connection
        .intern_atom(false, name.as_bytes())
        .map_err(error)?
        .reply()
        .map(|reply| reply.atom)
        .map_err(error)
}

impl Reader {
    fn next_event(&mut self) -> Result<Event, String> {
        loop {
            if Instant::now() >= self.deadline {
                return Err("X11 clipboard read timed out after 5 seconds".into());
            }
            if let Some(event) = self.connection.poll_for_event().map_err(error)? {
                self.events += 1;
                if self.events > MAX_EVENTS {
                    return Err("X11 clipboard exceeded its event limit".into());
                }
                return Ok(event);
            }
            super::wait_readable(self.connection.stream(), self.deadline)?;
        }
    }

    fn property(&self, limit: usize) -> Result<GetPropertyReply, String> {
        let words =
            u32::try_from(limit.div_ceil(4)).map_err(|_| "Invalid X11 clipboard property bound")?;
        let reply = self
            .connection
            .get_property(false, self.window, self.property, AtomEnum::ANY, 0, words)
            .map_err(error)?
            .reply()
            .map_err(error)?;
        if reply.bytes_after != 0 || reply.value.len() > limit {
            return Err("X11 clipboard property exceeds its bounded text limit".into());
        }
        Ok(reply)
    }

    fn acknowledge(&self) -> Result<(), String> {
        self.connection
            .delete_property(self.window, self.property)
            .map_err(error)?;
        self.connection.flush().map_err(error)
    }

    fn convert(
        &mut self,
        target: Atom,
        data_type: Atom,
        format: u8,
        limit: usize,
    ) -> Result<Vec<u8>, String> {
        self.connection
            .convert_selection(
                self.window,
                self.clipboard,
                target,
                self.property,
                CURRENT_TIME,
            )
            .map_err(error)?;
        self.connection.flush().map_err(error)?;
        loop {
            if let Event::SelectionNotify(event) = self.next_event()?
                && event.requestor == self.window
                && event.selection == self.clipboard
                && event.target == target
            {
                if event.property != self.property {
                    return Err("X11 clipboard owner refused the requested text conversion".into());
                }
                break;
            }
        }
        let reply = self.property(limit)?;
        if reply.type_ != self.incr {
            validate(&reply, data_type, format)?;
            self.acknowledge()?;
            return Ok(reply.value);
        }
        if reply.format != 32 || reply.value.len() != 4 {
            return Err("X11 clipboard has an invalid incremental-transfer header".into());
        }
        let announced = reply
            .value32()
            .and_then(|mut values| values.next())
            .ok_or("X11 clipboard has no incremental-transfer size")?;
        if announced as usize > limit {
            return Err("X11 clipboard incremental transfer exceeds its bounded text limit".into());
        }
        self.acknowledge()?;
        let mut bytes = Vec::new();
        for _ in 0..MAX_CHUNKS {
            loop {
                if let Event::PropertyNotify(event) = self.next_event()?
                    && event.window == self.window
                    && event.atom == self.property
                    && event.state == Property::NEW_VALUE
                {
                    break;
                }
            }
            let reply = self.property(limit - bytes.len())?;
            validate(&reply, data_type, format)?;
            self.acknowledge()?;
            if reply.value.is_empty() {
                return Ok(bytes);
            }
            bytes
                .try_reserve_exact(reply.value.len())
                .map_err(|_| "Cannot allocate bounded X11 clipboard text")?;
            bytes.extend_from_slice(&reply.value);
        }
        Err("X11 clipboard exceeded its incremental-transfer chunk limit".into())
    }

    fn owner(&self) -> Result<Window, String> {
        self.connection
            .get_selection_owner(self.clipboard)
            .map_err(error)?
            .reply()
            .map(|reply| reply.owner)
            .map_err(error)
    }
}

fn validate(reply: &GetPropertyReply, data_type: Atom, format: u8) -> Result<(), String> {
    if reply.type_ != data_type
        || reply.format != format
        || reply.value.len() != reply.value_len as usize * usize::from(format / 8)
    {
        return Err("X11 clipboard returned an unexpected property type or encoding".into());
    }
    Ok(())
}

pub(super) fn read(deadline: Instant) -> Result<String, String> {
    let (connection, screen) = RustConnection::connect(None).map_err(error)?;
    let clipboard = atom(&connection, "CLIPBOARD")?;
    let property = atom(&connection, "HAMMERR_CLIPBOARD_READ")?;
    let incr = atom(&connection, "INCR")?;
    let targets = atom(&connection, "TARGETS")?;
    let window = connection.generate_id().map_err(error)?;
    let root = connection
        .setup()
        .roots
        .get(screen)
        .ok_or("X11 clipboard display has no screen")?
        .root;
    connection
        .create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(error)?
        .check()
        .map_err(error)?;
    let mut reader = Reader {
        connection,
        deadline,
        window,
        clipboard,
        property,
        incr,
        events: 0,
    };
    let owner = reader.owner()?;
    if owner == NONE {
        return Err("X11 clipboard is empty".into());
    }
    let offered = reader.convert(targets, AtomEnum::ATOM.into(), 32, MAX_TARGETS * 4)?;
    let offered = offered
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(u32::from_ne_bytes)
        .collect::<Vec<_>>();
    for name in TEXT_TARGETS {
        let target = atom(&reader.connection, name)?;
        if offered.contains(&target) {
            let bytes = reader.convert(target, target, 8, MAX_BYTES)?;
            if reader.owner()? != owner {
                return Err("X11 clipboard owner changed while reading".into());
            }
            if name == "STRING" {
                let length: usize = bytes.iter().map(|byte| char::from(*byte).len_utf8()).sum();
                if length > MAX_BYTES {
                    return Err("X11 clipboard text exceeds 8 MiB after UTF-8 conversion".into());
                }
                return Ok(bytes.into_iter().map(char::from).collect());
            }
            return String::from_utf8(bytes).map_err(|_| "X11 clipboard text is not UTF-8".into());
        }
    }
    Err("X11 clipboard offers no supported text encoding".into())
}
