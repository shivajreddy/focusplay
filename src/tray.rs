use anyhow::{anyhow, Context, Result};
use std::sync::{Arc, OnceLock};
use tracing::{debug, error, info};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW, PostQuitMessage, RegisterClassW,
    SetForegroundWindow, TrackPopupMenu, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
    IDI_APPLICATION, MF_CHECKED, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_COMMAND, WM_DESTROY, WM_LBUTTONUP,
    WM_RBUTTONUP, WM_USER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

/// Custom message for tray icon events
const WM_TRAY_ICON: u32 = WM_USER + 1;

/// Menu item IDs
const MENU_ID_DEFAULT: u16 = 1000;
const MENU_ID_SESSION_START: u16 = 2000; // Sessions start at 2000+
const MENU_ID_AUTOSTART: u16 = 3000;
const MENU_ID_EXIT: u16 = 9999;

/// Menu item IDs for browser tabs
const MENU_ID_BROWSER_TAB_START: u16 = 4000;

/// Callback for menu actions
pub enum TrayAction {
    /// User selected Default mode
    SelectDefault,
    /// User selected a session by index
    SelectSession(usize),
    /// User selected a browser tab by tab ID
    SelectBrowserTab(u32),
    /// User toggled autostart
    ToggleAutostart,
    /// User clicked exit
    Exit,
}

/// Callback type for tray events
pub type TrayCallback = Arc<dyn Fn(TrayAction) + Send + Sync>;

/// State for the tray menu
pub struct TrayMenuState {
    pub is_default: bool,
    pub sessions: Vec<TraySession>,
    pub browser_tabs: Vec<TrayBrowserTab>,
    pub autostart_enabled: bool,
}

/// Session info for tray display
#[allow(dead_code)]
pub struct TraySession {
    pub id: String,
    pub display_name: String,
    pub is_selected: bool,
}

/// Browser tab info for tray display
pub struct TrayBrowserTab {
    pub tab_id: u32,
    pub display_name: String,
    pub is_selected: bool,
}

/// System tray icon manager
pub struct TrayIcon {
    hwnd: HWND,
    icon_data: NOTIFYICONDATAW,
    #[allow(dead_code)]
    callback: TrayCallback,
    menu_state: Arc<parking_lot::RwLock<TrayMenuState>>,
}

// Global storage for window procedure callback (safe via OnceLock)
static TRAY_CALLBACK: OnceLock<TrayCallback> = OnceLock::new();
static MENU_STATE: OnceLock<Arc<parking_lot::RwLock<TrayMenuState>>> = OnceLock::new();

impl TrayIcon {
    /// Create a new tray icon
    pub fn new(callback: TrayCallback) -> Result<Self> {
        let menu_state = Arc::new(parking_lot::RwLock::new(TrayMenuState {
            is_default: true,
            sessions: Vec::new(),
            browser_tabs: Vec::new(),
            autostart_enabled: false,
        }));

        // Store globally for window proc
        let _ = TRAY_CALLBACK.set(callback.clone());
        let _ = MENU_STATE.set(menu_state.clone());

        // Create hidden window for tray messages
        let hwnd = Self::create_hidden_window()?;

        // Create tray icon
        let icon_data = Self::create_icon_data(hwnd)?;

        unsafe {
            if !Shell_NotifyIconW(NIM_ADD, &icon_data).as_bool() {
                return Err(anyhow!("Failed to add tray icon"));
            }
        }

        info!("Tray icon created");

        Ok(Self {
            hwnd,
            icon_data,
            callback,
            menu_state,
        })
    }

    /// Update the menu state
    pub fn update_menu_state(&self, state: TrayMenuState) {
        *self.menu_state.write() = state;
    }

    /// Update the tooltip
    #[allow(dead_code)]
    pub fn set_tooltip(&mut self, tooltip: &str) -> Result<()> {
        let tooltip_wide: Vec<u16> = tooltip.encode_utf16().chain(std::iter::once(0)).collect();
        let len = tooltip_wide.len().min(128);
        self.icon_data.szTip[..len].copy_from_slice(&tooltip_wide[..len]);

        unsafe {
            if !Shell_NotifyIconW(NIM_MODIFY, &self.icon_data).as_bool() {
                return Err(anyhow!("Failed to update tooltip"));
            }
        }

        Ok(())
    }

    /// Create the hidden window for receiving tray messages
    fn create_hidden_window() -> Result<HWND> {
        unsafe {
            let instance = GetModuleHandleW(None).context("Failed to get module handle")?;
            let hinstance: HINSTANCE = std::mem::transmute(instance);

            let class_name = w!("FocusPlayTrayClass");

            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: hinstance,
                lpszClassName: class_name,
                ..Default::default()
            };

            let atom = RegisterClassW(&wc);
            if atom == 0 {
                // Class might already be registered, continue
                debug!("Window class already registered or failed");
            }

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class_name,
                w!("FocusPlay"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                hinstance,
                None,
            )
            .context("Failed to create hidden window")?;

            Ok(hwnd)
        }
    }

    /// Create the tray icon data structure
    fn create_icon_data(hwnd: HWND) -> Result<NOTIFYICONDATAW> {
        unsafe {
            let icon = LoadIconW(None, IDI_APPLICATION)?;

            let mut icon_data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: WM_TRAY_ICON,
                hIcon: icon,
                ..Default::default()
            };

            // Set initial tooltip
            let tooltip = w!("FocusPlay - Default");
            let tooltip_bytes = tooltip.as_wide();
            let len = tooltip_bytes.len().min(127);
            icon_data.szTip[..len].copy_from_slice(&tooltip_bytes[..len]);

            Ok(icon_data)
        }
    }

    /// Show the context menu
    fn show_context_menu(hwnd: HWND) -> Result<()> {
        unsafe {
            let state = MENU_STATE.get().unwrap().read();

            let menu = CreatePopupMenu().context("Failed to create popup menu")?;

            // Default option
            let default_flags = if state.is_default {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING | MF_UNCHECKED
            };
            let default_text: Vec<u16> = "Default (system handles)\0".encode_utf16().collect();
            AppendMenuW(
                menu,
                default_flags,
                MENU_ID_DEFAULT as usize,
                PCWSTR(default_text.as_ptr()),
            )?;

            // Separator
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;

            // SMTC Sessions
            for (i, session) in state.sessions.iter().enumerate() {
                let flags = if session.is_selected {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING | MF_UNCHECKED
                };
                let text: Vec<u16> = format!("{}\0", session.display_name)
                    .encode_utf16()
                    .collect();
                AppendMenuW(
                    menu,
                    flags,
                    (MENU_ID_SESSION_START + i as u16) as usize,
                    PCWSTR(text.as_ptr()),
                )?;
            }

            // Browser tabs (from extension)
            if !state.browser_tabs.is_empty() {
                // Separator before browser tabs if there were SMTC sessions
                if !state.sessions.is_empty() {
                    AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
                }

                // Browser tabs header
                let header: Vec<u16> = "Browser Tabs\0".encode_utf16().collect();
                AppendMenuW(
                    menu,
                    MF_STRING | MF_UNCHECKED,
                    0, // No action, just a label
                    PCWSTR(header.as_ptr()),
                )?;

                for (i, tab) in state.browser_tabs.iter().enumerate() {
                    let flags = if tab.is_selected {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING | MF_UNCHECKED
                    };
                    let text: Vec<u16> =
                        format!("  {}\0", tab.display_name).encode_utf16().collect();
                    AppendMenuW(
                        menu,
                        flags,
                        (MENU_ID_BROWSER_TAB_START + i as u16) as usize,
                        PCWSTR(text.as_ptr()),
                    )?;
                }
            }

            // Separator
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;

            // Autostart
            let autostart_flags = if state.autostart_enabled {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING | MF_UNCHECKED
            };
            let autostart_text: Vec<u16> = "Autostart with Windows\0".encode_utf16().collect();
            AppendMenuW(
                menu,
                autostart_flags,
                MENU_ID_AUTOSTART as usize,
                PCWSTR(autostart_text.as_ptr()),
            )?;

            // Exit
            let exit_text: Vec<u16> = "Exit\0".encode_utf16().collect();
            AppendMenuW(
                menu,
                MF_STRING,
                MENU_ID_EXIT as usize,
                PCWSTR(exit_text.as_ptr()),
            )?;

            drop(state); // Release lock before showing menu

            // Get cursor position
            let mut point = windows::Win32::Foundation::POINT::default();
            GetCursorPos(&mut point)?;

            // Need to set foreground window for menu to work properly
            let _ = SetForegroundWindow(hwnd);

            // Show menu
            let _ = TrackPopupMenu(
                menu,
                TPM_BOTTOMALIGN | TPM_LEFTALIGN | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                hwnd,
                None,
            );

            DestroyMenu(menu)?;

            Ok(())
        }
    }

    /// Handle a menu command
    fn handle_menu_command(menu_id: u16) {
        let action = if menu_id == MENU_ID_DEFAULT {
            TrayAction::SelectDefault
        } else if menu_id == MENU_ID_AUTOSTART {
            TrayAction::ToggleAutostart
        } else if menu_id == MENU_ID_EXIT {
            TrayAction::Exit
        } else if menu_id >= MENU_ID_BROWSER_TAB_START {
            // Browser tab selected - look up the tab_id from state
            let index = (menu_id - MENU_ID_BROWSER_TAB_START) as usize;
            if let Some(menu_state) = MENU_STATE.get() {
                let state = menu_state.read();
                if let Some(tab) = state.browser_tabs.get(index) {
                    TrayAction::SelectBrowserTab(tab.tab_id)
                } else {
                    return;
                }
            } else {
                return;
            }
        } else if menu_id >= MENU_ID_SESSION_START && menu_id < MENU_ID_AUTOSTART {
            TrayAction::SelectSession((menu_id - MENU_ID_SESSION_START) as usize)
        } else {
            return;
        };

        if let Some(callback) = TRAY_CALLBACK.get() {
            callback(action);
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.icon_data);
            let _ = DestroyWindow(self.hwnd);
        }
        info!("Tray icon destroyed");
    }
}

/// Window procedure for the hidden window
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAY_ICON => {
            let event = lparam.0 as u32;
            match event {
                WM_RBUTTONUP | WM_LBUTTONUP => {
                    if let Err(e) = TrayIcon::show_context_menu(hwnd) {
                        error!("Failed to show context menu: {}", e);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let menu_id = (wparam.0 & 0xFFFF) as u16;
            TrayIcon::handle_menu_command(menu_id);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Run the Windows message loop
pub fn run_message_loop() -> Result<()> {
    info!("Starting message loop");

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    info!("Message loop ended");
    Ok(())
}

/// Post a quit message to exit the message loop
pub fn post_quit() {
    unsafe {
        PostQuitMessage(0);
    }
}

/// Update the global menu state (can be called from anywhere)
pub fn update_menu_state(state: TrayMenuState) {
    if let Some(menu_state) = MENU_STATE.get() {
        *menu_state.write() = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_menu_ids() {
        assert!(MENU_ID_DEFAULT < MENU_ID_SESSION_START);
        assert!(MENU_ID_SESSION_START < MENU_ID_AUTOSTART);
        assert!(MENU_ID_AUTOSTART < MENU_ID_EXIT);
    }
}
