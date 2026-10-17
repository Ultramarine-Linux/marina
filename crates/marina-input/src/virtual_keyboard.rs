//! Injects a no-op key tap into InputPlumber's virtual keyboard.
//!
//! wlroots resumes libinput after a session pause without draining the
//! `DEVICE_ADDED` events that resume queues, so Sway keeps an empty seat until
//! a physical event on a device it owns wakes libinput's fd. The handheld's
//! gamepad reaches Sway only through InputPlumber's separate virtual pad, so
//! controller presses never do. Writing to an evdev node injects the event to
//! every reader of that node, which is enough to make libinput dispatch.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
};

const VIRTUAL_KEYBOARD_NAME: &str = "InputPlumber Keyboard";
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const SYN_REPORT: u16 = 0;
// Unbound by Sway and Marina, and supported by InputPlumber's keyboard target.
const KEY_F24: u16 = 194;

/// Taps `KEY_F24` on InputPlumber's virtual keyboard.
///
/// Requires write access to the keyboard's evdev node (the `input` group on
/// the handheld image).
pub fn nudge_virtual_keyboard() -> io::Result<()> {
    let path = find_virtual_keyboard()?;
    let mut device = OpenOptions::new().write(true).open(&path)?;
    let mut events = Vec::with_capacity(4 * INPUT_EVENT_SIZE);
    for (kind, code, value) in [
        (EV_KEY, KEY_F24, 1),
        (EV_SYN, SYN_REPORT, 0),
        (EV_KEY, KEY_F24, 0),
        (EV_SYN, SYN_REPORT, 0),
    ] {
        events.extend_from_slice(&input_event(kind, code, value));
    }
    device.write_all(&events)
}

fn find_virtual_keyboard() -> io::Result<PathBuf> {
    for entry in fs::read_dir("/sys/class/input")? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(node) = name.to_str().filter(|name| name.starts_with("event")) else {
            continue;
        };
        let device_name = fs::read_to_string(entry.path().join("device/name")).unwrap_or_default();
        if device_name.trim() == VIRTUAL_KEYBOARD_NAME {
            return Ok(PathBuf::from("/dev/input").join(node));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("{VIRTUAL_KEYBOARD_NAME} evdev node not found"),
    ))
}

// `struct input_event` on 64-bit Linux: a zeroed `timeval` (the kernel stamps
// injected events itself), then type, code and value.
const INPUT_EVENT_SIZE: usize = 24;

fn input_event(kind: u16, code: u16, value: i32) -> [u8; INPUT_EVENT_SIZE] {
    let mut event = [0; INPUT_EVENT_SIZE];
    event[16..18].copy_from_slice(&kind.to_ne_bytes());
    event[18..20].copy_from_slice(&code.to_ne_bytes());
    event[20..24].copy_from_slice(&value.to_ne_bytes());
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_event_matches_the_64_bit_kernel_layout() {
        let event = input_event(EV_KEY, KEY_F24, 1);
        assert_eq!(&event[..16], &[0; 16]);
        assert_eq!(u16::from_ne_bytes([event[16], event[17]]), EV_KEY);
        assert_eq!(u16::from_ne_bytes([event[18], event[19]]), KEY_F24);
        assert_eq!(
            i32::from_ne_bytes([event[20], event[21], event[22], event[23]]),
            1
        );
    }
}
