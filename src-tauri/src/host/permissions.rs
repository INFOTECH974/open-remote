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
//!
//! Une application sans certificat Apple est reconnue par macOS à son
//! empreinte exacte, qui change à chaque version. Après une mise à jour, les
//! Réglages Système montrent toujours Distant coché, mais cette autorisation
//! est celle de l'ancienne version et ne vaut plus : il faut la retirer
//! ([`reset`]) puis l'accorder de nouveau.

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

/// Ouvre les Réglages Système sur l'autorisation à accorder (`screen` ou
/// `input`), après avoir fait inscrire Distant dans la liste par macOS.
pub fn open_settings(kind: &str) {
    match kind {
        "screen" => {
            sys::request_screen();
            sys::open_settings("Privacy_ScreenCapture");
        }
        _ => {
            sys::request_input();
            sys::open_settings("Privacy_Accessibility");
        }
    }
}

/// Vrai si les autorisations données à cette version vaudront encore pour la
/// suivante. Faux sous macOS quand l'application n'est signée que « ad hoc »
/// (sans certificat) : macOS la reconnaît alors à son empreinte, qui change à
/// chaque version, et une mise à jour installée à distance fait perdre la main
/// tant que personne ne les a redonnées sur place.
pub fn survive_updates() -> bool {
    if cfg!(debug_assertions) {
        if let Ok(value) = std::env::var("DISTANT_FAKE_ADHOC") {
            return value != "1";
        }
    }
    static STABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *STABLE.get_or_init(sys::stable_signature)
}

/// Retire de macOS les autorisations déjà enregistrées pour l'application
/// `identifier` : celles d'une version précédente restent affichées comme
/// accordées sans valoir pour la version qui tourne. Tout est ensuite à
/// accorder de nouveau.
pub fn reset(identifier: &str) -> Result<(), String> {
    sys::reset(identifier)
}

#[cfg(target_os = "macos")]
mod sys {
    use std::{ffi::c_void, process::Command, ptr};

    /// Structure de CoreFoundation dont seule l'adresse nous intéresse.
    #[repr(C)]
    struct Opaque {
        _private: [u8; 0],
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: *const c_void;
        static kCFTypeDictionaryKeyCallBacks: Opaque;
        static kCFTypeDictionaryValueCallBacks: Opaque;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            count: isize,
            key_callbacks: *const Opaque,
            value_callbacks: *const Opaque,
        ) -> *const c_void;
        fn CFRelease(object: *const c_void);
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        static kAXTrustedCheckOptionPrompt: *const c_void;
        /// Renvoient un `Boolean` de Carbon : un octet, non nul si vrai.
        fn AXIsProcessTrusted() -> u8;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
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

    /// Demande l'accessibilité avec l'option « prévenir l'utilisateur » :
    /// macOS inscrit Distant dans la liste et propose d'ouvrir les réglages.
    pub fn request_input() {
        unsafe {
            let keys = [kAXTrustedCheckOptionPrompt];
            let values = [kCFBooleanTrue];
            let options = CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            if !options.is_null() {
                AXIsProcessTrustedWithOptions(options);
                CFRelease(options);
            }
        }
    }

    pub fn open_settings(pane: &str) {
        let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
        if let Err(e) = Command::new("open").arg(url).spawn() {
            tracing::warn!("impossible d'ouvrir les Réglages Système : {e}");
        }
    }

    /// `codesign -dv` décrit la signature de l'application qui tourne ; une
    /// signature sans certificat s'y annonce par « Signature=adhoc ». Dans le
    /// doute (commande absente, sortie inattendue), on la tient pour instable.
    pub fn stable_signature() -> bool {
        let Ok(exe) = std::env::current_exe() else { return false };
        let Ok(output) = Command::new("codesign").arg("-dv").arg(exe).output() else { return false };
        // `codesign` écrit sa description sur la sortie d'erreur.
        let description = String::from_utf8_lossy(&output.stderr);
        output.status.success() && description.contains("Authority=") && !description.contains("Signature=adhoc")
    }

    pub fn reset(identifier: &str) -> Result<(), String> {
        for service in ["ScreenCapture", "Accessibility"] {
            let status = Command::new("tccutil")
                .args(["reset", service, identifier])
                .status()
                .map_err(|e| format!("tccutil introuvable : {e}"))?;
            if !status.success() {
                return Err(format!("macOS a refusé de réinitialiser l'autorisation {service}."));
            }
        }
        Ok(())
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

    pub fn request_input() {}

    pub fn open_settings(_pane: &str) {}

    pub fn stable_signature() -> bool {
        true
    }

    pub fn reset(_identifier: &str) -> Result<(), String> {
        Ok(())
    }
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
