mod autostart;
mod config;
mod keyboard;
mod session;
mod tray;

use anyhow::Result;
use parking_lot::RwLock;
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use keyboard::MediaKey;
use session::{Mode, SessionManager};
use tray::{TrayAction, TrayIcon, TrayMenuState, TraySession};

/// Application state shared across components
struct AppState {
    session_manager: SessionManager,
    config: config::Config,
}

impl AppState {
    fn new() -> Result<Self> {
        let config = config::Config::load()?;
        let session_manager = SessionManager::new()?;

        Ok(Self {
            session_manager,
            config,
        })
    }

    /// Build the tray menu state from current app state
    fn build_menu_state(&self) -> TrayMenuState {
        let mode = self.session_manager.mode();
        let sessions = self.session_manager.sessions();

        let is_default = matches!(mode, Mode::Default);
        let locked_id = match &mode {
            Mode::Locked(id) => Some(id.clone()),
            Mode::Default => None,
        };

        let tray_sessions: Vec<TraySession> = sessions
            .iter()
            .map(|s| TraySession {
                id: s.id.clone(),
                display_name: SessionManager::format_session_display(s),
                is_selected: locked_id.as_ref() == Some(&s.id),
            })
            .collect();

        TrayMenuState {
            is_default,
            sessions: tray_sessions,
            autostart_enabled: autostart::is_enabled().unwrap_or(false),
        }
    }

    /// Get the locked session display name for tooltip
    fn get_tooltip(&self) -> String {
        match self.session_manager.mode() {
            Mode::Default => "FocusPlay - Default".to_string(),
            Mode::Locked(id) => {
                if let Some(session) = self.session_manager.get_session(&id) {
                    format!(
                        "FocusPlay - {}",
                        SessionManager::format_session_display(&session)
                    )
                } else {
                    "FocusPlay - Locked (unknown)".to_string()
                }
            }
        }
    }
}

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("focusplay=info".parse()?))
        .init();

    info!("FocusPlay starting...");

    // Initialize app state
    let state = Arc::new(RwLock::new(AppState::new()?));

    // Log initial sessions
    {
        let s = state.read();
        let sessions = s.session_manager.sessions();
        info!("Found {} media sessions", sessions.len());
        for session in &sessions {
            info!(
                "  - {} ({:?})",
                SessionManager::format_session_display(session),
                session.status
            );
        }
    }

    // Create tray icon with callback
    let state_for_tray = state.clone();
    let tray_callback = Arc::new(move |action: TrayAction| {
        let mut s = state_for_tray.write();
        match action {
            TrayAction::SelectDefault => {
                info!("Switching to Default mode");
                s.session_manager.set_mode(Mode::Default);
            }
            TrayAction::SelectSession(index) => {
                let sessions = s.session_manager.sessions();
                if let Some(session) = sessions.get(index) {
                    info!("Locking to session: {}", session.id);
                    s.session_manager.set_mode(Mode::Locked(session.id.clone()));
                }
            }
            TrayAction::ToggleAutostart => match autostart::toggle() {
                Ok(enabled) => {
                    info!("Autostart toggled: {}", enabled);
                    s.config.settings.autostart = enabled;
                    if let Err(e) = s.config.save() {
                        error!("Failed to save config: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to toggle autostart: {}", e);
                }
            },
            TrayAction::Exit => {
                info!("Exit requested");
                tray::post_quit();
                return; // Don't update menu state on exit
            }
        }
        // Update tray menu after state change
        tray::update_menu_state(s.build_menu_state());
    });

    let tray = TrayIcon::new(tray_callback)?;

    // Update tray menu with initial state
    {
        let s = state.read();
        tray.update_menu_state(s.build_menu_state());
    }

    // Install keyboard hook
    let state_for_keyboard = state.clone();
    let keyboard_callback = Arc::new(move |key: MediaKey| -> bool {
        let s = state_for_keyboard.read();

        // In default mode, pass through to Windows
        if matches!(s.session_manager.mode(), Mode::Default) {
            return false;
        }

        // In locked mode, handle the key ourselves
        let result = match key {
            MediaKey::PlayPause => s.session_manager.play_pause(),
            MediaKey::NextTrack => s.session_manager.next_track(),
            MediaKey::PrevTrack => s.session_manager.previous_track(),
        };

        match result {
            Ok(true) => {
                info!("Media command sent successfully");
                true // Consume the key
            }
            Ok(false) => {
                warn!("Media command returned false");
                false // Pass through
            }
            Err(e) => {
                error!("Failed to send media command: {}", e);
                false // Pass through on error
            }
        }
    });

    keyboard::install_hook(keyboard_callback)?;

    // Register for session changes
    let state_for_sessions = state.clone();
    let _session_token = {
        let s = state.read();
        s.session_manager.on_sessions_changed(move || {
            let s = state_for_sessions.write();
            if let Err(e) = s.session_manager.refresh_sessions() {
                error!("Failed to refresh sessions: {}", e);
            }
            info!("Sessions changed, refreshed list");
            // Update tray menu with new sessions
            tray::update_menu_state(s.build_menu_state());
        })?
    };

    info!("FocusPlay running. Right-click tray icon to configure.");

    // Run the message loop (blocks until quit)
    tray::run_message_loop()?;

    // Cleanup
    keyboard::uninstall_hook()?;
    drop(tray);

    info!("FocusPlay shutting down");
    Ok(())
}
