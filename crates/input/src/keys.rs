//! winit physical keys -> HID keyboard usages (page 0x07).
//!
//! Physical keys (scan positions), not characters: iOS applies its own
//! keyboard layout, exactly as with a hardware keyboard. Ctrl is sent as GUI
//! (Command), so Ctrl+C/V/X/A/Z arrive as their Command equivalents
//! (verified in Phase 0).

use winit::keyboard::KeyCode as K;

const LETTERS: [K; 26] = [
    K::KeyA, K::KeyB, K::KeyC, K::KeyD, K::KeyE, K::KeyF, K::KeyG, K::KeyH, K::KeyI, K::KeyJ,
    K::KeyK, K::KeyL, K::KeyM, K::KeyN, K::KeyO, K::KeyP, K::KeyQ, K::KeyR, K::KeyS, K::KeyT,
    K::KeyU, K::KeyV, K::KeyW, K::KeyX, K::KeyY, K::KeyZ,
];

const DIGITS: [K; 10] = [
    K::Digit1, K::Digit2, K::Digit3, K::Digit4, K::Digit5, K::Digit6, K::Digit7, K::Digit8,
    K::Digit9, K::Digit0,
];

pub fn hid_usage(code: K) -> Option<u16> {
    if let Some(i) = LETTERS.iter().position(|k| *k == code) {
        return Some(0x04 + i as u16);
    }
    if let Some(i) = DIGITS.iter().position(|k| *k == code) {
        return Some(0x1E + i as u16);
    }
    Some(match code {
        K::Enter | K::NumpadEnter => 0x28,
        K::Escape => 0x29,
        K::Backspace => 0x2A,
        K::Tab => 0x2B,
        K::Space => 0x2C,
        K::Minus => 0x2D,
        K::Equal => 0x2E,
        K::BracketLeft => 0x2F,
        K::BracketRight => 0x30,
        K::Backslash => 0x31,
        K::Semicolon => 0x33,
        K::Quote => 0x34,
        K::Backquote => 0x35,
        K::Comma => 0x36,
        K::Period => 0x37,
        K::Slash => 0x38,
        K::CapsLock => 0x39,
        K::Home => 0x4A,
        K::PageUp => 0x4B,
        K::Delete => 0x4C,
        K::End => 0x4D,
        K::PageDown => 0x4E,
        K::ArrowRight => 0x4F,
        K::ArrowLeft => 0x50,
        K::ArrowDown => 0x51,
        K::ArrowUp => 0x52,
        K::ControlLeft => 0xE3,  // -> Left GUI (Command)
        K::ControlRight => 0xE7, // -> Right GUI
        K::ShiftLeft => 0xE1,
        K::ShiftRight => 0xE5,
        K::AltLeft => 0xE2,  // -> Option
        K::AltRight => 0xE6,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_command_mapping() {
        assert_eq!(hid_usage(K::KeyA), Some(0x04));
        assert_eq!(hid_usage(K::KeyZ), Some(0x1D));
        assert_eq!(hid_usage(K::Digit1), Some(0x1E));
        assert_eq!(hid_usage(K::Digit0), Some(0x27));
        assert_eq!(hid_usage(K::ControlLeft), Some(0xE3), "Ctrl acts as Command");
        assert_eq!(hid_usage(K::F1), None, "F-keys are app shortcuts");
    }
}
