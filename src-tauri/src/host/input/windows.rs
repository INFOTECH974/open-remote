//! Injection sous Windows via `SendInput`.
//!
//! On n'utilise pas enigo ici : ses coordonnées absolues ne couvrent que
//! l'écran principal, et il traduit les caractères avec la disposition
//! clavier locale. `SendInput` avec `MOUSEEVENTF_VIRTUALDESK` couvre tous les
//! écrans, et `KEYEVENTF_SCANCODE` envoie la touche physique.

use distant_proto::session::MouseButton;
use windows_sys::Win32::UI::{
    Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
        MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    },
    WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    },
};

use super::keymap::{KeyDef, NONE};

const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

pub struct Backend;

impl Backend {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self)
    }

    /// `x`, `y` : pixels physiques dans le bureau virtuel (tous écrans confondus).
    pub fn move_to(&mut self, x: i32, y: i32) {
        // SAFETY : simple lecture de métriques système.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        if vw <= 1 || vh <= 1 {
            return;
        }
        // Coordonnées normalisées 0..65535 sur le bureau virtuel.
        let nx = ((x - vx) as i64 * 65535 / (vw - 1) as i64) as i32;
        let ny = ((y - vy) as i64 * 65535 / (vh - 1) as i64) as i32;
        send(&[mouse(nx, ny, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)]);
    }

    pub fn button(&mut self, button: MouseButton, down: bool) {
        let (flags, data) = match (button, down) {
            (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
            (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
        };
        send(&[mouse(0, 0, data, flags)]);
    }

    /// Unités de 1/120 de cran. Sous Windows, une valeur positive fait
    /// défiler vers le haut : on inverse l'axe vertical.
    pub fn wheel(&mut self, dx: i32, dy: i32) {
        if dy != 0 {
            send(&[mouse(0, 0, (-dy) as u32, MOUSEEVENTF_WHEEL)]);
        }
        if dx != 0 {
            send(&[mouse(0, 0, dx as u32, MOUSEEVENTF_HWHEEL)]);
        }
    }

    pub fn key(&mut self, key: &KeyDef, down: bool) {
        if key.win == NONE {
            return;
        }
        let mut flags = KEYEVENTF_SCANCODE;
        if key.win & 0xFF00 == 0xE000 {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if !down {
            flags |= KEYEVENTF_KEYUP;
        }
        send(&[INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: key.win & 0x00FF,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }]);
    }
}

fn mouse(dx: i32, dy: i32, data: u32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn send(inputs: &[INPUT]) {
    // SAFETY : `inputs` est un tableau valide de structures INPUT initialisées.
    let sent = unsafe {
        SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32)
    };
    if sent as usize != inputs.len() {
        // Typiquement : fenêtre administrateur au premier plan (UIPI) ou bureau sécurisé.
        tracing::debug!("SendInput a refusé l'événement");
    }
}
