use anyhow::{anyhow, Context, Result};
use std::env;
use tracing::info;
use windows::core::PCWSTR;
use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ,
};

/// Registry key path for autostart
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APP_NAME: &str = "FocusPlay";

/// Helper to convert WIN32_ERROR to Result
fn check_win32(err: WIN32_ERROR, msg: &str) -> Result<()> {
    if err.is_ok() {
        Ok(())
    } else {
        Err(anyhow!("{}: error code {}", msg, err.0))
    }
}

/// Check if autostart is enabled
pub fn is_enabled() -> Result<bool> {
    unsafe {
        let key_path: Vec<u16> = RUN_KEY.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hkey = HKEY::default();

        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );

        if result.is_err() {
            return Ok(false);
        }

        // Try to query the value
        let value_name: Vec<u16> = APP_NAME.encode_utf16().chain(std::iter::once(0)).collect();

        // Check if value exists by trying to query it
        let query_result =
            RegQueryValueExW(hkey, PCWSTR(value_name.as_ptr()), None, None, None, None);

        let _ = RegCloseKey(hkey);

        Ok(query_result.is_ok())
    }
}

/// Enable autostart
pub fn enable() -> Result<()> {
    let exe_path = env::current_exe().context("Failed to get executable path")?;
    let exe_path_str = exe_path.to_string_lossy();

    info!("Enabling autostart: {}", exe_path_str);

    unsafe {
        let key_path: Vec<u16> = RUN_KEY.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hkey = HKEY::default();

        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        check_win32(result, "Failed to open registry key")?;

        let value_name: Vec<u16> = APP_NAME.encode_utf16().chain(std::iter::once(0)).collect();
        let value_data: Vec<u16> = exe_path_str
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let value_bytes: &[u8] =
            std::slice::from_raw_parts(value_data.as_ptr() as *const u8, value_data.len() * 2);

        let result = RegSetValueExW(
            hkey,
            PCWSTR(value_name.as_ptr()),
            0,
            REG_SZ,
            Some(value_bytes),
        );
        check_win32(result, "Failed to set registry value")?;

        let _ = RegCloseKey(hkey);
    }

    info!("Autostart enabled");
    Ok(())
}

/// Disable autostart
pub fn disable() -> Result<()> {
    info!("Disabling autostart");

    unsafe {
        let key_path: Vec<u16> = RUN_KEY.encode_utf16().chain(std::iter::once(0)).collect();
        let mut hkey = HKEY::default();

        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key_path.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );

        if result.is_err() {
            // Key doesn't exist, nothing to disable
            return Ok(());
        }

        let value_name: Vec<u16> = APP_NAME.encode_utf16().chain(std::iter::once(0)).collect();

        // Delete the value (ignore error if it doesn't exist)
        let _ = RegDeleteValueW(hkey, PCWSTR(value_name.as_ptr()));

        let _ = RegCloseKey(hkey);
    }

    info!("Autostart disabled");
    Ok(())
}

/// Toggle autostart
pub fn toggle() -> Result<bool> {
    if is_enabled()? {
        disable()?;
        Ok(false)
    } else {
        enable()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    // Note: Registry tests would modify system state, so we only test basic functionality
    use super::*;

    #[test]
    fn test_constants() {
        assert!(!RUN_KEY.is_empty());
        assert!(!APP_NAME.is_empty());
    }
}
