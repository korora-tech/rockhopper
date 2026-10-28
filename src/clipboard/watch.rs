//! Watching and setting the clipboard through wlr data-control, on a thread
//! with its own Wayland connection.
//! <https://wayland.app/protocols/wlr-data-control-unstable-v1>

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wayland_client::backend::{ObjectId, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::ZwlrDataControlDeviceV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_offer_v1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1::ZwlrDataControlSourceV1;

use super::store::Captured;

/// Password managers mark secrets with this; such copies are never kept.
const SECRET_MIMES: &[&str] = &[
    "x-kde-passwordManagerHint",
    "application/x-nspasteboard-concealed-type",
];
const TEXT_MIMES: &[&str] = &[
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "STRING",
    "TEXT",
];
const PNG: &str = "image/png";
const MAX_TEXT: usize = 1 << 20;
const MAX_IMAGE: usize = 32 << 20;
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a paste target may take to read what we offered.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Sends commands to the watcher thread.
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Command>,
    wake: Arc<UnixStream>,
}

enum Command {
    Set {
        mimes: Vec<String>,
        data: Arc<Vec<u8>>,
    },
}

impl Handle {
    pub fn set_text(&self, text: &str) {
        let mimes = TEXT_MIMES.iter().map(|m| m.to_string()).collect();
        self.send(Command::Set {
            mimes,
            data: Arc::new(text.as_bytes().to_vec()),
        });
    }

    pub fn set_png(&self, png: Arc<Vec<u8>>) {
        self.send(Command::Set {
            mimes: vec![PNG.into()],
            data: png,
        });
    }

    fn send(&self, command: Command) {
        if self.tx.send(command).is_ok() {
            _ = (&*self.wake).write_all(&[0]);
        }
    }
}

/// Start watching. Each copy is passed to `on_copy` on a reader thread.
/// `None` when the compositor lacks wlr data-control.
pub fn start(images: bool, on_copy: impl Fn(Captured) + Send + Sync + 'static) -> Option<Handle> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
    let qh = queue.handle();
    let manager: ZwlrDataControlManagerV1 = match globals.bind(&qh, 1..=2, ()) {
        Ok(m) => m,
        Err(_) => {
            eprintln!(
                "rockhopper: the compositor has no wlr data-control; clipboard history is off"
            );
            return None;
        }
    };
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ()).ok()?;
    let device = manager.get_data_device(&seat, &qh, ());
    let (tx, rx) = mpsc::channel();
    let (wake_rx, wake_tx) = UnixStream::pair().ok()?;
    wake_rx.set_nonblocking(true).ok()?;
    // One reader, so copies are recorded in the order they were made.
    let (reads, pending) = mpsc::channel::<(UnixStream, &'static str)>();
    std::thread::Builder::new()
        .name("rockhopper-clipboard-read".into())
        .spawn(move || {
            for (mut reader, mime) in pending {
                if let Some(captured) = read_captured(&mut reader, mime) {
                    on_copy(captured);
                }
            }
        })
        .ok()?;
    let mut state = State {
        offers: HashMap::new(),
        source: None,
        images,
        reads,
        conn: conn.clone(),
    };

    std::thread::Builder::new()
        .name("rockhopper-clipboard".into())
        .spawn(move || {
            loop {
                if queue.dispatch_pending(&mut state).is_err() {
                    break;
                }
                // A full socket is not fatal; what is left is sent next time.
                if let Err(err) = queue.flush()
                    && !matches!(&err, WaylandError::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock)
                {
                    break;
                }
                let Some(guard) = queue.prepare_read() else {
                    continue;
                };
                let mut fds = [
                    libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 },
                    libc::pollfd { fd: wake_rx.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                ];
                // Wait until the Wayland connection or the wake socket is ready.
                if crate::syscalls::poll(&mut fds, -1).is_err() {
                    drop(guard);
                    continue;
                }
                if fds[0].revents != 0 {
                    if guard.read().is_err() {
                        break;
                    }
                } else {
                    drop(guard);
                }
                if fds[1].revents != 0 {
                    let mut drain = [0u8; 64];
                    while (&wake_rx).read(&mut drain).is_ok_and(|n| n > 0) {}
                    for command in rx.try_iter() {
                        match command {
                            Command::Set { mimes, data } => {
                                let source = manager.create_data_source(&qh, ());
                                for mime in &mimes {
                                    source.offer(mime.clone());
                                }
                                device.set_selection(Some(&source));
                                if let Some((old, _)) = state.source.replace((source, data)) {
                                    old.destroy();
                                }
                            }
                        }
                    }
                }
            }
            eprintln!("rockhopper: lost the Wayland connection; clipboard history stopped");
        })
        .ok()?;

    Some(Handle {
        tx,
        wake: Arc::new(wake_tx),
    })
}

struct State {
    /// MIME types announced for each live offer.
    offers: HashMap<ObjectId, Vec<String>>,
    /// The selection we set, with its bytes.
    source: Option<(ZwlrDataControlSourceV1, Arc<Vec<u8>>)>,
    images: bool,
    /// Hands each selection to the reader thread.
    reads: mpsc::Sender<(UnixStream, &'static str)>,
    conn: Connection,
}

/// Which type to read from an offer, if any. Text wins over images, since
/// apps that offer both (office suites) mean the text.
fn choose(mimes: &[String], images: bool) -> Option<&'static str> {
    if mimes.iter().any(|m| SECRET_MIMES.contains(&m.as_str())) {
        return None;
    }
    TEXT_MIMES
        .iter()
        .find(|t| mimes.iter().any(|m| m == *t))
        .copied()
        .or_else(|| (images && mimes.iter().any(|m| m == PNG)).then_some(PNG))
}

impl State {
    fn on_selection(&mut self, offer: ZwlrDataControlOfferV1) {
        let mimes = self.offers.remove(&offer.id()).unwrap_or_default();
        let Some(mime) = choose(&mimes, self.images) else {
            offer.destroy();
            return;
        };
        let Ok((reader, writer)) = UnixStream::pair() else {
            offer.destroy();
            return;
        };
        offer.receive(mime.into(), writer.as_fd());
        _ = self.conn.flush();
        drop(writer);
        offer.destroy();
        // Read off this thread: the source may be our own selection, which
        // this thread has to serve.
        _ = self.reads.send((reader, mime));
    }
}

fn read_captured(reader: &mut UnixStream, mime: &str) -> Option<Captured> {
    let limit = if mime == PNG { MAX_IMAGE } else { MAX_TEXT };
    let bytes = read_limited(reader, limit)?;
    if mime == PNG {
        Some(Captured::Png(bytes))
    } else {
        String::from_utf8(bytes).ok().map(Captured::Text)
    }
}

/// Everything until EOF, or `None` past `limit` bytes or the timeout.
fn read_limited(reader: &mut UnixStream, limit: usize) -> Option<Vec<u8>> {
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        reader.set_read_timeout(Some(left)).ok()?;
        match reader.read(&mut buf) {
            Ok(0) => return Some(out),
            Ok(n) if out.len() + n > limit => return None,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrDataControlManagerV1,
        _: <ZwlrDataControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_device_v1::Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
            }
            zwlr_data_control_device_v1::Event::Selection { id: Some(offer) } => {
                state.on_selection(offer)
            }
            zwlr_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => {
                state.offers.remove(&offer.id());
                offer.destroy();
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event
            && let Some(mimes) = state.offers.get_mut(&offer.id())
        {
            mimes.push(mime_type);
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        source: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_source_v1::Event::Send { fd, .. } => {
                if let Some((_, data)) = state.source.as_ref().filter(|(s, _)| s == source) {
                    let data = data.clone();
                    std::thread::spawn(move || write_all(fd, &data));
                }
            }
            zwlr_data_control_source_v1::Event::Cancelled => {
                if state.source.as_ref().is_some_and(|(s, _)| s == source) {
                    state.source = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

/// Write `data` to a paste target, giving up after [`WRITE_TIMEOUT`] so a
/// target that never reads cannot hold the thread and the data forever.
fn write_all(fd: OwnedFd, data: &[u8]) {
    let raw = fd.as_raw_fd();
    if crate::syscalls::set_nonblocking(fd.as_fd()).is_err() {
        return;
    }
    let mut file = std::fs::File::from(fd);
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut written = 0;
    while written < data.len() {
        match file.write(&data[written..]) {
            Ok(0) => return,
            Ok(n) => written += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    return;
                };
                let mut pfd = libc::pollfd {
                    fd: raw,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                if !crate::syscalls::poll(
                    std::slice::from_mut(&mut pfd),
                    left.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32,
                )
                .is_ok_and(|ready| ready > 0)
                {
                    return;
                }
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn choosing_a_type() {
        assert_eq!(
            choose(&m(&["text/plain", "UTF8_STRING"]), true),
            Some("UTF8_STRING")
        );
        assert_eq!(choose(&m(&["image/png", "text/html"]), true), Some(PNG));
        assert_eq!(choose(&m(&["image/png", "text/html"]), false), None);
        assert_eq!(
            choose(&m(&["image/png", "text/plain;charset=utf-8"]), true),
            Some("text/plain;charset=utf-8")
        );
        assert_eq!(
            choose(&m(&["text/plain", "x-kde-passwordManagerHint"]), true),
            None
        );
        assert_eq!(choose(&m(&["application/x-thing"]), true), None);
    }

    #[test]
    fn writes_give_up_on_a_stalled_reader() {
        let (reader, writer) = UnixStream::pair().unwrap();
        let started = Instant::now();
        // Far more than the socket buffer holds, and nobody reads.
        write_all(OwnedFd::from(writer), &vec![0u8; 8 << 20]);
        assert!(started.elapsed() >= WRITE_TIMEOUT);
        assert!(started.elapsed() < WRITE_TIMEOUT + Duration::from_secs(2));
        drop(reader);
    }

    #[test]
    fn reads_until_eof_within_limit() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        b.write_all(b"hello").unwrap();
        drop(b);
        assert_eq!(read_limited(&mut a, 10).unwrap(), b"hello");
        let (mut a, mut b) = UnixStream::pair().unwrap();
        b.write_all(b"too long").unwrap();
        drop(b);
        assert!(read_limited(&mut a, 3).is_none());
    }
}
