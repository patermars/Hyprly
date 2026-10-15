use std::{
    collections::HashSet,
    fs::{self, File},
    io::Read,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::mpsc::Sender,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct MousePress {
    pub button: u16,
    pub at: Instant,
}

const EV_KEY: u16 = 0x01;
const BTN_MOUSE_FIRST: u16 = 0x110;
const BTN_MOUSE_LAST: u16 = 0x11f;
const EVIOCGBIT_BASE: u32 = 0x20;

pub fn start(sender: Sender<MousePress>) {
    thread::spawn(move || {
        let mut watched = HashSet::new();
        loop {
            if let Ok(entries) = fs::read_dir("/dev/input") {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !entry.file_name().to_string_lossy().starts_with("event")
                        || !watched.insert(path.clone())
                    {
                        continue;
                    }
                    let device_sender = sender.clone();
                    thread::spawn(move || watch_device(path, device_sender));
                }
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}

fn watch_device(path: PathBuf, sender: Sender<MousePress>) {
    loop {
        match File::open(&path) {
            Ok(mut file) if has_mouse_buttons(&file) => loop {
                match read_event(&mut file) {
                    Ok(event)
                        if event.type_ == EV_KEY
                            && (BTN_MOUSE_FIRST..=BTN_MOUSE_LAST).contains(&event.code)
                            && event.value == 1 =>
                    {
                        if sender
                            .send(MousePress {
                                button: event.code,
                                at: Instant::now(),
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            },
            _ => {}
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn has_mouse_buttons(file: &File) -> bool {
    // Linux EVIOCGBIT(EV_KEY, len): query only capabilities. Keyboard events
    // are never read or forwarded by this listener.
    let mut bits = [0u8; 96];
    let request = ((2u32 << 30)
        | ((bits.len() as u32) << 16)
        | ((b'E' as u32) << 8)
        | (EVIOCGBIT_BASE + EV_KEY as u32)) as libc::c_ulong;
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request, bits.as_mut_ptr()) };
    if result < 0 {
        return false;
    }
    (BTN_MOUSE_FIRST..=BTN_MOUSE_LAST).any(|code| {
        let byte = bits[code as usize / 8];
        byte & (1 << (code % 8)) != 0
    })
}

fn read_event(file: &mut File) -> std::io::Result<libc::input_event> {
    let mut event = std::mem::MaybeUninit::<libc::input_event>::uninit();
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(
            event.as_mut_ptr().cast::<u8>(),
            std::mem::size_of::<libc::input_event>(),
        )
    };
    file.read_exact(bytes)?;
    Ok(unsafe { event.assume_init() })
}
