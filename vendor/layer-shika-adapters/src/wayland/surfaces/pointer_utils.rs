use slint::platform::PointerEventButton;

pub(crate) fn wayland_button_to_slint(button: u32) -> PointerEventButton {
    match button {
        0x110 => PointerEventButton::Left,
        0x111 => PointerEventButton::Right,
        0x112 => PointerEventButton::Middle,
        0x115 => PointerEventButton::Forward,
        0x116 => PointerEventButton::Back,
        _ => PointerEventButton::Other,
    }
}
