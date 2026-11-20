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

/// The number pad's digit keys, 1-9 then 0 (keypad usages 0x59-0x62).
const KEYPAD: [K; 10] = [
    K::Numpad1, K::Numpad2, K::Numpad3, K::Numpad4, K::Numpad5, K::Numpad6, K::Numpad7, K::Numpad8,
    K::Numpad9, K::Numpad0,
];

/// The HID usage for a key. `num_lock` is Windows's Num Lock state: with it
/// off, the number pad moves the cursor as it does on Windows (7 is Home,
/// 8 is Up and so on), since iOS has no Num Lock of its own.
pub fn hid_usage(code: K, num_lock: bool) -> Option<u16> {
    if !num_lock {
        match code {
            K::Numpad7 => return Some(0x4A), // Home
            K::Numpad8 => return Some(0x52), // Up
            K::Numpad9 => return Some(0x4B), // Page Up
            K::Numpad4 => return Some(0x50), // Left
            K::Numpad6 => return Some(0x4F), // Right
            K::Numpad1 => return Some(0x4D), // End
            K::Numpad2 => return Some(0x51), // Down
            K::Numpad3 => return Some(0x4E), // Page Down
            K::NumpadDecimal => return Some(0x4C), // Delete
            K::Numpad5 | K::Numpad0 => return None, // Clear, Insert: nothing on iOS
            _ => {}
        }
    }
    if let Some(i) = KEYPAD.iter().position(|k| *k == code) {
        return Some(0x59 + i as u16);
    }
    if let Some(i) = LETTERS.iter().position(|k| *k == code) {
        return Some(0x04 + i as u16);
    }
    if let Some(i) = DIGITS.iter().position(|k| *k == code) {
        return Some(0x1E + i as u16);
    }
    Some(match code {
        K::Enter => 0x28,
        K::NumpadEnter => 0x58,
        K::NumpadDivide => 0x54,
        K::NumpadMultiply => 0x55,
        K::NumpadSubtract => 0x56,
        K::NumpadAdd => 0x57,
        K::NumpadDecimal => 0x63,
        K::NumpadEqual => 0x67,
        K::NumpadComma => 0x85,
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
        // Num Lock stays on Windows: it only picks what the pad sends.
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hid_usage_on(code: K) -> Option<u16> {
        hid_usage(code, true)
    }

    #[test]
    fn letters_digits_and_command_mapping() {
        assert_eq!(hid_usage_on(K::KeyA), Some(0x04));
        assert_eq!(hid_usage_on(K::KeyZ), Some(0x1D));
        assert_eq!(hid_usage_on(K::Digit1), Some(0x1E));
        assert_eq!(hid_usage_on(K::Digit0), Some(0x27));
        assert_eq!(hid_usage_on(K::ControlLeft), Some(0xE3), "Ctrl acts as Command");
        assert_eq!(hid_usage_on(K::F1), None, "F-keys are app shortcuts");
    }

    #[test]
    fn number_pad_follows_num_lock() {
        assert_eq!(hid_usage(K::Numpad1, true), Some(0x59));
        assert_eq!(hid_usage(K::Numpad9, true), Some(0x61));
        assert_eq!(hid_usage(K::Numpad0, true), Some(0x62));
        assert_eq!(hid_usage(K::NumpadDecimal, true), Some(0x63));
        assert_eq!(hid_usage(K::NumpadAdd, true), Some(0x57));
        assert_eq!(hid_usage(K::NumpadEnter, true), Some(0x58));
        assert_eq!(hid_usage(K::NumLock, true), None, "Num Lock stays on Windows");

        assert_eq!(hid_usage(K::Numpad7, false), Some(0x4A), "Home");
        assert_eq!(hid_usage(K::Numpad8, false), Some(0x52), "Up");
        assert_eq!(hid_usage(K::NumpadDecimal, false), Some(0x4C), "Delete");
        assert_eq!(hid_usage(K::Numpad5, false), None);
        assert_eq!(hid_usage(K::NumpadAdd, false), Some(0x57), "operators ignore Num Lock");
        assert_eq!(hid_usage(K::NumpadEnter, false), Some(0x58));
    }
}
