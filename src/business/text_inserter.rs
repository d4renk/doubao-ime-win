//! Text insertion service.
//!
//! The real implementation uses Windows `SendInput`.  The non-Windows
//! implementation is an intentionally harmless stub so the configuration,
//! ASR and hotkey state logic can be checked on Ubuntu.

#[cfg(not(target_os = "windows"))]
use anyhow::Result;

use super::context_capture::{is_expected_foreground, TargetWindow};

/// Text inserter service.
pub struct TextInserter;

impl TextInserter {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::{is_expected_foreground, TargetWindow, TextInserter};
    use anyhow::{bail, Result};
    use std::mem::size_of;
    use std::thread;
    use std::time::Duration;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_BACK,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, IsWindow, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    const FOCUS_RETRY_COUNT: usize = 10;
    const FOCUS_RETRY_DELAY: Duration = Duration::from_millis(15);

    impl TextInserter {
        pub fn insert(&self, text: &str) -> Result<()> {
            if text.is_empty() {
                return Ok(());
            }

            let mut inputs: Vec<INPUT> = Vec::new();
            for ch in text.encode_utf16() {
                inputs.push(self.create_unicode_input(ch, true));
                inputs.push(self.create_unicode_input(ch, false));
            }
            self.send_inputs(&inputs)
        }

        pub fn delete_chars(&self, count: usize) -> Result<()> {
            if count == 0 {
                return Ok(());
            }

            let mut inputs: Vec<INPUT> = Vec::new();
            for _ in 0..count {
                inputs.push(self.create_key_input(VK_BACK, true));
                inputs.push(self.create_key_input(VK_BACK, false));
            }
            self.send_inputs(&inputs)
        }

        /// Replace text recently inserted into a specific target window.
        ///
        /// No keyboard input is sent until the captured target is confirmed as
        /// the foreground window, preventing an old result from editing a newly
        /// focused application.
        pub fn replace_recent(
            &self,
            target: TargetWindow,
            original_chars: usize,
            replacement: &str,
        ) -> Result<()> {
            let hwnd = target.hwnd();
            if hwnd.0 == 0 || !unsafe { IsWindow(hwnd).as_bool() } {
                bail!("cannot replace text: target window is invalid");
            }

            if unsafe { IsIconic(hwnd).as_bool() } {
                unsafe {
                    ShowWindow(hwnd, SW_RESTORE);
                }
            }
            unsafe {
                SetForegroundWindow(hwnd);
            }

            let mut focused = false;
            for attempt in 0..FOCUS_RETRY_COUNT {
                if is_expected_foreground(target, TargetWindow::capture_foreground()) {
                    focused = true;
                    break;
                }
                if attempt + 1 < FOCUS_RETRY_COUNT {
                    thread::sleep(FOCUS_RETRY_DELAY);
                }
            }
            if !focused {
                bail!("cannot replace text: target window did not regain foreground focus");
            }

            // Keep deletion and insertion in one SendInput call after the final
            // focus check, minimizing the chance of another app receiving only
            // one half of the replacement.
            let mut inputs = Vec::with_capacity(
                original_chars
                    .saturating_add(replacement.encode_utf16().count())
                    .saturating_mul(2),
            );
            for _ in 0..original_chars {
                inputs.push(self.create_key_input(VK_BACK, true));
                inputs.push(self.create_key_input(VK_BACK, false));
            }
            for ch in replacement.encode_utf16() {
                inputs.push(self.create_unicode_input(ch, true));
                inputs.push(self.create_unicode_input(ch, false));
            }
            self.send_inputs(&inputs)
        }

        fn create_unicode_input(&self, ch: u16, key_down: bool) -> INPUT {
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: ch,
                        dwFlags: if key_down {
                            KEYEVENTF_UNICODE
                        } else {
                            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                        },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        }

        fn create_key_input(&self, vk: VIRTUAL_KEY, key_down: bool) -> INPUT {
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: if key_down {
                            KEYBD_EVENT_FLAGS(0)
                        } else {
                            KEYEVENTF_KEYUP
                        },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        }

        fn send_inputs(&self, inputs: &[INPUT]) -> Result<()> {
            if inputs.is_empty() {
                return Ok(());
            }
            let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
            if sent != inputs.len() as u32 {
                tracing::warn!("SendInput sent {} of {} inputs", sent, inputs.len());
            }
            Ok(())
        }
    }
}

#[cfg(not(target_os = "windows"))]
impl TextInserter {
    pub fn insert(&self, _text: &str) -> Result<()> {
        tracing::debug!("Text insertion is not supported on this platform");
        Ok(())
    }

    pub fn delete_chars(&self, _count: usize) -> Result<()> {
        tracing::debug!("Text deletion is not supported on this platform");
        Ok(())
    }

    pub fn replace_recent(
        &self,
        _target: TargetWindow,
        _original_chars: usize,
        _replacement: &str,
    ) -> Result<()> {
        tracing::debug!("Text replacement is not supported on this platform");
        Ok(())
    }
}

impl Default for TextInserter {
    fn default() -> Self {
        Self::new()
    }
}
