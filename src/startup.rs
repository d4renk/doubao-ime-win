//! Operating-system startup registration.

use anyhow::Result;

const VALUE_NAME: &str = "DoubaoVoiceInput";

#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Return whether the startup entry points to the current executable.
#[cfg(target_os = "windows")]
pub(crate) fn is_enabled() -> Result<bool> {
    use anyhow::Context;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, ERROR_UNSUPPORTED_TYPE};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let subkey = wide_null(RUN_KEY);
    let value_name = wide_null(VALUE_NAME);
    let mut byte_count = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut byte_count),
        )
    };
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_UNSUPPORTED_TYPE {
        return Ok(false);
    }
    status
        .ok()
        .context("unable to read the Windows startup entry")?;

    let mut value = vec![0u16; (byte_count as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(value.as_mut_ptr().cast()),
            Some(&mut byte_count),
        )
    };
    if status != ERROR_SUCCESS {
        status
            .ok()
            .context("unable to read the Windows startup entry value")?;
    }

    let length = (byte_count as usize / 2).min(value.len());
    let length = value[..length]
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(length);
    let registered = OsString::from_wide(&value[..length]);
    let executable = std::env::current_exe().context("unable to locate the current executable")?;
    let expected = startup_command(executable);
    Ok(registered == expected)
}

/// Create or remove the current user's startup entry.
#[cfg(target_os = "windows")]
pub(crate) fn set_enabled(enabled: bool) -> Result<()> {
    use anyhow::Context;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let subkey = wide_null(RUN_KEY);
    let value_name = wide_null(VALUE_NAME);
    let mut key = HKEY::default();

    if enabled {
        let executable =
            std::env::current_exe().context("unable to locate the current executable")?;
        let command = startup_command(executable);
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        }
        .ok()
        .context("unable to open the Windows startup registry key")?;

        let data = command
            .encode_wide()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let result =
            unsafe { RegSetValueExW(key, PCWSTR(value_name.as_ptr()), None, REG_SZ, Some(&data)) }
                .ok()
                .context("unable to create the Windows startup entry");
        let _ = unsafe { RegCloseKey(key) };
        result
    } else {
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                None,
                KEY_SET_VALUE,
                &mut key,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        status
            .ok()
            .context("unable to open the Windows startup registry key")?;

        let status = unsafe { RegDeleteValueW(key, PCWSTR(value_name.as_ptr())) };
        let _ = unsafe { RegCloseKey(key) };
        if status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            status
                .ok()
                .context("unable to remove the Windows startup entry")
        }
    }
}

#[cfg(target_os = "windows")]
fn startup_command(path: std::path::PathBuf) -> std::ffi::OsString {
    use std::ffi::OsString;

    let mut command = OsString::from("\"");
    command.push(path);
    command.push("\"");
    command
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn is_enabled() -> Result<bool> {
    Ok(false)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn set_enabled(enabled: bool) -> Result<()> {
    if enabled {
        anyhow::bail!("automatic startup is only supported on Windows");
    }
    Ok(())
}
