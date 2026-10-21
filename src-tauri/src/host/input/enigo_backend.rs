//! Injection sous macOS et Linux (X11) via enigo.

use distant_proto::session::MouseButton;
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};

use super::keymap::{KeyDef, NONE};

pub struct Backend {
    enigo: Enigo,
    /// Reste de défilement (en 1/120 de cran) pas encore envoyé.
    wheel_x: i32,
    wheel_y: i32,
}

impl Backend {
    pub fn new() -> anyhow::Result<Self> {
        // Sous macOS, ouvre l'invite d'autorisation « Accessibilité » si besoin.
        let enigo = Enigo::new(&Settings::default())?;
        Ok(Self { enigo, wheel_x: 0, wheel_y: 0 })
    }

    pub fn move_to(&mut self, x: i32, y: i32) {
        log(self.enigo.move_mouse(x, y, Coordinate::Abs));
    }

    pub fn button(&mut self, button: MouseButton, down: bool) {
        let button = match button {
            MouseButton::Left => Button::Left,
            MouseButton::Right => Button::Right,
            MouseButton::Middle => Button::Middle,
            MouseButton::Back => Button::Back,
            MouseButton::Forward => Button::Forward,
        };
        log(self.enigo.button(button, direction(down)));
    }

    /// enigo ne défile que par crans entiers : on accumule les fractions.
    pub fn wheel(&mut self, dx: i32, dy: i32) {
        self.wheel_y += dy;
        let clicks = self.wheel_y / 120;
        if clicks != 0 {
            self.wheel_y -= clicks * 120;
            log(self.enigo.scroll(clicks, Axis::Vertical));
        }
        self.wheel_x += dx;
        let clicks = self.wheel_x / 120;
        if clicks != 0 {
            self.wheel_x -= clicks * 120;
            log(self.enigo.scroll(clicks, Axis::Horizontal));
        }
    }

    pub fn key(&mut self, key: &KeyDef, down: bool) {
        #[cfg(target_os = "macos")]
        let code = key.mac;
        #[cfg(not(target_os = "macos"))]
        let code = if key.evdev == NONE { NONE } else { key.evdev + 8 }; // keycode X11

        if code != NONE {
            log(self.enigo.raw(code, direction(down)));
        }
    }
}

fn direction(down: bool) -> Direction {
    if down {
        Direction::Press
    } else {
        Direction::Release
    }
}

fn log(result: enigo::InputResult<()>) {
    if let Err(e) = result {
        tracing::debug!("injection refusée : {e}");
    }
}
