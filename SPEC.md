# FocusPlay - Technical Specification

## Overview

FocusPlay is a lightweight Windows system tray application that gives users control over which media session receives media key commands (Play/Pause, Next, Previous). It solves the problem of Windows automatically routing media keys to the most recently active media session, which can be frustrating when multiple apps are playing audio/video.

## Problem Statement

Windows uses the System Media Transport Controls (SMTC) API to manage media sessions. When multiple applications are playing media:
- Media keys are routed to the "current" session (most recently active)
- Opening a new video/app hijacks the media keys
- Users have no control over which session receives commands
- Browser tabs share a single session, complicating per-tab control

## Solution

FocusPlay intercepts media keys via a low-level keyboard hook and routes commands to a user-selected session via the SMTC API, bypassing Windows' automatic routing.

---

## Functional Requirements

### 1. System Tray Application

- Runs as a background process with a system tray icon
- Minimal resource footprint (target: <5MB RAM)
- No main window - all interaction via tray menu

### 2. Tray Menu

```
+----------------------------------+
| [*] Default (system handles)     |
|----------------------------------|
| [ ] Spotify - Song Name          |
| [ ] Chrome - YouTube - Video     |
| [ ] VLC - movie.mp4              |
|----------------------------------|
| [ ] Autostart with Windows       |
| Exit                             |
+----------------------------------+
```

**Menu Items:**
- **Default**: When selected, FocusPlay passes media keys through to Windows (no interception)
- **Session list**: Shows all active media sessions with app name and media title
- **Autostart**: Toggle to enable/disable starting with Windows
- **Exit**: Close the application

**Session Display Format:**
```
{App Name} - {Media Title}
```
Examples:
- `Spotify - Never Gonna Give You Up`
- `Chrome - YouTube - Lofi beats to study to`
- `VLC - movie.mp4`

### 3. Media Key Interception

**Supported Keys:**
- Play/Pause (`VK_MEDIA_PLAY_PAUSE` - 0xB3)
- Next Track (`VK_MEDIA_NEXT_TRACK` - 0xB0)
- Previous Track (`VK_MEDIA_PREV_TRACK` - 0xB1)

**Behavior:**

| Mode | Key Press | Action |
|------|-----------|--------|
| Default | Any media key | Pass through to Windows (no interception) |
| Locked | Play/Pause | Call `TryTogglePlayPauseAsync()` on locked session |
| Locked | Next | Call `TrySkipNextAsync()` on locked session |
| Locked | Previous | Call `TrySkipPreviousAsync()` on locked session |

### 4. Session Monitoring

- Monitor SMTC `GlobalSystemMediaTransportControlsSessionManager`
- Listen for `SessionsChanged` event to update session list
- Listen for `CurrentSessionChanged` event (for future use)
- Track session metadata (app name, title) via `MediaPropertiesChanged` event

### 5. Session Death Handling

When a locked session is closed or crashes:
1. Detect via `SessionsChanged` event (session no longer in list)
2. Automatically revert to Default mode
3. Show Windows notification: "FocusPlay: Session ended, reverted to default"

### 6. Configuration

**File Location:** `%APPDATA%\focusplay\config.toml`

**Schema:**
```toml
[settings]
# Start FocusPlay when Windows starts
autostart = false

# Show Windows notifications for events
show_notifications = true
```

### 7. Autostart

When enabled:
- Create registry entry at `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`
- Key: `FocusPlay`
- Value: Path to executable

When disabled:
- Remove the registry entry

---

## Technical Architecture

### Components

```
+---------------------------------------------------+
|                    Main Thread                     |
|                                                    |
|  +-------------+  +-------------+  +------------+ |
|  | Tray Icon   |  | Menu Handler|  | Config Mgr | |
|  +-------------+  +-------------+  +------------+ |
|                                                    |
+------------------------+---------------------------+
                         |
                         | (channels)
                         v
+------------------------+---------------------------+
|                 Session Manager Task               |
|                                                    |
|  +------------------+  +------------------------+  |
|  | SMTC Integration |  | Session State Tracker  |  |
|  +------------------+  +------------------------+  |
|                                                    |
+---------------------------------------------------+
                         |
                         | (channels)
                         v
+---------------------------------------------------+
|              Keyboard Hook Thread                  |
|                                                    |
|  +------------------+  +------------------------+  |
|  | Low-level Hook   |  | Key -> Command Router  |  |
|  +------------------+  +------------------------+  |
|                                                    |
+---------------------------------------------------+
```

### Threading Model

1. **Main Thread**: Windows message pump, tray icon, menu handling
2. **Session Manager Task**: Async task for SMTC event handling (tokio)
3. **Keyboard Hook**: Must run on a thread with a message pump (Windows requirement)

### Data Flow

```
User presses Play/Pause
         |
         v
+------------------+
| Keyboard Hook    |
| (low-level)      |
+--------+---------+
         |
         v
+------------------+     +------------------+
| Mode = Default?  |---->| Pass to Windows  |
+--------+---------+ Yes +------------------+
         | No
         v
+------------------+
| Get locked       |
| session ID       |
+--------+---------+
         |
         v
+------------------+
| SMTC: Send       |
| command to       |
| session          |
+------------------+
```

### State

```rust
struct AppState {
    // Current mode
    mode: Mode, // Default | Locked(SessionId)
    
    // All active sessions
    sessions: HashMap<SessionId, SessionInfo>,
    
    // Configuration
    config: Config,
}

enum Mode {
    Default,
    Locked(String), // Session ID
}

struct SessionInfo {
    id: String,
    app_name: String,
    title: String,
    // Handle to SMTC session for sending commands
    session: GlobalSystemMediaTransportControlsSession,
}
```

---

## Dependencies

```toml
[dependencies]
# Windows API bindings
windows = { version = "0.58", features = [
    "Media_Control",
    "Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Shell",
    "Win32_System_Registry",
    "Win32_UI_Input_KeyboardAndMouse",
]}

# Async runtime
tokio = { version = "1", features = ["rt-multi-thread", "sync", "macros"] }

# Config
serde = { version = "1", features = ["derive"] }
toml = "0.8"

# Paths
dirs = "5"
```

---

## Platform Requirements

- **OS**: Windows 10 version 1809+ (SMTC API requirement)
- **Architecture**: x86_64

---

## Future Enhancements (Phase 2)

1. **Active Window Binding**: Hotkey to lock media keys to the session associated with the currently focused window

2. **Multiple Session Slots**: Assign different sessions to different hotkey combinations
   ```
   Alt+1 -> Play/Pause Spotify
   Alt+2 -> Play/Pause Chrome
   ```

3. **Custom Hotkeys**: User-configurable hotkeys for:
   - Cycling through sessions
   - Quick-lock to specific apps
   - Clear lock

4. **Per-App Defaults**: Remember preferred session per application context

---

## File Structure

```
focusplay/
├── Cargo.toml
├── SPEC.md
├── src/
│   ├── main.rs           # Entry point, tray setup, message loop
│   ├── config.rs         # Config loading/saving
│   ├── session.rs        # SMTC session management
│   ├── keyboard.rs       # Low-level keyboard hook
│   ├── tray.rs           # System tray icon and menu
│   └── autostart.rs      # Registry operations for autostart
└── assets/
    └── icon.ico          # Tray icon
```

---

## Error Handling

| Scenario | Behavior |
|----------|----------|
| SMTC API unavailable | Show error notification, exit |
| Config file missing | Create with defaults |
| Config file invalid | Use defaults, log warning |
| Session command fails | Ignore silently (session may have ended) |
| Keyboard hook fails | Show error notification, exit |

---

## Notifications

| Event | Message |
|-------|---------|
| Session locked | "Locked to: {App} - {Title}" |
| Session ended | "Session ended, reverted to default" |
| Error | "Error: {description}" |

Notifications respect the `show_notifications` config setting.
