//! Text insertion service.
//!
//! The real implementation uses Windows `SendInput`.  The non-Windows
//! implementation is an intentionally harmless stub so the configuration,
//! ASR and hotkey state logic can be checked on Ubuntu.

use anyhow::Result;

/// Text inserter service.
pub struct TextInserter;

impl TextInserter {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use super::TextInserter;
    use anyhow::Result;
    use std::mem::size_of;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_BACK,
    };

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
}

impl Default for TextInserter {
    fn default() -> Self {
        Self::new()
    }
}
