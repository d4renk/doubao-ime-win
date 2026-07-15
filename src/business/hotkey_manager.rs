//! Global and raw keyboard shortcut management.
//!
//! Standard shortcuts continue to use `global-hotkey`.  On Windows, raw
//! bindings are observed with low-level keyboard and mouse hooks so vendor
//! keys and mouse side buttons which do not have a `global-hotkey::Code` can
//! still be configured.

use anyhow::{anyhow, Result};
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "windows")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::data::HotkeyConfig;

/// Events emitted by a hotkey listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Toggle recording once.
    Toggle,
}

/// Identity of a Windows raw keyboard event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawKeyBinding {
    pub vk_code: u32,
    pub scan_code: u32,
    pub extended: bool,
}

/// Hotkey manager for global hotkey handling.
#[derive(Clone)]
pub struct HotkeyManager {
    manager: Arc<GlobalHotKeyManager>,
    registered_hotkey: Arc<Mutex<Option<HotKey>>>,
    config: Arc<RwLock<HotkeyConfig>>,
    is_active: Arc<AtomicBool>,
    listener_started: Arc<AtomicBool>,
    #[cfg(target_os = "windows")]
    capture_sender: Arc<Mutex<Option<mpsc::Sender<RawKeyBinding>>>>,
}

impl HotkeyManager {
    /// Create a new hotkey manager based on configuration.
    pub fn new(config: &HotkeyConfig) -> Result<Self> {
        validate_config(config)?;

        let manager = Arc::new(
            GlobalHotKeyManager::new()
                .map_err(|e| anyhow!("Failed to create hotkey manager: {}", e))?,
        );
        let registered_hotkey = if config.binding.eq_ignore_ascii_case("raw") {
            None
        } else if let Some(hotkey) = configured_standard_hotkey(config)? {
            manager
                .register(hotkey)
                .map_err(|e| anyhow!("Failed to register hotkey: {}", e))?;
            tracing::info!("Registered standard hotkey: {}", config.combo_key);
            Some(hotkey)
        } else {
            tracing::info!("Standard modifier double-tap will use the Windows keyboard hook");
            None
        };

        if config.binding.eq_ignore_ascii_case("raw") {
            tracing::info!(
                "Configured raw key binding: vk=0x{:X}, scan=0x{:X}, extended={}",
                config.raw_vk_code,
                config.raw_scan_code,
                config.raw_extended
            );
        }

        Ok(Self {
            manager,
            registered_hotkey: Arc::new(Mutex::new(registered_hotkey)),
            config: Arc::new(RwLock::new(config.clone())),
            is_active: Arc::new(AtomicBool::new(true)),
            listener_started: Arc::new(AtomicBool::new(false)),
            #[cfg(target_os = "windows")]
            capture_sender: Arc::new(Mutex::new(None)),
        })
    }

    /// Reconfigure the active binding without restarting the application.
    pub fn reconfigure(&self, new_config: &HotkeyConfig) -> Result<()> {
        validate_config(new_config)?;

        let new_hotkey = if new_config.binding.eq_ignore_ascii_case("raw") {
            None
        } else {
            configured_standard_hotkey(new_config)?
        };

        let mut current = self
            .registered_hotkey
            .lock()
            .map_err(|_| anyhow!("Hotkey registration state is poisoned"))?;

        if *current != new_hotkey {
            if let Some(hotkey) = new_hotkey {
                self.manager
                    .register(hotkey)
                    .map_err(|e| anyhow!("Failed to register new hotkey: {}", e))?;
            }

            if let Some(old_hotkey) = *current {
                // Registration of the new shortcut succeeded, so a failure to
                // unregister the old one is reported rather than silently
                // leaving two active standard bindings.
                if let Err(error) = self.manager.unregister(old_hotkey) {
                    if let Some(hotkey) = new_hotkey {
                        let _ = self.manager.unregister(hotkey);
                    }
                    return Err(anyhow!("Failed to unregister old hotkey: {}", error));
                }
            }

            *current = new_hotkey;
        }

        *self
            .config
            .write()
            .map_err(|_| anyhow!("Hotkey configuration state is poisoned"))? = new_config.clone();

        tracing::info!("Hotkey configuration applied immediately");
        Ok(())
    }

    /// Set a callback for hotkey events.
    pub fn on_event<F>(&self, callback: F)
    where
        F: Fn(HotkeyEvent) + Send + Sync + 'static,
    {
        if self.listener_started.swap(true, Ordering::SeqCst) {
            tracing::warn!("Hotkey listener was already started");
            return;
        }

        let config = self.config.clone();
        let is_active = self.is_active.clone();
        let callback = Arc::new(callback);

        // Standard events are delivered through the global-hotkey channel.
        let standard_config = config.clone();
        let standard_active = is_active.clone();
        let standard_callback = callback.clone();
        thread::spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            let mut last_press_time: Option<Instant> = None;

            loop {
                if !standard_active.load(Ordering::SeqCst) {
                    break;
                }

                let event = match receiver.recv() {
                    Ok(event) => event,
                    Err(_) => break,
                };

                if !standard_active.load(Ordering::SeqCst)
                    || standard_config
                        .read()
                        .map(|config| config.binding.eq_ignore_ascii_case("raw"))
                        .unwrap_or(true)
                    || event.state != HotKeyState::Pressed
                {
                    continue;
                }

                let current_config = match standard_config.read() {
                    Ok(config) => config.clone(),
                    Err(_) => continue,
                };

                if let Ok(Some(current_hotkey)) = configured_standard_hotkey(&current_config) {
                    if event.id != current_hotkey.id() {
                        continue;
                    }
                }

                if current_config.mode.eq_ignore_ascii_case("combo") {
                    standard_callback(HotkeyEvent::Toggle);
                    continue;
                }

                let now = Instant::now();
                if let Some(last) = last_press_time {
                    if now.duration_since(last)
                        <= Duration::from_millis(current_config.double_tap_interval)
                    {
                        standard_callback(HotkeyEvent::Toggle);
                        last_press_time = None;
                        continue;
                    }
                }
                last_press_time = Some(now);
            }
        });

        // Raw hooks are Windows-only.  The hook stays installed while the
        // application is alive and simply ignores events when standard mode
        // is selected, allowing runtime switching without a restart.
        #[cfg(target_os = "windows")]
        {
            let raw_config = config;
            let raw_active = is_active;
            let capture_sender = self.capture_sender.clone();
            thread::spawn(move || {
                run_raw_key_hook(raw_config, raw_active, callback, capture_sender);
            });
        }

        #[cfg(not(target_os = "windows"))]
        if config
            .read()
            .map(|config| config.binding.eq_ignore_ascii_case("raw"))
            .unwrap_or(false)
        {
            tracing::warn!("Raw keyboard bindings are only supported on Windows");
        }
    }

    /// Backward-compatible convenience API for toggle-only callers.
    pub fn on_trigger<F>(&self, callback: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.on_event(move |event| {
            if event == HotkeyEvent::Toggle {
                callback();
            }
        });
    }

    /// Stop the hotkey listeners and unregister the standard binding.
    pub fn stop(&self) {
        self.is_active.store(false, Ordering::SeqCst);
        if let Ok(mut current) = self.registered_hotkey.lock() {
            if let Some(hotkey) = current.take() {
                let _ = self.manager.unregister(hotkey);
            }
        }
    }

    /// Capture the next physical Windows key for use as a raw binding.
    #[cfg(target_os = "windows")]
    pub fn capture_raw_key(&self, timeout: Duration) -> Result<RawKeyBinding> {
        let (result_tx, result_rx) = mpsc::channel();
        {
            let mut sender = self
                .capture_sender
                .lock()
                .map_err(|_| anyhow!("Raw-key capture state is poisoned"))?;
            if sender.is_some() {
                return Err(anyhow!("A raw-key capture is already in progress"));
            }
            *sender = Some(result_tx);
        }
        let result = result_rx.recv_timeout(timeout);
        if let Ok(mut sender) = self.capture_sender.lock() {
            sender.take();
        }
        result.map_err(|_| {
            anyhow!("No physical key or mouse side-button was captured before timeout")
        })
    }
}

fn validate_config(config: &HotkeyConfig) -> Result<()> {
    if config.binding.eq_ignore_ascii_case("raw") {
        if config.raw_vk_code == 0 {
            return Err(anyhow!("Raw binding requires a non-zero virtual-key code"));
        }
        Ok(())
    } else {
        let _ = configured_standard_hotkey(config)?;
        Ok(())
    }
}

fn configured_standard_hotkey(config: &HotkeyConfig) -> Result<Option<HotKey>> {
    if config.mode.eq_ignore_ascii_case("combo") {
        Ok(Some(parse_combo_key(&config.combo_key)?))
    } else if is_modifier_double_tap(config) {
        Ok(None)
    } else {
        Ok(Some(HotKey::new(
            None,
            parse_key_code(&config.double_tap_key)?,
        )))
    }
}

fn is_modifier_double_tap(config: &HotkeyConfig) -> bool {
    config.mode.eq_ignore_ascii_case("double_tap")
        && matches!(
            config.double_tap_key.to_lowercase().as_str(),
            "ctrl" | "control" | "shift" | "alt"
        )
}

#[cfg(target_os = "windows")]
fn standard_modifier_matches(key: &str, vk_code: u32) -> bool {
    match key.to_lowercase().as_str() {
        "ctrl" | "control" => matches!(vk_code, 0x11 | 0xA2 | 0xA3),
        "shift" => matches!(vk_code, 0x10 | 0xA0 | 0xA1),
        "alt" => matches!(vk_code, 0x12 | 0xA4 | 0xA5),
        _ => false,
    }
}

/// Windows low-level keyboard hook for raw bindings.
#[cfg(target_os = "windows")]
fn run_raw_key_hook(
    config: Arc<RwLock<HotkeyConfig>>,
    is_active: Arc<AtomicBool>,
    callback: Arc<dyn Fn(HotkeyEvent) + Send + Sync>,
    capture_sender: Arc<Mutex<Option<mpsc::Sender<RawKeyBinding>>>>,
) {
    use std::cell::RefCell;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
        KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_QUIT, WM_XBUTTONDOWN,
        WM_XBUTTONUP,
    };

    struct HookState {
        config: Arc<RwLock<HotkeyConfig>>,
        is_active: Arc<AtomicBool>,
        callback: Arc<dyn Fn(HotkeyEvent) + Send + Sync>,
        pressed: Option<RawKeyBinding>,
        last_modifier_release: Option<Instant>,
        capture_sender: Arc<Mutex<Option<mpsc::Sender<RawKeyBinding>>>>,
    }

    thread_local! {
        static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
    }

    HOOK_STATE.with(|state| {
        *state.borrow_mut() = Some(HookState {
            config,
            is_active,
            callback,
            pressed: None,
            last_modifier_release: None,
            capture_sender,
        });
    });

    unsafe extern "system" fn keyboard_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        use windows::Win32::UI::WindowsAndMessaging::{LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_UP};

        if code >= 0 {
            let keyboard = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let flags = keyboard.flags;
            if !flags.contains(LLKHF_INJECTED) {
                HOOK_STATE.with(|state| {
                    if let Some(ref mut hook) = *state.borrow_mut() {
                        if !hook.is_active.load(Ordering::SeqCst) {
                            return;
                        }

                        let identity = RawKeyBinding {
                            vk_code: keyboard.vkCode,
                            scan_code: keyboard.scanCode,
                            extended: flags.contains(LLKHF_EXTENDED),
                        };
                        let is_up = flags.contains(LLKHF_UP)
                            || wparam.0 as u32 == 0x0101
                            || wparam.0 as u32 == 0x0105;

                        if !is_up && deliver_capture(&hook.capture_sender, identity) {
                            return;
                        }

                        let config = match hook.config.read() {
                            Ok(config) => config.clone(),
                            Err(_) => return,
                        };
                        let raw_matches = config.binding.eq_ignore_ascii_case("raw")
                            && config.raw_vk_code == identity.vk_code
                            && (config.raw_scan_code == 0
                                || config.raw_scan_code == identity.scan_code)
                            && config.raw_extended == identity.extended;
                        let modifier_matches = config.binding.eq_ignore_ascii_case("standard")
                            && is_modifier_double_tap(&config)
                            && standard_modifier_matches(&config.double_tap_key, identity.vk_code);

                        if is_up {
                            if hook.pressed == Some(identity) {
                                hook.pressed = None;
                            }
                            if modifier_matches {
                                let now = Instant::now();
                                if hook.last_modifier_release.is_some_and(|last| {
                                    now.duration_since(last)
                                        <= Duration::from_millis(config.double_tap_interval)
                                }) {
                                    (hook.callback)(HotkeyEvent::Toggle);
                                    hook.last_modifier_release = None;
                                } else {
                                    hook.last_modifier_release = Some(now);
                                }
                            }
                        } else if raw_matches && hook.pressed.is_none() {
                            hook.pressed = Some(identity);
                            (hook.callback)(HotkeyEvent::Toggle);
                        }
                    }
                });
            }
        }

        CallNextHookEx(None, code, wparam, lparam)
    }

    unsafe extern "system" fn mouse_hook_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if code >= 0 && matches!(wparam.0 as u32, WM_XBUTTONDOWN | WM_XBUTTONUP) {
            let mouse = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            if let Some(identity) = mouse_side_button_binding(mouse.mouseData) {
                HOOK_STATE.with(|state| {
                    if let Some(ref mut hook) = *state.borrow_mut() {
                        if !hook.is_active.load(Ordering::SeqCst) {
                            return;
                        }
                        if wparam.0 as u32 == WM_XBUTTONDOWN
                            && deliver_capture(&hook.capture_sender, identity)
                        {
                            return;
                        }
                        let config = match hook.config.read() {
                            Ok(config) => config.clone(),
                            Err(_) => return,
                        };
                        let raw_matches = config.binding.eq_ignore_ascii_case("raw")
                            && config.raw_vk_code == identity.vk_code
                            && (config.raw_scan_code == 0
                                || config.raw_scan_code == identity.scan_code)
                            && config.raw_extended == identity.extended;
                        if wparam.0 as u32 == WM_XBUTTONUP {
                            if hook.pressed == Some(identity) {
                                hook.pressed = None;
                            }
                        } else if raw_matches && hook.pressed.is_none() {
                            hook.pressed = Some(identity);
                            (hook.callback)(HotkeyEvent::Toggle);
                        }
                    }
                });
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }

    let keyboard_hook =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), None, 0) };
    match keyboard_hook {
        Ok(keyboard_hook) => {
            let mouse_hook =
                unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), None, 0) }
                    .map_err(|error| {
                        tracing::warn!("Mouse side-button hook is unavailable: {:?}", error);
                    })
                    .ok();
            if mouse_hook.is_some() {
                tracing::info!("Raw keyboard and mouse side-button hooks installed");
            }
            let mut msg = MSG::default();
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    if msg.message == WM_QUIT {
                        break;
                    }
                    DispatchMessageW(&msg);
                }
                if let Some(mouse_hook) = mouse_hook {
                    let _ = UnhookWindowsHookEx(mouse_hook);
                }
                let _ = UnhookWindowsHookEx(keyboard_hook);
            }
        }
        Err(error) => tracing::error!("Failed to install raw keyboard hook: {:?}", error),
    }
}

#[cfg(target_os = "windows")]
fn deliver_capture(
    capture_sender: &Mutex<Option<mpsc::Sender<RawKeyBinding>>>,
    binding: RawKeyBinding,
) -> bool {
    let Ok(mut sender) = capture_sender.lock() else {
        return false;
    };
    let Some(sender) = sender.take() else {
        return false;
    };
    let _ = sender.send(binding);
    true
}

#[cfg(target_os = "windows")]
fn mouse_side_button_binding(mouse_data: u32) -> Option<RawKeyBinding> {
    let vk_code = match (mouse_data >> 16) as u16 {
        1 => 0x05, // VK_XBUTTON1
        2 => 0x06, // VK_XBUTTON2
        _ => return None,
    };
    Some(RawKeyBinding {
        vk_code,
        scan_code: 0,
        extended: false,
    })
}

/// Parse a combo key string like `Ctrl+Shift+V`.
fn parse_combo_key(key_str: &str) -> Result<HotKey> {
    let parts: Vec<&str> = key_str.split('+').map(|s| s.trim()).collect();
    let mut modifiers = Modifiers::empty();
    let mut key_code: Option<Code> = None;

    for part in parts {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= Modifiers::CONTROL,
            "shift" => modifiers |= Modifiers::SHIFT,
            "alt" => modifiers |= Modifiers::ALT,
            "super" | "win" | "meta" => modifiers |= Modifiers::SUPER,
            _ => key_code = Some(parse_key_code(part)?),
        }
    }

    let code = key_code.ok_or_else(|| anyhow!("No key specified in combo: {}", key_str))?;
    Ok(HotKey::new(Some(modifiers), code))
}

/// Parse a standard key name.  Raw vendor keys intentionally do not go
/// through this parser because they have no stable `Code` representation.
fn parse_key_code(key: &str) -> Result<Code> {
    let key = key.to_uppercase();
    let code = match key.as_str() {
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "SPACE" => Code::Space,
        "ENTER" | "RETURN" => Code::Enter,
        "ESCAPE" | "ESC" => Code::Escape,
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        "F13" => Code::F13,
        "F14" => Code::F14,
        "F15" => Code::F15,
        "F16" => Code::F16,
        "F17" => Code::F17,
        "F18" => Code::F18,
        "F19" => Code::F19,
        "F20" => Code::F20,
        "F21" => Code::F21,
        "F22" => Code::F22,
        "F23" => Code::F23,
        "F24" => Code::F24,
        "VOLUMEUP" | "AUDIOVOLUMEUP" => Code::AudioVolumeUp,
        "VOLUMEDOWN" | "AUDIOVOLUMEDOWN" => Code::AudioVolumeDown,
        "VOLUMEMUTE" | "AUDIOVOLUMEMUTE" => Code::AudioVolumeMute,
        "MEDIAPLAY" => Code::MediaPlay,
        "MEDIAPAUSE" => Code::MediaPause,
        "MEDIAPLAYPAUSE" => Code::MediaPlayPause,
        "MEDIASTOP" => Code::MediaStop,
        "MEDIANEXT" | "MEDIATRACKNEXT" => Code::MediaTrackNext,
        "MEDIAPREV" | "MEDIATRACKPREV" => Code::MediaTrackPrevious,
        _ => return Err(anyhow!("Unknown key: {}", key)),
    };
    Ok(code)
}
