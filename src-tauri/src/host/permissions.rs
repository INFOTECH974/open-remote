//! Autorisations système dont un poste a besoin pour être contrôlé.
//!
//! Seul macOS en demande : sans « Enregistrement de l'écran », la capture
//! renvoie une image vide, sans aucune fenêtre ; sans « Accessibilité », la
//! souris et le clavier injectés sont ignorés. Dans les deux cas macOS ne
//! signale rien à l'application : c'est à elle de poser la question, de le
//! dire sur place et de prévenir le poste qui contrôle.
//!
//! macOS ne relit l'autorisation d'enregistrement de l'écran qu'au lancement
//! de l'application : après l'avoir accordée, il faut relancer Distant.

use serde::Serialize;

/// `true` = accordée. Toujours vrai hors de macOS.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Permissions {
    /// Enregistrement de l'écran.
    pub screen: bool,
    /// Accessibilité : contrôle de la souris et du clavier.
    pub input: bool,
}

/// Noms transmis au poste qui contrôle (`Control::Welcome`).
pub const WARNING_SCREEN: &str = "screen_capture";
pub const WARNING_INPUT: &str = "accessibility";

impl Permissions {
    pub fn check() -> Self {
        // En développement, `DISTANT_FAKE_PERMISSIONS` liste les autorisations
        // accordées (« input », « screen,input », ou vide pour aucune) : de
        // quoi voir l'écran des autorisations sans Mac.
        if cfg!(debug_assertions) {
            if let Ok(granted) = std::env::var("DISTANT_FAKE_PERMISSIONS") {
                return Self { screen: granted.contains("screen"), input: granted.contains("input") };
            }
        }
        Self { screen: sys::screen(), input: sys::input() }
    }

    /// Ce qui manque, pour le poste qui contrôle.
    pub fn warnings(self) -> Vec<String> {
        let mut warnings = Vec::new();
        if !self.screen {
            warnings.push(WARNING_SCREEN.to_string());
        }
        if !self.input {
            warnings.push(WARNING_INPUT.to_string());
        }
        warnings
    }
}

/// Fait apparaître la demande d'autorisation de macOS pour l'enregistrement
/// de l'écran (la première fois seulement) et inscrit Distant dans la liste
/// des Réglages Système.
pub fn request_screen() {
    sys::request_screen();
}

/// Ouvre les Réglages Système sur l'autorisation à accorder (`screen` ou `input`).
pub fn open_settings(kind: &str) {
    match kind {
        "screen" => {
            sys::request_screen();
            sys::open_settings("Privacy_ScreenCapture");
        }
        _ => sys::open_settings("Privacy_Accessibility"),
    }
}

#[cfg(target_os = "macos")]
mod sys {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        /// Renvoie un `Boolean` de Carbon : un octet, non nul si vrai.
        fn AXIsProcessTrusted() -> u8;
    }

    pub fn screen() -> bool {
        // Sans effet de bord : ne fait apparaître aucune demande.
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    pub fn request_screen() {
        unsafe {
            CGRequestScreenCaptureAccess();
        }
    }

    pub fn input() -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    pub fn open_settings(pane: &str) {
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
        if let Err(e) = std::process::Command::new("open").arg(url).spawn() {
            tracing::warn!("impossible d'ouvrir les Réglages Système : {e}");
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod sys {
    pub fn screen() -> bool {
        true
    }

    pub fn request_screen() {}

    pub fn input() -> bool {
        true
    }

    pub fn open_settings(_pane: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_name_what_is_missing() {
        assert!(Permissions { screen: true, input: true }.warnings().is_empty());
        assert_eq!(Permissions { screen: false, input: true }.warnings(), [WARNING_SCREEN]);
        assert_eq!(Permissions { screen: false, input: false }.warnings(), [WARNING_SCREEN, WARNING_INPUT]);
    }
}
