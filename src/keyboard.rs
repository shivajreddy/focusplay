use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info};
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
};

/// Media key that was pressed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    PlayPause,
    NextTrack,
    PrevTrack,
}

/// Callback type for media key events
/// Return true to consume the key (prevent default handling)
/// Return false to pass the key through to Windows
pub type MediaKeyCallback = Arc<dyn Fn(MediaKey) -> bool + Send + Sync>;

/// Global state for the keyboard hook
static mut HOOK_HANDLE: HHOOK = HHOOK(std::ptr::null_mut());
static mut CALLBACK: Option<MediaKeyCallback> = None;
static HOOK_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Low-level keyboard hook procedure
unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let kb_struct = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk_code = kb_struct.vkCode as u16;

        // Check if this is a key down event
        let is_key_down = wparam.0 == WM_KEYDOWN as usize || wparam.0 == WM_SYSKEYDOWN as usize;

        if is_key_down {
            // Check if it's a media key we care about
            let media_key = match vk_code {
                x if x == VK_MEDIA_PLAY_PAUSE.0 => Some(MediaKey::PlayPause),
                x if x == VK_MEDIA_NEXT_TRACK.0 => Some(MediaKey::NextTrack),
                x if x == VK_MEDIA_PREV_TRACK.0 => Some(MediaKey::PrevTrack),
                _ => None,
            };

            if let Some(key) = media_key {
                debug!(?key, "Media key pressed");

                // Call the callback if set
                if let Some(ref callback) = CALLBACK {
                    let should_consume = callback(key);
                    if should_consume {
                        debug!(?key, "Consuming media key");
                        // Return non-zero to consume the key
                        return LRESULT(1);
                    }
                }
            }
        }
    }

    // Pass to next hook
    CallNextHookEx(HOOK_HANDLE, code, wparam, lparam)
}

/// Install the keyboard hook
pub fn install_hook(callback: MediaKeyCallback) -> Result<()> {
    if HOOK_ACTIVE.load(Ordering::SeqCst) {
        return Ok(()); // Already installed
    }

    info!("Installing keyboard hook");

    unsafe {
        CALLBACK = Some(callback);

        let hook = SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_hook_proc),
            HINSTANCE::default(),
            0,
        )
        .context("Failed to install keyboard hook")?;

        HOOK_HANDLE = hook;
        HOOK_ACTIVE.store(true, Ordering::SeqCst);
    }

    info!("Keyboard hook installed");
    Ok(())
}

/// Uninstall the keyboard hook
pub fn uninstall_hook() -> Result<()> {
    if !HOOK_ACTIVE.load(Ordering::SeqCst) {
        return Ok(()); // Not installed
    }

    info!("Uninstalling keyboard hook");

    unsafe {
        if !HOOK_HANDLE.0.is_null() {
            UnhookWindowsHookEx(HOOK_HANDLE).context("Failed to uninstall keyboard hook")?;
            HOOK_HANDLE = HHOOK(std::ptr::null_mut());
        }
        CALLBACK = None;
        HOOK_ACTIVE.store(false, Ordering::SeqCst);
    }

    info!("Keyboard hook uninstalled");
    Ok(())
}

/// Check if the hook is currently installed
pub fn is_hook_installed() -> bool {
    HOOK_ACTIVE.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_key_equality() {
        assert_eq!(MediaKey::PlayPause, MediaKey::PlayPause);
        assert_ne!(MediaKey::PlayPause, MediaKey::NextTrack);
    }

    #[test]
    fn test_hook_not_installed_initially() {
        // Note: Can't test actual hook installation in unit tests
        // as it requires a Windows message loop
        assert!(!is_hook_installed());
    }
}
