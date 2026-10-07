// SPDX-License-Identifier: MPL-2.0
//! The X11 `CLIPBOARD` selection through a pure-Rust connection (`x11rb`).
//!
//! A worker thread owns the selection: it holds the copied data, answers
//! `TARGETS`, `TIMESTAMP` and the text and metadata targets for any requestor,
//! streams large data with the ICCCM `INCR` protocol, and on shutdown hands the
//! data to a clipboard manager when one runs. Pastes run on the caller's thread
//! over a second connection, so the worker never waits for them. While this
//! process owns the selection a paste reads the data directly.
use super::{TEXT_PLAIN_UTF8, TRANSFER_TIMEOUT, busy, decode_latin1, decode_text, over_limit};
use bareline_platform::clipboard::MAX_CLIPBOARD_METADATA_BYTES;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE,
    connection::{Connection as _, RequestConnection as _},
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
            Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

/// Larger data goes out in `INCR` chunks of this size (or the server's request
/// limit, if smaller).
const CHUNK: usize = 256 * 1024;
/// Incremental transfers in flight at once; further large requests are refused.
const TRANSFERS: usize = 16;
/// A stalled incremental transfer is dropped after this long.
const TRANSFER_IDLE: Duration = Duration::from_secs(5);
/// How long shutdown waits for a clipboard manager to take the data.
const HANDOVER: Duration = Duration::from_secs(2);
/// Attempts to read text and metadata from one unchanged owner.
const SNAPSHOT_ATTEMPTS: usize = 3;

fn x11_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("X11 clipboard: {error}"))
}
fn too_large() -> io::Error {
    io::Error::new(io::ErrorKind::OutOfMemory, "clipboard data too large")
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    timestamp: Atom,
    multiple: Atom,
    incr: Atom,
    utf8_string: Atom,
    text: Atom,
    text_plain_utf8: Atom,
    text_plain: Atom,
    /// The property conversions are delivered to on our own windows.
    property: Atom,
    /// The property a zero-length append uses to learn the server time.
    stamp: Atom,
    manager: Atom,
    save_targets: Atom,
    /// One per metadata MIME type, in [`METADATA_TYPES`] order.
    metadata: [Atom; 2],
}
const METADATA_TYPES: [&str; 2] = [
    "application/x-bareline-rectangle-v1",
    "application/x-bareline-multiselection-v1",
];
impl Atoms {
    fn intern(conn: &RustConnection) -> io::Result<Self> {
        let names: [&str; 15] = [
            "CLIPBOARD",
            "TARGETS",
            "TIMESTAMP",
            "MULTIPLE",
            "INCR",
            "UTF8_STRING",
            "TEXT",
            TEXT_PLAIN_UTF8,
            "text/plain",
            "BARELINE_SELECTION",
            "BARELINE_TIMESTAMP",
            "CLIPBOARD_MANAGER",
            "SAVE_TARGETS",
            METADATA_TYPES[0],
            METADATA_TYPES[1],
        ];
        // Send every request before waiting for the first reply.
        let cookies = names
            .iter()
            .map(|name| conn.intern_atom(false, name.as_bytes()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(x11_error)?;
        let mut atoms = Vec::with_capacity(names.len());
        for cookie in cookies {
            atoms.push(cookie.reply().map_err(x11_error)?.atom);
        }
        Ok(Self {
            clipboard: atoms[0],
            targets: atoms[1],
            timestamp: atoms[2],
            multiple: atoms[3],
            incr: atoms[4],
            utf8_string: atoms[5],
            text: atoms[6],
            text_plain_utf8: atoms[7],
            text_plain: atoms[8],
            property: atoms[9],
            stamp: atoms[10],
            manager: atoms[11],
            save_targets: atoms[12],
            metadata: [atoms[13], atoms[14]],
        })
    }
    fn metadata(&self, mime: &str) -> Option<Atom> {
        METADATA_TYPES
            .iter()
            .position(|known| *known == mime)
            .map(|index| self.metadata[index])
    }
}

/// One connection with a hidden window of its own.
struct Session {
    conn: RustConnection,
    window: Window,
    atoms: Atoms,
}
impl Session {
    fn open() -> io::Result<Self> {
        let (conn, screen) = RustConnection::connect(None).map_err(x11_error)?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .ok_or_else(|| x11_error("no such screen"))?
            .root;
        let window = conn.generate_id().map_err(x11_error)?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(x11_error)?;
        let atoms = Atoms::intern(&conn)?;
        conn.flush().map_err(x11_error)?;
        Ok(Self { conn, window, atoms })
    }
    fn owner(&self) -> io::Result<Window> {
        Ok(self
            .conn
            .get_selection_owner(self.atoms.clipboard)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?
            .owner)
    }
    /// The next event, waiting until `deadline` (forever with `None`) or until
    /// `wake` becomes readable; `None` then.
    fn next_event(&self, deadline: Option<Instant>, wake: Option<&UnixStream>) -> io::Result<Option<Event>> {
        loop {
            if let Some(event) = self.conn.poll_for_event().map_err(x11_error)? {
                return Ok(Some(event));
            }
            self.conn.flush().map_err(x11_error)?;
            let timeout = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Ok(None);
                    }
                    Some(Timespec {
                        tv_sec: remaining.as_secs() as i64,
                        tv_nsec: i64::from(remaining.subsec_nanos()),
                    })
                }
                None => None,
            };
            let stream = self.conn.stream();
            let mut fds = [PollFd::new(stream, PollFlags::IN), PollFd::new(stream, PollFlags::IN)];
            let count = match wake {
                Some(wake) => {
                    fds[1] = PollFd::new(wake, PollFlags::IN);
                    2
                }
                None => 1,
            };
            match poll(&mut fds[..count], timeout.as_ref()) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(errno) => return Err(errno.into()),
            }
            if count == 2 && !fds[1].revents().is_empty() {
                return Ok(None);
            }
        }
    }
}

/// What this process offers while it owns the selection.
struct Offer {
    text: Arc<[u8]>,
    /// The metadata MIME type's atom and its envelope.
    metadata: Option<(Atom, Arc<[u8]>)>,
}
impl Offer {
    fn targets(&self, atoms: &Atoms) -> Vec<Atom> {
        let mut targets = vec![
            atoms.targets,
            atoms.timestamp,
            atoms.utf8_string,
            atoms.text_plain_utf8,
            atoms.text,
        ];
        if self.text.is_ascii() {
            targets.extend([AtomEnum::STRING.into(), atoms.text_plain]);
        }
        targets.extend(self.metadata.as_ref().map(|(atom, _)| *atom));
        targets
    }
    /// The property type and bytes answering `target`.
    fn data(&self, target: Atom, atoms: &Atoms) -> Option<(Atom, Arc<[u8]>)> {
        if target == atoms.utf8_string || target == atoms.text {
            Some((atoms.utf8_string, self.text.clone()))
        } else if target == atoms.text_plain_utf8 {
            Some((atoms.text_plain_utf8, self.text.clone()))
        } else if (target == u32::from(AtomEnum::STRING) || target == atoms.text_plain) && self.text.is_ascii() {
            // ASCII is valid Latin-1, the only encoding these targets allow.
            Some((target, self.text.clone()))
        } else {
            self.metadata
                .as_ref()
                .filter(|(atom, _)| *atom == target)
                .map(|(atom, envelope)| (*atom, envelope.clone()))
        }
    }
}
type Shared = Arc<Mutex<Option<Arc<Offer>>>>;

enum Command {
    Own(Arc<Offer>, SyncSender<io::Result<()>>),
}
/// An `INCR` transfer to one requestor property.
struct Transfer {
    requestor: Window,
    property: Atom,
    kind: Atom,
    data: Arc<[u8]>,
    offset: usize,
    finished: bool,
    touched: Instant,
}
/// The selection owner, on its worker thread.
struct Owner {
    session: Session,
    current: Option<(Arc<Offer>, u32)>,
    shared: Shared,
    transfers: Vec<Transfer>,
    chunk: usize,
}
impl Owner {
    fn run(mut self, commands: Receiver<Command>, wake: UnixStream) {
        loop {
            let deadline = (!self.transfers.is_empty()).then(|| Instant::now() + Duration::from_secs(1));
            match self.session.next_event(deadline, Some(&wake)) {
                Ok(Some(event)) => self.handle(event),
                Ok(None) => {}
                // The server connection is gone: nothing can be served any more.
                Err(_) => return,
            }
            let now = Instant::now();
            self.transfers
                .retain(|transfer| now.duration_since(transfer.touched) < TRANSFER_IDLE);
            let mut drained = [0; 64];
            while matches!((&wake).read(&mut drained), Ok(count) if count > 0) {}
            loop {
                match commands.try_recv() {
                    Ok(Command::Own(offer, reply)) => {
                        let _ = reply.send(self.acquire(offer));
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.hand_over();
                        return;
                    }
                }
            }
        }
    }
    fn conn(&self) -> &RustConnection {
        &self.session.conn
    }
    fn acquire(&mut self, offer: Arc<Offer>) -> io::Result<()> {
        let (window, atoms) = (self.session.window, &self.session.atoms);
        // ICCCM: own the selection with a real server time, which a zero-length
        // append to a property of our own window reports back.
        self.conn()
            .change_property8(PropMode::APPEND, window, atoms.stamp, AtomEnum::STRING, &[])
            .map_err(x11_error)?;
        let (stamp, deadline) = (atoms.stamp, Instant::now() + TRANSFER_TIMEOUT);
        let time = loop {
            match self.session.next_event(Some(deadline), None)? {
                Some(Event::PropertyNotify(event)) if event.window == window && event.atom == stamp => {
                    break event.time;
                }
                Some(event) => self.handle(event),
                None => return Err(busy()),
            }
        };
        let clipboard = self.session.atoms.clipboard;
        self.conn()
            .set_selection_owner(window, clipboard, time)
            .map_err(x11_error)?;
        if self.session.owner()? != window {
            return Err(busy());
        }
        *lock(&self.shared) = Some(offer.clone());
        self.current = Some((offer, time));
        Ok(())
    }
    fn handle(&mut self, event: Event) {
        match event {
            Event::SelectionRequest(request) => self.respond(request),
            Event::SelectionClear(clear)
                if clear.selection == self.session.atoms.clipboard && clear.owner == self.session.window =>
            {
                self.current = None;
                *lock(&self.shared) = None;
            }
            Event::PropertyNotify(event) if event.state == Property::DELETE => self.progress(event.window, event.atom),
            _ => {}
        }
    }
    fn respond(&mut self, request: SelectionRequestEvent) {
        // Obsolete requestors name no property; ICCCM says to use the target.
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let served = request.selection == self.session.atoms.clipboard
            && match self.current.clone() {
                // A request older than this ownership is for a previous owner.
                Some((offer, time)) if request.time == CURRENT_TIME || request.time >= time => {
                    self.serve(request.requestor, property, request.target, &offer, time)
                }
                _ => false,
            };
        let reply = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if served { property } else { NONE },
        };
        let _ = self
            .conn()
            .send_event(false, request.requestor, EventMask::NO_EVENT, reply);
    }
    fn serve(&mut self, requestor: Window, property: Atom, target: Atom, offer: &Offer, time: u32) -> bool {
        let atoms = &self.session.atoms;
        if target == atoms.targets {
            let targets = offer.targets(atoms);
            return self
                .conn()
                .change_property32(PropMode::REPLACE, requestor, property, AtomEnum::ATOM, &targets)
                .is_ok();
        }
        if target == atoms.timestamp {
            return self
                .conn()
                .change_property32(PropMode::REPLACE, requestor, property, AtomEnum::INTEGER, &[time])
                .is_ok();
        }
        if target == atoms.multiple {
            // Optional in practice and unused by current toolkits; refused.
            return false;
        }
        match offer.data(target, atoms) {
            Some((kind, data)) => self.send(requestor, property, kind, data),
            None => false,
        }
    }
    fn send(&mut self, requestor: Window, property: Atom, kind: Atom, data: Arc<[u8]>) -> bool {
        if data.len() <= self.chunk {
            return self
                .conn()
                .change_property8(PropMode::REPLACE, requestor, property, kind, &data)
                .is_ok();
        }
        self.transfers
            .retain(|transfer| transfer.requestor != requestor || transfer.property != property);
        if self.transfers.len() >= TRANSFERS {
            return false;
        }
        let incr = self.session.atoms.incr;
        // The requestor deleting the INCR property asks for the first chunk.
        let started = self
            .conn()
            .change_window_attributes(
                requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .and_then(|_| {
                self.conn().change_property32(
                    PropMode::REPLACE,
                    requestor,
                    property,
                    incr,
                    &[u32::try_from(data.len()).unwrap_or(u32::MAX)],
                )
            })
            .is_ok();
        if started {
            self.transfers.push(Transfer {
                requestor,
                property,
                kind,
                data,
                offset: 0,
                finished: false,
                touched: Instant::now(),
            });
        }
        started
    }
    /// The requestor consumed the last piece of an incremental transfer.
    fn progress(&mut self, window: Window, property: Atom) {
        let Some(index) = self
            .transfers
            .iter()
            .position(|transfer| transfer.requestor == window && transfer.property == property)
        else {
            return;
        };
        let chunk = self.chunk;
        let transfer = &mut self.transfers[index];
        if transfer.finished {
            self.transfers.swap_remove(index);
            return;
        }
        let end = (transfer.offset + chunk).min(transfer.data.len());
        let piece = transfer.data[transfer.offset..end].to_vec();
        // An empty piece after the data marks the end of the transfer.
        transfer.finished = piece.is_empty();
        transfer.offset = end;
        transfer.touched = Instant::now();
        let (requestor, kind) = (transfer.requestor, transfer.kind);
        if self
            .conn()
            .change_property8(PropMode::REPLACE, requestor, property, kind, &piece)
            .is_err()
        {
            self.transfers.swap_remove(index);
        }
    }
    /// On shutdown, a clipboard manager may keep the copied data available.
    fn hand_over(&mut self) {
        let Some((offer, _)) = self.current.clone() else {
            return;
        };
        let atoms = &self.session.atoms;
        let (window, manager, save_targets, property) =
            (self.session.window, atoms.manager, atoms.save_targets, atoms.property);
        let has_manager = self
            .conn()
            .get_selection_owner(manager)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.owner != NONE);
        if !has_manager || self.session.owner().ok() != Some(window) {
            return;
        }
        let targets = offer.targets(&self.session.atoms);
        let requested = self
            .conn()
            .change_property32(PropMode::REPLACE, window, property, AtomEnum::ATOM, &targets)
            .and_then(|_| {
                self.conn()
                    .convert_selection(window, manager, save_targets, property, CURRENT_TIME)
            })
            .is_ok();
        if !requested {
            return;
        }
        let deadline = Instant::now() + HANDOVER;
        while let Ok(Some(event)) = self.session.next_event(Some(deadline), None) {
            match event {
                Event::SelectionNotify(notify) if notify.selection == manager => return,
                event => self.handle(event),
            }
        }
    }
}

/// A paste's own connection.
struct Reader {
    session: Session,
}
impl Reader {
    /// The bytes of `target` (at most `limit`), or `None` when the owner refuses it.
    fn convert(&self, target: Atom, limit: usize) -> io::Result<Option<(Atom, Vec<u8>)>> {
        let Session { conn, window, atoms } = &self.session;
        let (window, property) = (*window, atoms.property);
        let words = u32::try_from(limit / 4 + 2).unwrap_or(u32::MAX);
        conn.delete_property(window, property).map_err(x11_error)?;
        conn.convert_selection(window, atoms.clipboard, target, property, CURRENT_TIME)
            .map_err(x11_error)?;
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        loop {
            match self.session.next_event(Some(deadline), None)? {
                None => return Err(busy()),
                Some(Event::SelectionNotify(notify))
                    if notify.requestor == window && notify.selection == atoms.clipboard && notify.target == target =>
                {
                    if notify.property == NONE {
                        return Ok(None);
                    }
                    break;
                }
                Some(_) => {}
            }
        }
        let reply = conn
            .get_property(false, window, property, AtomEnum::ANY, 0, words)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?;
        if reply.type_ != atoms.incr {
            conn.delete_property(window, property).map_err(x11_error)?;
            if reply.bytes_after > 0 || reply.value.len() > limit {
                return Err(too_large());
            }
            return Ok(Some((reply.type_, reply.value)));
        }
        // INCR: deleting the announcement asks for the first chunk; each chunk
        // is read and deleted until an empty one ends the transfer.
        if let Some(announced) = reply.value32().and_then(|mut size| size.next())
            && announced as usize > limit
        {
            conn.delete_property(window, property).map_err(x11_error)?;
            return Err(too_large());
        }
        conn.delete_property(window, property).map_err(x11_error)?;
        let mut data = Vec::new();
        let mut kind = NONE;
        let mut deadline = Instant::now() + TRANSFER_TIMEOUT;
        loop {
            match self.session.next_event(Some(deadline), None)? {
                None => return Err(busy()),
                Some(Event::PropertyNotify(event))
                    if event.window == window && event.atom == property && event.state == Property::NEW_VALUE =>
                {
                    let remaining = limit.saturating_sub(data.len());
                    let words = u32::try_from(remaining / 4 + 2).unwrap_or(u32::MAX);
                    let chunk = conn
                        .get_property(true, window, property, AtomEnum::ANY, 0, words)
                        .map_err(x11_error)?
                        .reply()
                        .map_err(x11_error)?;
                    if chunk.bytes_after > 0 || chunk.value.len() > remaining {
                        return Err(too_large());
                    }
                    if chunk.value.is_empty() {
                        return Ok(Some((kind, data)));
                    }
                    kind = chunk.type_;
                    data.extend_from_slice(&chunk.value);
                    deadline = Instant::now() + TRANSFER_TIMEOUT;
                }
                Some(_) => {}
            }
        }
    }
    /// The owner's advertised targets, or `None` when it does not answer `TARGETS`.
    fn targets(&self) -> io::Result<Option<Vec<Atom>>> {
        let Some((_, bytes)) = self.convert(self.session.atoms.targets, 64 * 1024)? else {
            return Ok(None);
        };
        Ok(Some(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|atom| u32::from_ne_bytes(*atom))
                .collect(),
        ))
    }
    fn text(&self, targets: Option<&[Atom]>, max_bytes: usize) -> io::Result<Option<String>> {
        let atoms = &self.session.atoms;
        let string = u32::from(AtomEnum::STRING);
        let preference = [atoms.utf8_string, atoms.text_plain_utf8, string];
        for target in preference {
            if targets.is_some_and(|targets| !targets.contains(&target)) {
                continue;
            }
            match self.convert(target, max_bytes) {
                Ok(Some((_, bytes))) => {
                    let bytes = if target == string { decode_latin1(&bytes) } else { bytes };
                    return decode_text(bytes, max_bytes);
                }
                Ok(None) => {}
                Err(error) if error.kind() == io::ErrorKind::OutOfMemory => {
                    return Err(over_limit("The clipboard text", max_bytes));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }
    fn metadata(&self, targets: Option<&[Atom]>, mime: &str) -> io::Result<Option<Vec<u8>>> {
        let Some(atom) = self.session.atoms.metadata(mime) else {
            return Ok(None);
        };
        if !targets.is_some_and(|targets| targets.contains(&atom)) {
            return Ok(None);
        }
        // The envelope header plus the largest payload; anything larger is not ours.
        match self.convert(atom, MAX_CLIPBOARD_METADATA_BYTES + 64) {
            Ok(found) => Ok(found.map(|(_, bytes)| bytes)),
            Err(error) if error.kind() == io::ErrorKind::OutOfMemory => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub(super) struct X11Clipboard {
    commands: Option<Sender<Command>>,
    wake: UnixStream,
    worker: Option<JoinHandle<()>>,
    owner_window: Window,
    shared: Shared,
    reader: Mutex<Reader>,
}
impl X11Clipboard {
    pub fn connect() -> io::Result<Self> {
        let owner = Session::open()?;
        let reader = Reader {
            session: Session::open()?,
        };
        let owner_window = owner.window;
        let chunk = CHUNK.min(owner.conn.maximum_request_bytes().saturating_sub(1024).max(4096));
        let shared: Shared = Arc::default();
        let (commands, receiver) = mpsc::channel();
        let (wake, wake_worker) = UnixStream::pair()?;
        wake_worker.set_nonblocking(true)?;
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("bareline-clipboard".into())
            .spawn(move || {
                Owner {
                    session: owner,
                    current: None,
                    shared: worker_shared,
                    transfers: Vec::new(),
                    chunk,
                }
                .run(receiver, wake_worker)
            })?;
        Ok(Self {
            commands: Some(commands),
            wake,
            worker: Some(worker),
            owner_window,
            shared,
            reader: Mutex::new(reader),
        })
    }
    pub fn write(&self, text: &str, metadata: Option<(&str, Vec<u8>)>) -> io::Result<()> {
        let reader = lock(&self.reader);
        let metadata = metadata.and_then(|(mime, envelope)| {
            reader
                .session
                .atoms
                .metadata(mime)
                .map(|atom| (atom, Arc::<[u8]>::from(envelope)))
        });
        drop(reader);
        let offer = Arc::new(Offer {
            text: Arc::from(text.as_bytes()),
            metadata,
        });
        let (reply, answer) = mpsc::sync_channel(1);
        self.commands
            .as_ref()
            .ok_or_else(busy)?
            .send(Command::Own(offer, reply))
            .map_err(|_| x11_error("the clipboard owner stopped"))?;
        let _ = (&self.wake).write(&[1]);
        answer
            .recv_timeout(TRANSFER_TIMEOUT + Duration::from_secs(1))
            .map_err(|_| busy())?
    }
    /// What this process offers, while it still owns the selection.
    fn own_offer(&self, owner: Window) -> Option<Arc<Offer>> {
        (owner == self.owner_window)
            .then(|| lock(&self.shared).clone())
            .flatten()
    }
    /// Runs `read` against one owner, retrying when the selection changed hands
    /// during the read, so text and metadata always come from the same copy.
    fn snapshot<T>(
        &self,
        own: impl Fn(&Offer) -> io::Result<Option<T>>,
        read: impl Fn(&Reader) -> io::Result<Option<T>>,
    ) -> io::Result<Option<T>> {
        let reader = lock(&self.reader);
        for _ in 0..SNAPSHOT_ATTEMPTS {
            let owner = reader.session.owner()?;
            if owner == NONE {
                return Ok(None);
            }
            if let Some(offer) = self.own_offer(owner) {
                return own(&offer);
            }
            let value = read(&reader)?;
            if reader.session.owner()? == owner {
                return Ok(value);
            }
        }
        Err(busy())
    }
    pub fn read_text(&self, max_bytes: usize) -> io::Result<Option<String>> {
        self.snapshot(
            |offer| decode_text(offer.text.to_vec(), max_bytes),
            |reader| {
                let targets = reader.targets()?;
                reader.text(targets.as_deref(), max_bytes)
            },
        )
    }
    pub fn read_metadata(&self, mime: &str) -> io::Result<Option<Vec<u8>>> {
        let atom = lock(&self.reader).session.atoms.metadata(mime);
        self.snapshot(
            |offer| {
                Ok(offer
                    .metadata
                    .as_ref()
                    .filter(|(offered, _)| Some(*offered) == atom)
                    .map(|(_, envelope)| envelope.to_vec()))
            },
            |reader| {
                let targets = reader.targets()?;
                reader.metadata(targets.as_deref(), mime)
            },
        )
    }
    pub fn read_with_metadata(&self, text_limit: usize, mime: &str) -> io::Result<Option<(String, Option<Vec<u8>>)>> {
        let atom = lock(&self.reader).session.atoms.metadata(mime);
        self.snapshot(
            |offer| {
                Ok(decode_text(offer.text.to_vec(), text_limit)?.map(|text| {
                    let metadata = offer
                        .metadata
                        .as_ref()
                        .filter(|(offered, _)| Some(*offered) == atom)
                        .map(|(_, envelope)| envelope.to_vec());
                    (text, metadata)
                }))
            },
            |reader| {
                let targets = reader.targets()?;
                let Some(text) = reader.text(targets.as_deref(), text_limit)? else {
                    return Ok(None);
                };
                Ok(Some((text, reader.metadata(targets.as_deref(), mime)?)))
            },
        )
    }
}
impl Drop for X11Clipboard {
    fn drop(&mut self) {
        // Disconnecting the command channel asks the owner to hand over and stop.
        self.commands.take();
        let _ = (&self.wake).write(&[1]);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
