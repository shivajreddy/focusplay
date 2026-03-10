//! Shared message protocol between tray app, native host bridge, and extension.
//! All messages are length-prefixed JSON: 4 bytes (u32 LE) + JSON payload.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Messages FROM the browser extension (extension -> tray app)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ExtensionMessage {
    /// Tab list update from extension
    #[serde(rename = "tabs_update")]
    TabsUpdate { tabs: Vec<BrowserTab> },
}

/// Messages TO the browser extension (tray app -> extension)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HostMessage {
    /// Play/pause a specific tab
    #[serde(rename = "play_pause")]
    PlayPause { tab_id: u32 },
    /// Next track on a specific tab
    #[serde(rename = "next_track")]
    NextTrack { tab_id: u32 },
    /// Previous track on a specific tab
    #[serde(rename = "prev_track")]
    PrevTrack { tab_id: u32 },
    /// Request current tab list
    #[serde(rename = "get_tabs")]
    GetTabs,
}

/// Browser tab info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserTab {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub audible: bool,
    pub muted: bool,
}

/// Named pipe name
pub const PIPE_NAME: &str = r"\\.\pipe\focusplay";

/// Read a length-prefixed JSON message
pub fn read_message<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> Result<Option<T>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > 1024 * 1024 {
        return Ok(None);
    }

    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .context("Failed to read message payload")?;

    let msg: T = serde_json::from_slice(&buf).context("Failed to parse message")?;
    Ok(Some(msg))
}

/// Write a length-prefixed JSON message
pub fn write_message<T: Serialize>(writer: &mut impl Write, msg: &T) -> Result<()> {
    let json = serde_json::to_vec(msg).context("Failed to serialize message")?;
    let len = json.len() as u32;

    writer
        .write_all(&len.to_le_bytes())
        .context("Failed to write message length")?;
    writer
        .write_all(&json)
        .context("Failed to write message payload")?;
    writer.flush().context("Failed to flush")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_host_message() {
        let msg = HostMessage::PlayPause { tab_id: 42 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("play_pause"));
        assert!(json.contains("42"));
    }

    #[test]
    fn test_deserialize_tabs_update() {
        let json = r#"{
            "type": "tabs_update",
            "tabs": [
                {"id": 1, "title": "YouTube - Song", "url": "https://youtube.com", "audible": true, "muted": false}
            ]
        }"#;
        let msg: ExtensionMessage = serde_json::from_str(json).unwrap();
        match msg {
            ExtensionMessage::TabsUpdate { tabs } => {
                assert_eq!(tabs.len(), 1);
                assert_eq!(tabs[0].id, 1);
                assert_eq!(tabs[0].title, "YouTube - Song");
                assert!(tabs[0].audible);
            }
        }
    }

    #[test]
    fn test_roundtrip_message() {
        let msg = HostMessage::GetTabs;
        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();

        assert!(buf.len() > 4);
        let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        assert_eq!(len, buf.len() - 4);

        let json: serde_json::Value = serde_json::from_slice(&buf[4..]).unwrap();
        assert_eq!(json["type"], "get_tabs");
    }

    #[test]
    fn test_roundtrip_extension_message() {
        let msg = ExtensionMessage::TabsUpdate {
            tabs: vec![BrowserTab {
                id: 5,
                title: "Test".into(),
                url: "https://example.com".into(),
                audible: true,
                muted: false,
            }],
        };

        let mut buf = Vec::new();
        write_message(&mut buf, &msg).unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let decoded: Option<ExtensionMessage> = read_message(&mut cursor).unwrap();
        assert!(decoded.is_some());

        match decoded.unwrap() {
            ExtensionMessage::TabsUpdate { tabs } => {
                assert_eq!(tabs.len(), 1);
                assert_eq!(tabs[0].id, 5);
            }
        }
    }
}
