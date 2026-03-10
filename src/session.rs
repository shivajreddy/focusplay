use anyhow::{Context, Result};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};
use windows::Foundation::EventRegistrationToken;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

/// Information about a media session
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionInfo {
    /// Unique identifier for the session (source app user model id)
    pub id: String,
    /// Application name (e.g., "Spotify", "Chrome")
    pub app_name: String,
    /// Current media title
    pub title: String,
    /// Current artist (if available)
    pub artist: String,
    /// Playback status
    pub status: PlaybackStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Closed,
    Opened,
    Changing,
    Stopped,
    Playing,
    Paused,
}

impl From<GlobalSystemMediaTransportControlsSessionPlaybackStatus> for PlaybackStatus {
    fn from(status: GlobalSystemMediaTransportControlsSessionPlaybackStatus) -> Self {
        match status {
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Closed => {
                PlaybackStatus::Closed
            }
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Opened => {
                PlaybackStatus::Opened
            }
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Changing => {
                PlaybackStatus::Changing
            }
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Stopped => {
                PlaybackStatus::Stopped
            }
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing => {
                PlaybackStatus::Playing
            }
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused => {
                PlaybackStatus::Paused
            }
            _ => PlaybackStatus::Closed,
        }
    }
}

/// Target for media key routing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Let Windows handle media keys
    Default,
    /// Lock to a specific SMTC session by ID
    Locked(String),
    /// Lock to a specific browser tab by tab ID
    BrowserTab(u32),
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Default
    }
}

/// Browser tab info (from extension)
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BrowserTabInfo {
    pub tab_id: u32,
    pub title: String,
    pub url: String,
    pub audible: bool,
}

/// Manages media sessions and routes commands
pub struct SessionManager {
    manager: GlobalSystemMediaTransportControlsSessionManager,
    sessions: Arc<RwLock<HashMap<String, SessionInfo>>>,
    browser_tabs: Arc<RwLock<Vec<BrowserTabInfo>>>,
    mode: Arc<RwLock<Mode>>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new() -> Result<Self> {
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .context("Failed to request session manager")?
            .get()
            .context("Failed to get session manager")?;

        let session_manager = Self {
            manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            browser_tabs: Arc::new(RwLock::new(Vec::new())),
            mode: Arc::new(RwLock::new(Mode::Default)),
        };

        // Initial session enumeration
        session_manager.refresh_sessions()?;

        Ok(session_manager)
    }

    /// Get the current mode
    pub fn mode(&self) -> Mode {
        self.mode.read().clone()
    }

    /// Set the mode
    pub fn set_mode(&self, mode: Mode) {
        *self.mode.write() = mode;
    }

    /// Get all current sessions
    pub fn sessions(&self) -> Vec<SessionInfo> {
        self.sessions.read().values().cloned().collect()
    }

    /// Get a specific session by ID
    #[allow(dead_code)]
    pub fn get_session(&self, id: &str) -> Option<SessionInfo> {
        self.sessions.read().get(id).cloned()
    }

    /// Refresh the list of sessions
    pub fn refresh_sessions(&self) -> Result<()> {
        let sessions = self
            .manager
            .GetSessions()
            .context("Failed to get sessions")?;

        let mut session_map = HashMap::new();

        for session in sessions {
            match Self::extract_session_info(&session) {
                Ok(info) => {
                    debug!(?info, "Found session");
                    session_map.insert(info.id.clone(), info);
                }
                Err(e) => {
                    warn!("Failed to extract session info: {}", e);
                }
            }
        }

        // Check if locked SMTC session still exists (browser tabs are checked in update_browser_tabs)
        {
            let mode = self.mode.read();
            if let Mode::Locked(ref id) = *mode {
                if !session_map.contains_key(id) {
                    drop(mode);
                    info!("Locked session no longer exists, reverting to default");
                    self.set_mode(Mode::Default);
                }
            }
        }

        *self.sessions.write() = session_map;
        Ok(())
    }

    /// Extract session info from a SMTC session
    fn extract_session_info(
        session: &GlobalSystemMediaTransportControlsSession,
    ) -> Result<SessionInfo> {
        let id = session
            .SourceAppUserModelId()
            .context("Failed to get source app user model id")?
            .to_string();

        // Extract app name from the ID (e.g., "Spotify.exe" -> "Spotify")
        let app_name = id
            .split('\\')
            .last()
            .unwrap_or(&id)
            .trim_end_matches(".exe")
            .to_string();

        // Get media properties (title, artist)
        let (title, artist) = match session.TryGetMediaPropertiesAsync() {
            Ok(op) => {
                // Use blocking get for simplicity in sync context
                match op.get() {
                    Ok(props) => {
                        let title = props.Title().map(|s| s.to_string()).unwrap_or_default();
                        let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
                        (title, artist)
                    }
                    Err(_) => (String::new(), String::new()),
                }
            }
            Err(_) => (String::new(), String::new()),
        };

        // Get playback status
        let status = session
            .GetPlaybackInfo()
            .ok()
            .and_then(|info| info.PlaybackStatus().ok())
            .map(PlaybackStatus::from)
            .unwrap_or(PlaybackStatus::Closed);

        Ok(SessionInfo {
            id,
            app_name,
            title,
            artist,
            status,
        })
    }

    /// Send play/pause command to the appropriate session
    pub fn play_pause(&self) -> Result<bool> {
        let session = self.get_target_session()?;
        match session {
            Some(s) => s
                .TryTogglePlayPauseAsync()
                .context("Failed to send play/pause")?
                .get()
                .context("Play/pause command failed"),
            None => Ok(false),
        }
    }

    /// Send next track command to the appropriate session
    pub fn next_track(&self) -> Result<bool> {
        let session = self.get_target_session()?;
        match session {
            Some(s) => s
                .TrySkipNextAsync()
                .context("Failed to send next track")?
                .get()
                .context("Next track command failed"),
            None => Ok(false),
        }
    }

    /// Send previous track command to the appropriate session
    pub fn previous_track(&self) -> Result<bool> {
        let session = self.get_target_session()?;
        match session {
            Some(s) => s
                .TrySkipPreviousAsync()
                .context("Failed to send previous track")?
                .get()
                .context("Previous track command failed"),
            None => Ok(false),
        }
    }

    /// Get the SMTC session to send commands to based on current mode
    fn get_target_session(&self) -> Result<Option<GlobalSystemMediaTransportControlsSession>> {
        let mode = self.mode.read();

        match &*mode {
            Mode::Default | Mode::BrowserTab(_) => Ok(None), // Not targeting SMTC
            Mode::Locked(id) => {
                // Find the session with this ID
                let sessions = self
                    .manager
                    .GetSessions()
                    .context("Failed to get sessions")?;

                for session in sessions {
                    if let Ok(session_id) = session.SourceAppUserModelId() {
                        if session_id.to_string() == *id {
                            return Ok(Some(session));
                        }
                    }
                }

                // Session not found
                warn!("Locked session not found: {}", id);
                Ok(None)
            }
        }
    }

    /// Register for session change events
    pub fn on_sessions_changed<F>(&self, callback: F) -> Result<EventRegistrationToken>
    where
        F: Fn() + Send + 'static,
    {
        let token = self
            .manager
            .SessionsChanged(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                callback();
                Ok(())
            }))
            .context("Failed to register sessions changed handler")?;

        Ok(token)
    }

    /// Get a display string for a session (for tray menu)
    pub fn format_session_display(info: &SessionInfo) -> String {
        if info.title.is_empty() {
            info.app_name.clone()
        } else {
            format!("{} - {}", info.app_name, info.title)
        }
    }

    // ========================================================================
    // BROWSER TAB MANAGEMENT
    // ========================================================================

    /// Update browser tabs from extension
    pub fn update_browser_tabs(&self, tabs: Vec<BrowserTabInfo>) {
        // Check if locked browser tab still exists
        {
            let mode = self.mode.read();
            if let Mode::BrowserTab(tab_id) = *mode {
                if !tabs.iter().any(|t| t.tab_id == tab_id) {
                    drop(mode);
                    info!("Locked browser tab no longer audible, reverting to default");
                    self.set_mode(Mode::Default);
                }
            }
        }
        *self.browser_tabs.write() = tabs;
    }

    /// Get all browser tabs
    pub fn browser_tabs(&self) -> Vec<BrowserTabInfo> {
        self.browser_tabs.read().clone()
    }

    /// Check if current mode targets a browser tab
    #[allow(dead_code)]
    pub fn is_browser_tab_mode(&self) -> Option<u32> {
        match *self.mode.read() {
            Mode::BrowserTab(tab_id) => Some(tab_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_default() {
        let mode = Mode::default();
        assert_eq!(mode, Mode::Default);
    }

    #[test]
    fn test_session_display_with_title() {
        let info = SessionInfo {
            id: "test".to_string(),
            app_name: "Spotify".to_string(),
            title: "Never Gonna Give You Up".to_string(),
            artist: "Rick Astley".to_string(),
            status: PlaybackStatus::Playing,
        };
        assert_eq!(
            SessionManager::format_session_display(&info),
            "Spotify - Never Gonna Give You Up"
        );
    }

    #[test]
    fn test_session_display_without_title() {
        let info = SessionInfo {
            id: "test".to_string(),
            app_name: "Spotify".to_string(),
            title: String::new(),
            artist: String::new(),
            status: PlaybackStatus::Paused,
        };
        assert_eq!(SessionManager::format_session_display(&info), "Spotify");
    }
}
