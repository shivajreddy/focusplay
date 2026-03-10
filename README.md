# FocusPlay

A lightweight Windows system tray app that lets you control which media session receives your media keys (Play/Pause, Next, Previous).

## The Problem

When multiple apps are playing media, Windows routes media keys to the most recently active session. Open a new YouTube video? Your media keys now control that instead of Spotify. FocusPlay fixes this by letting you lock media keys to a specific session.

## Features

- **Session Locking**: Right-click the tray icon to see all active media sessions and pick which one your media keys control
- **Minimal Footprint**: Written in Rust, uses ~5MB RAM
- **Default Mode**: When set to "Default", FocusPlay stays out of the way and lets Windows handle media keys normally
- **Auto-Recovery**: If your locked session closes, automatically reverts to default mode

## Usage

1. Run `focusplay.exe`
2. Right-click the tray icon
3. Select a media session to lock to, or "Default" to let Windows handle it
4. Your media keys now control the selected session

## Configuration

Config file location: `%APPDATA%\focusplay\config.toml`

```toml
[settings]
autostart = false        # Start with Windows
show_notifications = true # Show Windows notifications
```

## Requirements

- Windows 10 version 1809 or later

## Building

```bash
cargo build --release
```

## License

MIT
