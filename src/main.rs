mod autostart;
mod config;
mod keyboard;
mod native_host;
mod pipe_server;
mod protocol;
mod session;
mod tray;

use anyhow::Result;
use parking_lot::RwLock;
use pipe_server::PipeServer;
use protocol::{ExtensionMessage, HostMessage};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use keyboard::MediaKey;
use session::{BrowserTabInfo, Mode, SessionManager};
use tray::{TrayAction, TrayIcon, TrayMenuState, TraySession};

/// Application state shared across components
struct AppState {
    session_manager: SessionManager,
    config: config::Config,
    pipe_server: PipeServer,
}

impl AppState {
    fn new() -> Result<Self> {
        let config = config::Config::load()?;
        let session_manager = SessionManager::new()?;
        let pipe_server = PipeServer::start()?;

        Ok(Self {
            session_manager,
            config,
            pipe_server,
        })
    }

    /// Build the tray menu state from current app state
    fn build_menu_state(&self) -> TrayMenuState {
        let mode = self.session_manager.mode();
        let sessions = self.session_manager.sessions();
        let browser_tabs = self.session_manager.browser_tabs();

        let is_default = matches!(mode, Mode::Default);

        // SMTC sessions (hide browser SMTC session if we have per-tab data)
        let has_browser_tabs = !browser_tabs.is_empty();
        let tray_sessions: Vec<TraySession> = sessions
            .iter()
            .filter(|s| {
                if has_browser_tabs {
                    let lower = s.app_name.to_lowercase();
                    !matches!(
                        lower.as_str(),
                        "chrome" | "msedge" | "firefox" | "brave" | "opera"
                    )
                } else {
                    true
                }
            })
            .map(|s| {
                let is_selected = matches!(&mode, Mode::Locked(id) if id == &s.id);
                TraySession {
                    id: s.id.clone(),
                    display_name: SessionManager::format_session_display(s),
                    is_selected,
                }
            })
            .collect();

        // Browser tabs from extension
        let tray_browser_tabs: Vec<tray::TrayBrowserTab> = browser_tabs
            .iter()
            .map(|t| {
                let is_selected = matches!(&mode, Mode::BrowserTab(id) if *id == t.tab_id);
                tray::TrayBrowserTab {
                    tab_id: t.tab_id,
                    display_name: t.title.clone(),
                    is_selected,
                }
            })
            .collect();

        TrayMenuState {
            is_default,
            sessions: tray_sessions,
            browser_tabs: tray_browser_tabs,
            autostart_enabled: autostart::is_enabled().unwrap_or(false),
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Detect if launched by Chrome (passes chrome-extension://... as arg)
    // or with explicit --native-host flag
    let is_native_host = args
        .iter()
        .any(|a| a == "--native-host" || a.starts_with("chrome-extension://"));

    if is_native_host {
        // Bridge mode: stdin/stdout <-> named pipe
        return native_host::run();
    }

    // Normal tray app mode
    run_tray_app()
}

fn run_tray_app() -> Result<()> {
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
            TrayAction::SelectBrowserTab(tab_id) => {
                info!("Locking to browser tab: {}", tab_id);
                s.session_manager.set_mode(Mode::BrowserTab(tab_id));
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
                return;
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
        let mode = s.session_manager.mode();

        match mode {
            Mode::Default => false, // Pass through to Windows

            Mode::Locked(_) => {
                let result = match key {
                    MediaKey::PlayPause => s.session_manager.play_pause(),
                    MediaKey::NextTrack => s.session_manager.next_track(),
                    MediaKey::PrevTrack => s.session_manager.previous_track(),
                };
                match result {
                    Ok(true) => {
                        info!("SMTC media command sent");
                        true
                    }
                    Ok(false) => {
                        warn!("SMTC media command returned false");
                        false
                    }
                    Err(e) => {
                        error!("SMTC media command failed: {}", e);
                        false
                    }
                }
            }

            Mode::BrowserTab(tab_id) => {
                let msg = match key {
                    MediaKey::PlayPause => HostMessage::PlayPause { tab_id },
                    MediaKey::NextTrack => HostMessage::NextTrack { tab_id },
                    MediaKey::PrevTrack => HostMessage::PrevTrack { tab_id },
                };
                match s.pipe_server.send(msg) {
                    Ok(()) => {
                        info!("Browser tab command sent via pipe");
                        true
                    }
                    Err(e) => {
                        error!("Failed to send browser tab command: {}", e);
                        false
                    }
                }
            }
        }
    });

    keyboard::install_hook(keyboard_callback)?;

    // Register for SMTC session changes
    let state_for_sessions = state.clone();
    let _session_token = {
        let s = state.read();
        s.session_manager.on_sessions_changed(move || {
            let s = state_for_sessions.write();
            if let Err(e) = s.session_manager.refresh_sessions() {
                error!("Failed to refresh sessions: {}", e);
            }
            info!("Sessions changed, refreshed list");
            tray::update_menu_state(s.build_menu_state());
        })?
    };

    // Poll for extension messages from the pipe server
    let state_for_pipe = state.clone();
    std::thread::Builder::new()
        .name("pipe-poll".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_millis(100));

            let s = state_for_pipe.read();
            if let Some(msg) = s.pipe_server.try_recv() {
                match msg {
                    ExtensionMessage::TabsUpdate { tabs } => {
                        let tab_infos: Vec<BrowserTabInfo> = tabs
                            .into_iter()
                            .map(|t| BrowserTabInfo {
                                tab_id: t.id,
                                title: t.title,
                                url: t.url,
                                audible: t.audible,
                            })
                            .collect();
                        info!("Browser tabs updated: {} tabs", tab_infos.len());
                        s.session_manager.update_browser_tabs(tab_infos);
                        tray::update_menu_state(s.build_menu_state());
                    }
                }
            }
        })?;

    info!("FocusPlay running. Right-click tray icon to configure.");

    // Run the message loop (blocks until quit)
    tray::run_message_loop()?;

    // Cleanup
    keyboard::uninstall_hook()?;
    drop(tray);

    info!("FocusPlay shutting down");
    Ok(())
}
