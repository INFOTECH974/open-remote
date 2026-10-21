//! Correspondance entre `KeyboardEvent.code` (touche physique, norme W3C) et
//! les codes natifs de chaque système.
//!
//! - `win` : scancode (jeu 1), préfixé `0xE0..` pour les touches étendues ;
//! - `mac` : virtual keycode macOS (`kVK_*`) ;
//! - `evdev` : code Linux (`KEY_*`), le keycode X11 vaut `evdev + 8`.
//!
//! Valeurs reprises de la table de référence de Chromium (`dom_code_data.inc`).

/// Pas d'équivalent sur ce système.
pub const NONE: u16 = 0xFFFF;

#[derive(Debug, PartialEq, Eq)]
pub struct KeyDef {
    pub code: &'static str,
    pub win: u16,
    pub mac: u16,
    pub evdev: u16,
}

macro_rules! keys {
    ($($code:literal => $win:expr, $mac:expr, $evdev:expr;)*) => {
        &[$(KeyDef { code: $code, win: $win, mac: $mac, evdev: $evdev }),*]
    };
}

static KEYS: &[KeyDef] = keys! {
    "Escape" => 0x0001, 0x35, 1;
    "Digit1" => 0x0002, 0x12, 2;
    "Digit2" => 0x0003, 0x13, 3;
    "Digit3" => 0x0004, 0x14, 4;
    "Digit4" => 0x0005, 0x15, 5;
    "Digit5" => 0x0006, 0x17, 6;
    "Digit6" => 0x0007, 0x16, 7;
    "Digit7" => 0x0008, 0x1A, 8;
    "Digit8" => 0x0009, 0x1C, 9;
    "Digit9" => 0x000A, 0x19, 10;
    "Digit0" => 0x000B, 0x1D, 11;
    "Minus" => 0x000C, 0x1B, 12;
    "Equal" => 0x000D, 0x18, 13;
    "Backspace" => 0x000E, 0x33, 14;
    "Tab" => 0x000F, 0x30, 15;
    "KeyQ" => 0x0010, 0x0C, 16;
    "KeyW" => 0x0011, 0x0D, 17;
    "KeyE" => 0x0012, 0x0E, 18;
    "KeyR" => 0x0013, 0x0F, 19;
    "KeyT" => 0x0014, 0x11, 20;
    "KeyY" => 0x0015, 0x10, 21;
    "KeyU" => 0x0016, 0x20, 22;
    "KeyI" => 0x0017, 0x22, 23;
    "KeyO" => 0x0018, 0x1F, 24;
    "KeyP" => 0x0019, 0x23, 25;
    "BracketLeft" => 0x001A, 0x21, 26;
    "BracketRight" => 0x001B, 0x1E, 27;
    "Enter" => 0x001C, 0x24, 28;
    "ControlLeft" => 0x001D, 0x3B, 29;
    "KeyA" => 0x001E, 0x00, 30;
    "KeyS" => 0x001F, 0x01, 31;
    "KeyD" => 0x0020, 0x02, 32;
    "KeyF" => 0x0021, 0x03, 33;
    "KeyG" => 0x0022, 0x05, 34;
    "KeyH" => 0x0023, 0x04, 35;
    "KeyJ" => 0x0024, 0x26, 36;
    "KeyK" => 0x0025, 0x28, 37;
    "KeyL" => 0x0026, 0x25, 38;
    "Semicolon" => 0x0027, 0x29, 39;
    "Quote" => 0x0028, 0x27, 40;
    "Backquote" => 0x0029, 0x32, 41;
    "ShiftLeft" => 0x002A, 0x38, 42;
    "Backslash" => 0x002B, 0x2A, 43;
    "KeyZ" => 0x002C, 0x06, 44;
    "KeyX" => 0x002D, 0x07, 45;
    "KeyC" => 0x002E, 0x08, 46;
    "KeyV" => 0x002F, 0x09, 47;
    "KeyB" => 0x0030, 0x0B, 48;
    "KeyN" => 0x0031, 0x2D, 49;
    "KeyM" => 0x0032, 0x2E, 50;
    "Comma" => 0x0033, 0x2B, 51;
    "Period" => 0x0034, 0x2F, 52;
    "Slash" => 0x0035, 0x2C, 53;
    "ShiftRight" => 0x0036, 0x3C, 54;
    "NumpadMultiply" => 0x0037, 0x43, 55;
    "AltLeft" => 0x0038, 0x3A, 56;
    "Space" => 0x0039, 0x31, 57;
    "CapsLock" => 0x003A, 0x39, 58;
    "F1" => 0x003B, 0x7A, 59;
    "F2" => 0x003C, 0x78, 60;
    "F3" => 0x003D, 0x63, 61;
    "F4" => 0x003E, 0x76, 62;
    "F5" => 0x003F, 0x60, 63;
    "F6" => 0x0040, 0x61, 64;
    "F7" => 0x0041, 0x62, 65;
    "F8" => 0x0042, 0x64, 66;
    "F9" => 0x0043, 0x65, 67;
    "F10" => 0x0044, 0x6D, 68;
    "Pause" => 0x0045, 0x71, 119;
    "ScrollLock" => 0x0046, 0x6B, 70;
    "Numpad7" => 0x0047, 0x59, 71;
    "Numpad8" => 0x0048, 0x5B, 72;
    "Numpad9" => 0x0049, 0x5C, 73;
    "NumpadSubtract" => 0x004A, 0x4E, 74;
    "Numpad4" => 0x004B, 0x56, 75;
    "Numpad5" => 0x004C, 0x57, 76;
    "Numpad6" => 0x004D, 0x58, 77;
    "NumpadAdd" => 0x004E, 0x45, 78;
    "Numpad1" => 0x004F, 0x53, 79;
    "Numpad2" => 0x0050, 0x54, 80;
    "Numpad3" => 0x0051, 0x55, 81;
    "Numpad0" => 0x0052, 0x52, 82;
    "NumpadDecimal" => 0x0053, 0x41, 83;
    "IntlBackslash" => 0x0056, 0x0A, 86;
    "F11" => 0x0057, 0x67, 87;
    "F12" => 0x0058, 0x6F, 88;
    "NumpadEqual" => 0x0059, 0x51, 117;
    "F13" => 0x0064, 0x69, 183;
    "F14" => 0x0065, 0x6B, 184;
    "F15" => 0x0066, 0x71, 185;
    "IntlRo" => 0x0073, 0x5E, 89;
    "IntlYen" => 0x007D, 0x5D, 124;
    "NumpadComma" => 0x007E, 0x5F, 121;
    "NumpadEnter" => 0xE01C, 0x4C, 96;
    "ControlRight" => 0xE01D, 0x3E, 97;
    "AudioVolumeMute" => 0xE020, 0x4A, 113;
    "AudioVolumeDown" => 0xE02E, 0x49, 114;
    "AudioVolumeUp" => 0xE030, 0x48, 115;
    "NumpadDivide" => 0xE035, 0x4B, 98;
    "PrintScreen" => 0xE037, 0x69, 99;
    "AltRight" => 0xE038, 0x3D, 100;
    "NumLock" => 0xE045, 0x47, 69;
    "Home" => 0xE047, 0x73, 102;
    "ArrowUp" => 0xE048, 0x7E, 103;
    "PageUp" => 0xE049, 0x74, 104;
    "ArrowLeft" => 0xE04B, 0x7B, 105;
    "ArrowRight" => 0xE04D, 0x7C, 106;
    "End" => 0xE04F, 0x77, 107;
    "ArrowDown" => 0xE050, 0x7D, 108;
    "PageDown" => 0xE051, 0x79, 109;
    "Insert" => 0xE052, 0x72, 110;
    "Delete" => 0xE053, 0x75, 111;
    "MetaLeft" => 0xE05B, 0x37, 125;
    "MetaRight" => 0xE05C, 0x36, 126;
    "ContextMenu" => 0xE05D, 0x6E, 127;
};

pub fn lookup(code: &str) -> Option<&'static KeyDef> {
    KEYS.iter().find(|k| k.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn codes_are_unique() {
        let mut seen = HashSet::new();
        for k in KEYS {
            assert!(seen.insert(k.code), "doublon : {}", k.code);
        }
        let wins: Vec<_> = KEYS.iter().map(|k| k.win).collect();
        let unique: HashSet<_> = wins.iter().collect();
        assert_eq!(wins.len(), unique.len(), "scancode Windows en double");
    }

    #[test]
    fn lookup_works() {
        assert_eq!(lookup("KeyA").unwrap().win, 0x1E);
        assert_eq!(lookup("ArrowUp").unwrap().win, 0xE048);
        assert!(lookup("Inconnue").is_none());
    }
}
