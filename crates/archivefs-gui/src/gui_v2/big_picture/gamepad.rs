//! Reads game controllers from Linux evdev (`/dev/input/event*`).
//!
//! evdev is used because it needs no new dependency, works the same on X11
//! and Wayland (egui and winit deliver no controller input at all), and the
//! kernel's gamepad classification keeps out the mice and tablets that also
//! appear as `/dev/input/js*` on real machines.
//!
//! Privacy and safety: only devices with gamepad buttons stay open. Every
//! other descriptor, keyboards above all, is closed straight after the
//! capability check without reading one event. Devices are opened read-only
//! and never grabbed, so emulators see the controller as usual.

use super::nav::{Button, Control, Direction, PadEvent};

const STICK_ON: f32 = 0.55;
const STICK_OFF: f32 = 0.35;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::gui_v2) enum PadStatus {
    /// Nothing has been checked yet, or this platform has no reader.
    #[default]
    Unsupported,
    /// No controller could be read. `unreadable` devices the kernel
    /// describes as controllers could not be opened for lack of permission;
    /// unreadable keyboards and mice are not counted.
    NotFound {
        unreadable: usize,
    },
    Connected(Vec<String>),
}

impl PadStatus {
    pub(in crate::gui_v2) fn connected(&self) -> bool {
        matches!(self, Self::Connected(_))
    }

    pub(in crate::gui_v2) fn describe(&self) -> String {
        match self {
            Self::Unsupported => "Controllers are not supported on this system".into(),
            Self::NotFound { unreadable: 0 } => "No controller found".into(),
            Self::NotFound { unreadable: 1 } => {
                "A controller is connected but EmuWiz has no permission to read it".into()
            }
            Self::NotFound { unreadable } => {
                format!(
                    "{unreadable} controllers are connected but EmuWiz has no permission to read them"
                )
            }
            Self::Connected(names) => match names.as_slice() {
                [name] => format!("Controller: {name}"),
                names => format!("{} controllers connected", names.len()),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct AxisRange {
    min: i32,
    max: i32,
}

impl AxisRange {
    /// -1.0 to 1.0, whatever range this controller reports.
    fn normal(self, value: i32) -> f32 {
        if self.max <= self.min {
            return 0.0;
        }
        let span = (i64::from(self.max) - i64::from(self.min)) as f32;
        (2.0 * (i64::from(value) - i64::from(self.min)) as f32 / span - 1.0).clamp(-1.0, 1.0)
    }
}

// linux/input-event-codes.h
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const SYN_DROPPED: u16 = 3;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;
const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_START: u16 = 0x13b;
const BTN_TOUCH: u16 = 0x14a;
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_DOWN: u16 = 0x221;
const BTN_DPAD_LEFT: u16 = 0x222;
const BTN_DPAD_RIGHT: u16 = 0x223;

fn bit(bits: &[u8], code: u16) -> bool {
    bits.get(usize::from(code) / 8)
        .is_some_and(|byte| byte & (1 << (code % 8)) != 0)
}

/// The kernel's gamepad class: a south button (`BTN_GAMEPAD`) and some way to
/// steer. Touchpads built into a pad expose `BTN_TOUCH` on a separate node.
fn is_gamepad(keys: &[u8], axes: &[u8]) -> bool {
    let steers = (bit(axes, ABS_X) && bit(axes, ABS_Y))
        || (bit(axes, ABS_HAT0X) && bit(axes, ABS_HAT0Y))
        || bit(keys, BTN_DPAD_UP);
    bit(keys, BTN_SOUTH) && !bit(keys, BTN_TOUCH) && steers
}

/// Parses a sysfs `capabilities/*` bitmap: hexadecimal kernel longs, most
/// significant first, into the byte layout `bit` reads. These files are
/// readable by everyone, unlike the device nodes themselves.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_capabilities(text: &str, bytes: usize) -> Option<Vec<u8>> {
    const WORD_BYTES: usize = (usize::BITS / 8) as usize;
    let mut out = vec![0_u8; bytes];
    for (index, word) in text.split_whitespace().rev().enumerate() {
        let value = usize::from_str_radix(word, 16).ok()?;
        for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
            if let Some(slot) = out.get_mut(index * WORD_BYTES + offset) {
                *slot = byte;
            }
        }
    }
    Some(out)
}

fn button(code: u16) -> Option<Control> {
    Some(match code {
        BTN_SOUTH => Control::Button(Button::South),
        BTN_EAST => Control::Button(Button::East),
        BTN_WEST => Control::Button(Button::West),
        BTN_NORTH => Control::Button(Button::North),
        BTN_TL => Control::Button(Button::LeftBumper),
        BTN_TR => Control::Button(Button::RightBumper),
        BTN_START => Control::Button(Button::Start),
        _ => return None,
    })
}

/// One direction of an analogue axis or hat, as an on/off control.
#[derive(Default)]
struct Pole {
    on: bool,
}

impl Pole {
    fn update(&mut self, deflection: f32) -> Option<bool> {
        let threshold = if self.on { STICK_OFF } else { STICK_ON };
        let on = deflection >= threshold;
        (on != self.on).then(|| {
            self.on = on;
            on
        })
    }
}

/// Decodes one controller's raw events. Analogue movement inside the dead
/// zone produces nothing, so a stick at rest costs no repaints.
#[derive(Default)]
struct Decoder {
    device: u32,
    x: AxisRange,
    y: AxisRange,
    stick: [Pole; 4],
    hat: [Pole; 4],
    /// D-pads reported as buttons rather than a hat.
    dpad: [bool; 4],
}

const AXIS_DIRECTIONS: [Direction; 4] = [
    Direction::Left,
    Direction::Right,
    Direction::Up,
    Direction::Down,
];

fn dpad_index(code: u16) -> Option<usize> {
    Some(match code {
        BTN_DPAD_LEFT => 0,
        BTN_DPAD_RIGHT => 1,
        BTN_DPAD_UP => 2,
        BTN_DPAD_DOWN => 3,
        _ => return None,
    })
}

impl Decoder {
    /// The stick, hat and d-pad buttons can all hold one direction. It is
    /// pressed when the first of them takes it and released when the last
    /// lets go, so letting go of one never cancels another still held.
    fn held(&self, index: usize) -> bool {
        self.stick[index].on || self.hat[index].on || self.dpad[index]
    }

    fn direction(&mut self, index: usize, was: bool, out: &mut Vec<PadEvent>) {
        let now = self.held(index);
        if now != was {
            let (device, control) = (self.device, Control::Direction(AXIS_DIRECTIONS[index]));
            out.push(if now {
                PadEvent::Pressed { device, control }
            } else {
                PadEvent::Released { device, control }
            });
        }
    }

    fn axis(&mut self, poles: (usize, usize), hat: bool, value: f32, out: &mut Vec<PadEvent>) {
        for (index, deflection) in [(poles.0, -value), (poles.1, value)] {
            let was = self.held(index);
            let bank = if hat { &mut self.hat } else { &mut self.stick };
            bank[index].update(deflection);
            self.direction(index, was, out);
        }
    }

    fn event(&mut self, kind: u16, code: u16, value: i32, out: &mut Vec<PadEvent>) {
        match (kind, code) {
            (EV_SYN, SYN_DROPPED) => {
                self.stick = Default::default();
                self.hat = Default::default();
                self.dpad = Default::default();
                out.push(PadEvent::Reset {
                    device: self.device,
                });
            }
            (EV_KEY, code) if value == 0 || value == 1 => {
                if let Some(index) = dpad_index(code) {
                    let was = self.held(index);
                    self.dpad[index] = value == 1;
                    self.direction(index, was, out);
                } else if let Some(control) = button(code) {
                    let device = self.device;
                    out.push(if value == 1 {
                        PadEvent::Pressed { device, control }
                    } else {
                        PadEvent::Released { device, control }
                    });
                }
            }
            (EV_ABS, ABS_X) => self.axis((0, 1), false, self.x.normal(value), out),
            (EV_ABS, ABS_Y) => self.axis((2, 3), false, self.y.normal(value), out),
            (EV_ABS, ABS_HAT0X) => self.axis((0, 1), true, value.signum() as f32, out),
            (EV_ABS, ABS_HAT0Y) => self.axis((2, 3), true, value.signum() as f32, out),
            _ => {}
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use eframe::egui;
    use std::{
        collections::{BTreeMap, HashMap},
        ffi::CString,
        fs,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        os::unix::{ffi::OsStrExt, fs::MetadataExt},
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    const POLL_MS: i32 = 200;
    const RESCAN: Duration = Duration::from_secs(2);
    const EV_MAX_BYTES: usize = 4;
    const KEY_BYTES: usize = 0x300 / 8;
    const ABS_BYTES: usize = 0x40 / 8;

    /// `_IOC(_IOC_READ, 'E', nr, size)`.
    const fn eviocread(nr: u32, size: usize) -> u32 {
        (2 << 30) | ((size as u32) << 16) | ((b'E' as u32) << 8) | nr
    }

    fn event_bits(fd: i32, event_type: u16, out: &mut [u8]) -> bool {
        // SAFETY: EVIOCGBIT writes at most out.len() bytes into out.
        unsafe {
            libc::ioctl(
                fd,
                eviocread(0x20 + u32::from(event_type), out.len()) as _,
                out.as_mut_ptr(),
            ) >= 0
        }
    }

    fn axis_range(fd: i32, axis: u16) -> AxisRange {
        // SAFETY: input_absinfo is plain integers and EVIOCGABS fills exactly it.
        let mut info: libc::input_absinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::input_absinfo>();
        let request = eviocread(0x40 + u32::from(axis), size);
        // SAFETY: the kernel writes one input_absinfo into info.
        if unsafe { libc::ioctl(fd, request as _, &mut info) } < 0 {
            return AxisRange::default();
        }
        AxisRange {
            min: info.minimum,
            max: info.maximum,
        }
    }

    fn device_name(fd: i32) -> String {
        let mut name = [0_u8; 128];
        let request = eviocread(0x06, name.len());
        // SAFETY: EVIOCGNAME writes at most name.len() bytes into name.
        let length = unsafe { libc::ioctl(fd, request as _, name.as_mut_ptr()) };
        if length <= 0 {
            return "Controller".into();
        }
        let end = name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(name.len());
        String::from_utf8_lossy(&name[..end]).trim().to_string()
    }

    enum Probe {
        Pad(Open),
        NotAPad,
        Denied,
        Gone,
    }

    struct Open {
        fd: OwnedFd,
        name: String,
        decoder: Decoder,
    }

    fn probe(path: &Path, device: u32) -> Probe {
        let Ok(raw_path) = CString::new(path.as_os_str().as_bytes()) else {
            return Probe::NotAPad;
        };
        // SAFETY: raw_path is a valid NUL-terminated string.
        let raw = unsafe {
            libc::open(
                raw_path.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if raw < 0 {
            return match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EACCES | libc::EPERM) => Probe::Denied,
                Some(libc::ENOENT | libc::ENODEV | libc::ENXIO) => Probe::Gone,
                _ => Probe::NotAPad,
            };
        }
        // SAFETY: open returned this descriptor and nothing else owns it.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut types = [0_u8; EV_MAX_BYTES];
        let mut keys = [0_u8; KEY_BYTES];
        let mut axes = [0_u8; ABS_BYTES];
        if !event_bits(raw, 0, &mut types)
            || !bit(&types, EV_KEY)
            || !event_bits(raw, EV_KEY, &mut keys)
        {
            return Probe::NotAPad;
        }
        if bit(&types, EV_ABS) {
            event_bits(raw, EV_ABS, &mut axes);
        }
        if !is_gamepad(&keys, &axes) {
            return Probe::NotAPad;
        }
        Probe::Pad(Open {
            name: device_name(raw),
            decoder: Decoder {
                device,
                x: axis_range(raw, ABS_X),
                y: axis_range(raw, ABS_Y),
                ..Default::default()
            },
            fd,
        })
    }

    /// Which device node a path names. A path such as `event5` is reused by
    /// the kernel for the next device once its device leaves; devtmpfs gives
    /// the new node a new inode, so an identity tells the two apart.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct NodeId {
        inode: u64,
        changed: (i64, i64),
    }

    /// Where devices come from: the kernel, or a fake in tests.
    trait Devices {
        fn nodes(&self) -> BTreeMap<PathBuf, NodeId>;
        fn probe(&self, path: &Path, device: u32) -> Probe;
        /// For a node EmuWiz may not open: whether the kernel's published
        /// capabilities say it is a controller. None when that is unknown.
        fn denied_is_gamepad(&self, path: &Path) -> Option<bool>;
    }

    struct Kernel {
        root: PathBuf,
    }

    impl Devices for Kernel {
        fn nodes(&self) -> BTreeMap<PathBuf, NodeId> {
            event_nodes(&self.root)
                .into_iter()
                .filter_map(|path| {
                    let meta = fs::symlink_metadata(&path).ok()?;
                    let id = NodeId {
                        inode: meta.ino(),
                        changed: (meta.ctime(), meta.ctime_nsec()),
                    };
                    Some((path, id))
                })
                .collect()
        }

        fn probe(&self, path: &Path, device: u32) -> Probe {
            probe(path, device)
        }

        fn denied_is_gamepad(&self, path: &Path) -> Option<bool> {
            let name = path.file_name()?;
            let caps = Path::new("/sys/class/input")
                .join(name)
                .join("device/capabilities");
            let keys = parse_capabilities(&fs::read_to_string(caps.join("key")).ok()?, KEY_BYTES)?;
            let axes = fs::read_to_string(caps.join("abs"))
                .ok()
                .and_then(|text| parse_capabilities(&text, ABS_BYTES))
                .unwrap_or_else(|| vec![0; ABS_BYTES]);
            Some(is_gamepad(&keys, &axes))
        }
    }

    fn event_nodes(root: &Path) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(root) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix("event"))
                    .is_some_and(|number| {
                        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
                    })
            })
            .map(|entry| entry.path())
            .collect()
    }

    #[derive(Default)]
    struct Scanner {
        pads: HashMap<PathBuf, Open>,
        /// Non-controllers already looked at: not reopened on every rescan.
        ignored: BTreeMap<PathBuf, NodeId>,
        next_device: u32,
    }

    impl Scanner {
        fn scan(&mut self, devices: &impl Devices, events: &mut Vec<PadEvent>) -> PadStatus {
            let nodes = devices.nodes();
            // A node that left, or whose name now belongs to another device,
            // is looked at again.
            self.ignored.retain(|path, id| nodes.get(path) == Some(id));
            self.pads.retain(|path, pad| {
                let keep = nodes.contains_key(path);
                if !keep {
                    events.push(PadEvent::Reset {
                        device: pad.decoder.device,
                    });
                }
                keep
            });
            let mut unreadable = 0;
            for (path, id) in nodes {
                if self.pads.contains_key(&path) || self.ignored.contains_key(&path) {
                    continue;
                }
                self.next_device += 1;
                match devices.probe(&path, self.next_device) {
                    Probe::Pad(pad) => {
                        log::info!("Big Picture: controller found: {}", pad.name);
                        self.pads.insert(path, pad);
                    }
                    Probe::NotAPad => {
                        self.ignored.insert(path, id);
                    }
                    // Keyboards and mice are normally unreadable on a desktop
                    // and say nothing about controllers. Only a node the
                    // kernel describes as a controller is a permission
                    // problem; it is tried again, as its access may follow.
                    Probe::Denied => match devices.denied_is_gamepad(&path) {
                        Some(true) => unreadable += 1,
                        Some(false) => {
                            self.ignored.insert(path, id);
                        }
                        None => {}
                    },
                    // Gone between listing and opening: look again next time.
                    Probe::Gone => {}
                }
            }
            if self.pads.is_empty() {
                PadStatus::NotFound { unreadable }
            } else {
                let mut names: Vec<_> = self.pads.values().map(|pad| pad.name.clone()).collect();
                names.sort();
                PadStatus::Connected(names)
            }
        }

        /// Waits up to the poll interval, then decodes what arrived. A device
        /// that fails has been unplugged and is dropped.
        fn read(&mut self, events: &mut Vec<PadEvent>) {
            let paths: Vec<PathBuf> = self.pads.keys().cloned().collect();
            let mut polls: Vec<libc::pollfd> = paths
                .iter()
                .map(|path| libc::pollfd {
                    fd: self.pads[path].fd.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            // SAFETY: polls is a live array of polls.len() pollfd entries.
            let ready =
                unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, POLL_MS) };
            if ready <= 0 {
                return;
            }
            for (path, poll) in paths.iter().zip(polls) {
                if poll.revents == 0 {
                    continue;
                }
                let Some(pad) = self.pads.get_mut(path) else {
                    continue;
                };
                if !drain(pad, events) {
                    events.push(PadEvent::Reset {
                        device: pad.decoder.device,
                    });
                    self.pads.remove(path);
                }
            }
        }
    }

    /// False once the device has gone.
    fn drain(pad: &mut Open, events: &mut Vec<PadEvent>) -> bool {
        const EVENT: usize = std::mem::size_of::<libc::input_event>();
        loop {
            // SAFETY: input_event is plain integers, so zero is a valid value.
            let mut batch: [libc::input_event; 32] = unsafe { std::mem::zeroed() };
            // SAFETY: read writes at most size_of_val(&batch) bytes into batch.
            let count = unsafe {
                libc::read(
                    pad.fd.as_raw_fd(),
                    batch.as_mut_ptr().cast(),
                    std::mem::size_of_val(&batch),
                )
            };
            if count < 0 {
                return std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock;
            }
            if count == 0 {
                return false;
            }
            for event in &batch[..count as usize / EVENT] {
                pad.decoder
                    .event(event.type_, event.code, event.value, events);
            }
        }
    }

    /// The background reader, alive only while Big Picture is open. Dropping
    /// it stops the thread and closes every device within one poll interval.
    pub(in crate::gui_v2) struct Reader {
        events: mpsc::Receiver<PadEvent>,
        status: Arc<Mutex<PadStatus>>,
        stop: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
    }

    impl Reader {
        pub(in crate::gui_v2) fn start(context: egui::Context) -> Self {
            Self::start_at(PathBuf::from("/dev/input"), context)
        }

        fn start_at(root: PathBuf, context: egui::Context) -> Self {
            let devices = Kernel { root };
            let mut scanner = Scanner::default();
            // The first scan runs here, so the mode opens knowing the answer.
            let status = Arc::new(Mutex::new(scanner.scan(&devices, &mut Vec::new())));
            let stop = Arc::new(AtomicBool::new(false));
            let paused = Arc::new(AtomicBool::new(false));
            let (sender, events) = mpsc::channel();
            let worker = Worker {
                devices,
                scanner,
                status: status.clone(),
                stop: stop.clone(),
                paused: paused.clone(),
                sender,
                context,
            };
            let started = std::thread::Builder::new()
                .name("emuwiz-gamepad".into())
                .spawn(move || worker.run());
            if let Err(error) = started {
                log::warn!("Big Picture: controller reader did not start: {error}");
                *status.lock().unwrap_or_else(|e| e.into_inner()) = PadStatus::Unsupported;
            }
            Self {
                events,
                status,
                stop,
                paused,
            }
        }

        pub(in crate::gui_v2) fn status(&self) -> PadStatus {
            self.status
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }

        /// While paused the reader delivers nothing and asks for no repaints,
        /// so an emulator's input cannot reach EmuWiz or keep it drawing.
        pub(in crate::gui_v2) fn set_paused(&self, paused: bool) {
            self.paused.store(paused, Ordering::Relaxed);
        }

        pub(in crate::gui_v2) fn drain(&self) -> impl Iterator<Item = PadEvent> + '_ {
            self.events.try_iter()
        }
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    struct Worker {
        devices: Kernel,
        scanner: Scanner,
        status: Arc<Mutex<PadStatus>>,
        stop: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        sender: mpsc::Sender<PadEvent>,
        context: egui::Context,
    }

    impl Worker {
        fn run(mut self) {
            let mut scanned = Instant::now();
            let mut was_paused = false;
            let mut events = Vec::new();
            while !self.stop.load(Ordering::Relaxed) {
                if self.scanner.pads.is_empty() {
                    std::thread::sleep(Duration::from_millis(POLL_MS as u64));
                } else {
                    self.scanner.read(&mut events);
                }
                if scanned.elapsed() >= RESCAN {
                    scanned = Instant::now();
                    let status = self.scanner.scan(&self.devices, &mut events);
                    let mut current = self.status.lock().unwrap_or_else(|e| e.into_inner());
                    if *current != status {
                        *current = status;
                        self.context.request_repaint();
                    }
                }
                let paused = self.paused.load(Ordering::Relaxed);
                if paused {
                    events.clear();
                } else if was_paused {
                    // What was pressed in the meantime is unknown; held
                    // controls must be released and pressed again to count.
                    events.extend(self.scanner.pads.values().map(|pad| PadEvent::Reset {
                        device: pad.decoder.device,
                    }));
                }
                was_paused = paused;
                if events.is_empty() {
                    continue;
                }
                for event in events.drain(..) {
                    if self.sender.send(event).is_err() {
                        return;
                    }
                }
                self.context.request_repaint();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// `/dev/uinput` ioctls, from linux/uinput.h.
        const UI_SET_EVBIT: u32 = 0x4004_5564;
        const UI_SET_KEYBIT: u32 = 0x4004_5565;
        const UI_SET_ABSBIT: u32 = 0x4004_5567;
        const UI_DEV_SETUP: u32 = 0x405c_5503;
        const UI_ABS_SETUP: u32 = 0x401c_5504;
        const UI_DEV_CREATE: u32 = 0x5501;

        #[repr(C)]
        struct Setup {
            bustype: u16,
            vendor: u16,
            product: u16,
            version: u16,
            name: [u8; 80],
            ff_effects_max: u32,
        }

        #[repr(C)]
        struct AbsSetup {
            code: u16,
            info: libc::input_absinfo,
        }

        /// A real kernel input device. Closing it unplugs it.
        struct VirtualPad(OwnedFd);

        impl VirtualPad {
            fn create(name: &str) -> Self {
                let path = CString::new("/dev/uinput").unwrap();
                // SAFETY: path is NUL-terminated.
                let raw = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
                assert!(raw >= 0, "this test needs write access to /dev/uinput");
                // SAFETY: open returned this descriptor.
                let fd = unsafe { OwnedFd::from_raw_fd(raw) };
                let mut setup = Setup {
                    bustype: 0x03,
                    vendor: 0x045e,
                    product: 0x028e,
                    version: 1,
                    name: [0; 80],
                    ff_effects_max: 0,
                };
                setup.name[..name.len()].copy_from_slice(name.as_bytes());
                // SAFETY: each ioctl takes the integer or struct given, per uinput.h.
                unsafe {
                    assert!(libc::ioctl(raw, UI_SET_EVBIT as _, i32::from(EV_KEY)) >= 0);
                    assert!(libc::ioctl(raw, UI_SET_EVBIT as _, i32::from(EV_ABS)) >= 0);
                    for key in [BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_START] {
                        assert!(libc::ioctl(raw, UI_SET_KEYBIT as _, i32::from(key)) >= 0);
                    }
                    for code in [ABS_X, ABS_Y, ABS_HAT0X, ABS_HAT0Y] {
                        assert!(libc::ioctl(raw, UI_SET_ABSBIT as _, i32::from(code)) >= 0);
                        let mut info: libc::input_absinfo = std::mem::zeroed();
                        (info.minimum, info.maximum) = if code < ABS_HAT0X {
                            (-32768, 32767)
                        } else {
                            (-1, 1)
                        };
                        let abs = AbsSetup { code, info };
                        assert!(libc::ioctl(raw, UI_ABS_SETUP as _, &abs) >= 0);
                    }
                    assert!(libc::ioctl(raw, UI_DEV_SETUP as _, &setup) >= 0);
                    assert!(libc::ioctl(raw, UI_DEV_CREATE as _) >= 0);
                }
                Self(fd)
            }

            fn send(&self, kind: u16, code: u16, value: i32) {
                for (kind, code, value) in [(kind, code, value), (EV_SYN, 0, 0)] {
                    // SAFETY: zero is a valid input_event.
                    let mut event: libc::input_event = unsafe { std::mem::zeroed() };
                    event.type_ = kind;
                    event.code = code;
                    event.value = value;
                    let size = std::mem::size_of_val(&event);
                    // SAFETY: event is live for size bytes.
                    let written =
                        unsafe { libc::write(self.0.as_raw_fd(), (&raw const event).cast(), size) };
                    assert_eq!(written, size as isize);
                }
            }
        }

        fn wait(
            reader: &Reader,
            mut done: impl FnMut(&[PadEvent], &PadStatus) -> bool,
        ) -> Vec<PadEvent> {
            let deadline = Instant::now() + Duration::from_secs(6);
            let mut seen = Vec::new();
            while Instant::now() < deadline {
                seen.extend(reader.drain());
                if done(&seen, &reader.status()) {
                    return seen;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("timed out; saw {seen:?}, status {:?}", reader.status());
        }

        fn presses(events: &[PadEvent]) -> Vec<Control> {
            events
                .iter()
                .filter_map(|event| match event {
                    PadEvent::Pressed { control, .. } => Some(*control),
                    _ => None,
                })
                .collect()
        }

        /// Exercises the whole reader against the kernel: discovery, buttons,
        /// hat, analogue axis, pausing, unplug. Run explicitly on a machine
        /// with `/dev/uinput` access: it creates a short-lived virtual pad
        /// that any desktop session on the machine can also see.
        #[test]
        #[ignore = "needs write access to /dev/uinput"]
        fn a_kernel_gamepad_is_found_read_paused_and_unplugged() {
            let name = "EmuWiz Test Pad";
            let reader = Reader::start(egui::Context::default());
            let pad = VirtualPad::create(name);
            let has_pad = |status: &PadStatus| matches!(status, PadStatus::Connected(names) if names.iter().any(|n| n == name));
            wait(&reader, |_, status| has_pad(status));
            // The kernel has an absolute axis at rest at 0 here, the middle.
            pad.send(EV_KEY, BTN_SOUTH, 1);
            pad.send(EV_KEY, BTN_SOUTH, 0);
            pad.send(EV_ABS, ABS_HAT0X, 1);
            pad.send(EV_ABS, ABS_HAT0X, 0);
            pad.send(EV_ABS, ABS_Y, 30_000);
            pad.send(EV_ABS, ABS_Y, 100);
            pad.send(EV_ABS, ABS_X, 1_000);
            pad.send(EV_KEY, BTN_START, 1);
            let seen = wait(&reader, |seen, _| {
                presses(seen).contains(&Control::Button(Button::Start))
            });
            assert_eq!(
                presses(&seen),
                [
                    Control::Button(Button::South),
                    Control::Direction(Direction::Right),
                    Control::Direction(Direction::Down),
                    Control::Button(Button::Start),
                ],
                "a small stick movement inside the dead zone is not a press"
            );
            pad.send(EV_KEY, BTN_START, 0);
            wait(&reader, |seen, _| {
                seen.iter().any(|e| matches!(e, PadEvent::Released { .. }))
            });

            reader.set_paused(true);
            std::thread::sleep(Duration::from_millis(250));
            pad.send(EV_KEY, BTN_SOUTH, 1);
            pad.send(EV_KEY, BTN_SOUTH, 0);
            std::thread::sleep(Duration::from_millis(450));
            reader.set_paused(false);
            let after = wait(&reader, |seen, _| {
                seen.iter().any(|e| matches!(e, PadEvent::Reset { .. }))
            });
            assert!(presses(&after).is_empty(), "paused input leaked: {after:?}");

            drop(pad);
            wait(&reader, |_, status| !has_pad(status));
        }

        /// What a fake node turns out to be when opened.
        #[derive(Clone)]
        enum Kind {
            Pad(&'static str),
            NotAPad,
            /// Could not be opened; the capabilities say whether it is a pad.
            Denied(Option<bool>),
            /// Removed between listing and opening.
            Gone,
        }

        /// Device enumeration and probing without the kernel: nodes appear,
        /// vanish and are reused exactly as the test says.
        #[derive(Default)]
        struct Fake {
            nodes: std::cell::RefCell<BTreeMap<PathBuf, (NodeId, Kind)>>,
            probes: std::cell::RefCell<Vec<PathBuf>>,
        }

        fn node(inode: u64) -> NodeId {
            NodeId {
                inode,
                changed: (1_700_000_000, i64::try_from(inode).unwrap()),
            }
        }

        impl Fake {
            fn plug(&self, name: &str, inode: u64, kind: Kind) {
                self.nodes
                    .borrow_mut()
                    .insert(PathBuf::from(name), (node(inode), kind));
            }
            fn unplug(&self, name: &str) {
                self.nodes.borrow_mut().remove(Path::new(name));
            }
            fn probed(&self, name: &str) -> usize {
                self.probes
                    .borrow()
                    .iter()
                    .filter(|path| *path == Path::new(name))
                    .count()
            }
        }

        impl Devices for Fake {
            fn nodes(&self) -> BTreeMap<PathBuf, NodeId> {
                self.nodes
                    .borrow()
                    .iter()
                    .map(|(path, (id, _))| (path.clone(), *id))
                    .collect()
            }
            fn probe(&self, path: &Path, device: u32) -> Probe {
                self.probes.borrow_mut().push(path.to_path_buf());
                match self.nodes.borrow().get(path).map(|(_, kind)| kind.clone()) {
                    Some(Kind::Pad(name)) => Probe::Pad(Open {
                        fd: fs::File::open("/dev/null").unwrap().into(),
                        name: name.into(),
                        decoder: Decoder {
                            device,
                            ..Default::default()
                        },
                    }),
                    Some(Kind::Denied(_)) => Probe::Denied,
                    Some(Kind::Gone) => Probe::Gone,
                    Some(Kind::NotAPad) | None => Probe::NotAPad,
                }
            }
            fn denied_is_gamepad(&self, path: &Path) -> Option<bool> {
                match self.nodes.borrow().get(path) {
                    Some((_, Kind::Denied(pad))) => *pad,
                    _ => None,
                }
            }
        }

        fn scan(scanner: &mut Scanner, fake: &Fake) -> (PadStatus, Vec<PadEvent>) {
            let mut events = Vec::new();
            let status = scanner.scan(fake, &mut events);
            (status, events)
        }

        fn connected(names: &[&str]) -> PadStatus {
            PadStatus::Connected(names.iter().map(|name| (*name).to_string()).collect())
        }

        #[test]
        fn a_pad_given_a_recently_freed_event_node_is_found_without_a_replug() {
            let fake = Fake::default();
            let mut scanner = Scanner::default();
            fake.plug("event7", 100, Kind::NotAPad);
            assert_eq!(
                scan(&mut scanner, &fake).0,
                PadStatus::NotFound { unreadable: 0 }
            );
            // Inside one rescan interval the mouse on event7 leaves and the
            // kernel gives the next device, a controller, the same name.
            fake.unplug("event7");
            fake.plug("event7", 101, Kind::Pad("Pad"));
            assert_eq!(scan(&mut scanner, &fake).0, connected(&["Pad"]));
            // A node that vanished as it was opened is not written off.
            fake.plug("event8", 200, Kind::Gone);
            scan(&mut scanner, &fake);
            fake.plug("event8", 200, Kind::Pad("Pad 2"));
            assert_eq!(scan(&mut scanner, &fake).0, connected(&["Pad", "Pad 2"]));
            // An unchanged non-controller is still not reopened.
            fake.plug("event2", 50, Kind::NotAPad);
            scan(&mut scanner, &fake);
            scan(&mut scanner, &fake);
            assert_eq!(fake.probed("event2"), 1);
        }

        #[test]
        fn a_pad_that_disconnects_and_wakes_again_is_found_again() {
            let fake = Fake::default();
            let mut scanner = Scanner::default();
            fake.plug("event5", 10, Kind::Pad("Pad"));
            assert_eq!(scan(&mut scanner, &fake).0, connected(&["Pad"]));
            fake.unplug("event5");
            let (status, events) = scan(&mut scanner, &fake);
            assert_eq!(status, PadStatus::NotFound { unreadable: 0 });
            assert!(
                matches!(events[..], [PadEvent::Reset { .. }]),
                "held controls are dropped"
            );
            // Bluetooth wake: the same pad comes back on the same node name.
            fake.plug("event5", 11, Kind::Pad("Pad"));
            assert_eq!(scan(&mut scanner, &fake).0, connected(&["Pad"]));
            // Or on another name, alongside a second pad.
            fake.plug("event9", 12, Kind::Pad("Pad 2"));
            assert_eq!(scan(&mut scanner, &fake).0, connected(&["Pad", "Pad 2"]));
        }

        #[test]
        fn unreadable_keyboards_and_mice_are_not_a_permission_problem() {
            let fake = Fake::default();
            let mut scanner = Scanner::default();
            fake.plug("event1", 1, Kind::Denied(Some(false)));
            fake.plug("event2", 2, Kind::Denied(Some(false)));
            fake.plug("event3", 3, Kind::Denied(None));
            assert_eq!(
                scan(&mut scanner, &fake).0,
                PadStatus::NotFound { unreadable: 0 }
            );
            scan(&mut scanner, &fake);
            assert_eq!(
                fake.probed("event1"),
                1,
                "a known non-controller is not reopened"
            );
            assert_eq!(fake.probed("event3"), 2, "an unknown node is tried again");
            // A controller that cannot be read is still reported, and is
            // found once its permission arrives.
            fake.plug("event4", 4, Kind::Denied(Some(true)));
            assert_eq!(
                scan(&mut scanner, &fake).0,
                PadStatus::NotFound { unreadable: 1 }
            );
            fake.plug("event4", 4, Kind::Pad("Pad"));
            assert_eq!(scan(&mut scanner, &fake).0, connected(&["Pad"]));
        }

        #[test]
        fn pointer_devices_on_this_machine_class_are_not_controllers() {
            // QEMU's USB tablet and similar show up as js* nodes: absolute
            // axes with mouse buttons but no gamepad button.
            let mut keys = [0_u8; KEY_BYTES];
            keys[0x110 / 8] |= 1;
            let mut axes = [0_u8; ABS_BYTES];
            axes[0] = 0b11;
            assert!(!is_gamepad(&keys, &axes));
        }
    }
}

#[cfg(target_os = "linux")]
pub(in crate::gui_v2) use linux::Reader;

#[cfg(not(target_os = "linux"))]
mod other {
    use super::{PadEvent, PadStatus};
    /// Keyboard and mouse still work; no controller reader exists here yet.
    pub(in crate::gui_v2) struct Reader;
    impl Reader {
        pub(in crate::gui_v2) fn start(_: eframe::egui::Context) -> Self {
            Self
        }
        pub(in crate::gui_v2) fn status(&self) -> PadStatus {
            PadStatus::Unsupported
        }
        pub(in crate::gui_v2) fn set_paused(&self, _: bool) {}
        pub(in crate::gui_v2) fn drain(&self) -> impl Iterator<Item = PadEvent> + '_ {
            std::iter::empty()
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(in crate::gui_v2) use other::Reader;

#[cfg(test)]
mod tests {
    use super::*;

    fn decoder() -> Decoder {
        Decoder {
            device: 7,
            x: AxisRange { min: 0, max: 255 },
            y: AxisRange {
                min: -32768,
                max: 32767,
            },
            ..Default::default()
        }
    }

    fn decode(decoder: &mut Decoder, kind: u16, code: u16, value: i32) -> Vec<PadEvent> {
        let mut out = Vec::new();
        decoder.event(kind, code, value, &mut out);
        out
    }

    fn pressed(control: Control) -> PadEvent {
        PadEvent::Pressed { device: 7, control }
    }
    fn released(control: Control) -> PadEvent {
        PadEvent::Released { device: 7, control }
    }
    const LEFT: Control = Control::Direction(Direction::Left);
    const RIGHT: Control = Control::Direction(Direction::Right);
    const UP: Control = Control::Direction(Direction::Up);

    #[test]
    fn gamepad_buttons_map_by_physical_position() {
        let mut d = decoder();
        assert_eq!(
            decode(&mut d, EV_KEY, BTN_SOUTH, 1),
            [pressed(Control::Button(Button::South))]
        );
        assert_eq!(
            decode(&mut d, EV_KEY, BTN_SOUTH, 0),
            [released(Control::Button(Button::South))]
        );
        assert_eq!(
            decode(&mut d, EV_KEY, BTN_DPAD_UP, 1),
            [pressed(UP)],
            "pads that report the d-pad as buttons"
        );
        assert!(
            decode(&mut d, EV_KEY, 0x13a, 1).is_empty(),
            "unmapped button"
        );
        assert!(
            decode(&mut d, EV_KEY, BTN_SOUTH, 2).is_empty(),
            "autorepeat"
        );
    }

    #[test]
    fn hats_press_and_release_each_direction() {
        let mut d = decoder();
        assert_eq!(decode(&mut d, EV_ABS, ABS_HAT0X, -1), [pressed(LEFT)]);
        // Rolling straight from left to right releases one and presses the other.
        assert_eq!(
            decode(&mut d, EV_ABS, ABS_HAT0X, 1),
            [released(LEFT), pressed(RIGHT)]
        );
        assert_eq!(decode(&mut d, EV_ABS, ABS_HAT0X, 0), [released(RIGHT)]);
    }

    #[test]
    fn the_stick_has_a_dead_zone_and_hysteresis_on_any_axis_range() {
        let mut d = decoder();
        assert!(decode(&mut d, EV_ABS, ABS_X, 128).is_empty(), "centre");
        assert!(
            decode(&mut d, EV_ABS, ABS_X, 170).is_empty(),
            "inside dead zone"
        );
        assert_eq!(decode(&mut d, EV_ABS, ABS_X, 250), [pressed(RIGHT)]);
        assert!(
            decode(&mut d, EV_ABS, ABS_X, 180).is_empty(),
            "wobble below the press point does not release"
        );
        assert_eq!(decode(&mut d, EV_ABS, ABS_X, 130), [released(RIGHT)]);
        assert_eq!(decode(&mut d, EV_ABS, ABS_Y, -32768), [pressed(UP)]);
    }

    #[test]
    fn the_stick_and_d_pad_hold_one_direction_together() {
        let mut d = decoder();
        assert_eq!(decode(&mut d, EV_ABS, ABS_X, 255), [pressed(RIGHT)]);
        assert!(
            decode(&mut d, EV_ABS, ABS_HAT0X, 1).is_empty(),
            "already held"
        );
        assert!(
            decode(&mut d, EV_ABS, ABS_HAT0X, 0).is_empty(),
            "the stick still holds it"
        );
        assert!(decode(&mut d, EV_KEY, BTN_DPAD_RIGHT, 1).is_empty());
        assert!(decode(&mut d, EV_KEY, BTN_DPAD_RIGHT, 0).is_empty());
        assert_eq!(decode(&mut d, EV_ABS, ABS_X, 128), [released(RIGHT)]);
        // The other way round: the d-pad holds while the stick lets go.
        assert_eq!(decode(&mut d, EV_KEY, BTN_DPAD_UP, 1), [pressed(UP)]);
        assert!(decode(&mut d, EV_ABS, ABS_Y, -32768).is_empty());
        assert!(decode(&mut d, EV_ABS, ABS_Y, 0).is_empty());
        assert_eq!(decode(&mut d, EV_KEY, BTN_DPAD_UP, 0), [released(UP)]);
    }

    /// Through the navigator: the repeat survives the d-pad being let go.
    #[test]
    fn letting_go_of_the_d_pad_keeps_a_held_stick_repeating() {
        let mut d = decoder();
        let mut nav = super::super::nav::Navigator::default();
        let mut feed = |d: &mut Decoder, kind, code, value, now| {
            decode(d, kind, code, value)
                .into_iter()
                .filter_map(|event| nav.feed(event, now, true))
                .count()
        };
        assert_eq!(feed(&mut d, EV_ABS, ABS_X, 255, 0.0), 1);
        feed(&mut d, EV_ABS, ABS_HAT0X, 1, 0.1);
        feed(&mut d, EV_ABS, ABS_HAT0X, 0, 0.2);
        assert!(nav.repeating(), "the stick is still held");
    }

    #[test]
    fn sysfs_capabilities_parse_into_the_same_bits() {
        // From a real Xbox 360 pad and this machine's keyboard.
        let pad = parse_capabilities("7cdb000000000000 0 0 0 0\n", 0x300 / 8).unwrap();
        let axes = parse_capabilities("3003f\n", 0x40 / 8).unwrap();
        assert!(bit(&pad, BTN_SOUTH) && bit(&pad, BTN_START));
        assert!(is_gamepad(&pad, &axes));
        let keyboard = parse_capabilities(
            "402000002 3803078f800d001 feffffdfffefffff fffffffffffffffe",
            0x300 / 8,
        )
        .unwrap();
        assert!(!is_gamepad(
            &keyboard,
            &parse_capabilities("0", 0x40 / 8).unwrap()
        ));
        assert_eq!(parse_capabilities("zz", 8), None);
    }

    #[test]
    fn a_degenerate_axis_range_reads_as_centred() {
        assert_eq!(AxisRange { min: 0, max: 0 }.normal(5), 0.0);
    }

    #[test]
    fn dropped_kernel_events_forget_what_was_held() {
        let mut d = decoder();
        decode(&mut d, EV_ABS, ABS_X, 255);
        assert_eq!(
            decode(&mut d, EV_SYN, SYN_DROPPED, 0),
            [PadEvent::Reset { device: 7 }]
        );
        assert_eq!(decode(&mut d, EV_ABS, ABS_X, 255), [pressed(RIGHT)]);
    }

    #[test]
    fn only_real_gamepad_capabilities_count() {
        let bits = |codes: &[u16], bytes: usize| {
            let mut out = vec![0_u8; bytes];
            for code in codes {
                out[usize::from(*code) / 8] |= 1 << (code % 8);
            }
            out
        };
        let keys = |codes| bits(codes, 0x300 / 8);
        let axes = |codes| bits(codes, 0x40 / 8);
        assert!(is_gamepad(&keys(&[BTN_SOUTH]), &axes(&[ABS_X, ABS_Y])));
        assert!(is_gamepad(
            &keys(&[BTN_SOUTH]),
            &axes(&[ABS_HAT0X, ABS_HAT0Y])
        ));
        assert!(is_gamepad(&keys(&[BTN_SOUTH, BTN_DPAD_UP]), &axes(&[])));
        assert!(
            !is_gamepad(&keys(&[BTN_SOUTH]), &axes(&[])),
            "nothing steers"
        );
        assert!(
            !is_gamepad(&keys(&[0x110]), &axes(&[ABS_X, ABS_Y])),
            "mouse"
        );
        assert!(
            !is_gamepad(&keys(&[BTN_SOUTH, BTN_TOUCH]), &axes(&[ABS_X, ABS_Y])),
            "touch surface"
        );
    }

    #[test]
    fn status_says_plainly_why_no_controller_is_available() {
        assert_eq!(
            PadStatus::NotFound { unreadable: 0 }.describe(),
            "No controller found"
        );
        assert!(
            PadStatus::NotFound { unreadable: 1 }
                .describe()
                .contains("permission")
        );
        assert!(
            PadStatus::NotFound { unreadable: 3 }
                .describe()
                .starts_with("3 controllers")
        );
        let one = PadStatus::Connected(vec!["Pad".into()]);
        assert!(one.connected());
        assert_eq!(one.describe(), "Controller: Pad");
    }
}
