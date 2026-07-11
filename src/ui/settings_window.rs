//! Lightweight native settings window.

use crate::business::HotkeyManager;
use crate::data::AppConfig;

/// Open the hotkey settings window.
pub fn show_settings(config: AppConfig, manager: HotkeyManager) {
    #[cfg(target_os = "windows")]
    windows_settings::show(config, manager);

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (config, manager);
        tracing::info!("Settings window is only available on Windows");
    }
}

#[cfg(target_os = "windows")]
mod windows_settings {
    use super::*;
    use std::cell::RefCell;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Duration;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const ID_COMBO: usize = 101;
    const ID_CAPTURE: usize = 102;
    const ID_STANDARD: usize = 103;
    const ID_RAW: usize = 104;
    const ID_TRIGGER: usize = 105;
    const ID_SAVE: usize = 106;
    const ID_CANCEL: usize = 107;
    const ID_TIMER: usize = 1;

    struct DialogState {
        config: AppConfig,
        manager: HotkeyManager,
        combo_edit: HWND,
        status_label: HWND,
        source_label: HWND,
        trigger_button: HWND,
        capture_rx: Option<Receiver<anyhow::Result<crate::business::RawKeyBinding>>>,
    }

    thread_local! {
        static STATE: RefCell<Option<DialogState>> = const { RefCell::new(None) };
    }

    pub fn show(config: AppConfig, manager: HotkeyManager) {
        STATE.with(|state| {
            *state.borrow_mut() = Some(DialogState {
                config,
                manager,
                combo_edit: HWND::default(),
                status_label: HWND::default(),
                source_label: HWND::default(),
                trigger_button: HWND::default(),
                capture_rx: None,
            });
        });

        unsafe {
            let Ok(instance) = GetModuleHandleW(None) else {
                return;
            };
            let class_name = w!("DoubaoHotkeySettings");
            let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                hCursor: cursor,
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassExW(&class);

            let hwnd = CreateWindowExW(
                WS_EX_DLGMODALFRAME,
                class_name,
                w!("豆包语音输入 - 快捷键设置"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                520,
                300,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            );
            if hwnd.0 == 0 {
                STATE.with(|state| *state.borrow_mut() = None);
                return;
            }

            let mut message = MSG::default();
            while GetMessageW(&mut message, HWND::default(), 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }

        STATE.with(|state| *state.borrow_mut() = None);
    }

    unsafe fn create_control(
        class: PCWSTR,
        text: PCWSTR,
        style: WINDOW_STYLE,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: HWND,
        id: usize,
        instance: HMODULE,
    ) -> HWND {
        CreateWindowExW(
            if class == w!("EDIT") {
                WS_EX_CLIENTEDGE
            } else {
                WINDOW_EX_STYLE(0)
            },
            class,
            text,
            style,
            x,
            y,
            width,
            height,
            parent,
            HMENU(id as isize),
            instance,
            None,
        )
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn set_text(hwnd: HWND, value: &str) {
        let text = wide(value);
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
    }

    unsafe fn get_text(hwnd: HWND) -> String {
        let mut buffer = [0u16; 256];
        let length = GetWindowTextW(hwnd, &mut buffer) as usize;
        String::from_utf16_lossy(&buffer[..length])
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CREATE => {
                let Ok(instance) = GetModuleHandleW(None) else {
                    return LRESULT(0);
                };

                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    let Some(state) = state.as_mut() else { return };

                    create_control(
                        w!("STATIC"),
                        w!("标准组合键："),
                        WS_CHILD | WS_VISIBLE,
                        20,
                        22,
                        110,
                        24,
                        hwnd,
                        0,
                        instance,
                    );
                    state.combo_edit = create_control(
                        w!("EDIT"),
                        PCWSTR(wide(&state.config.hotkey.combo_key).as_ptr()),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL,
                        135,
                        18,
                        180,
                        28,
                        hwnd,
                        ID_COMBO,
                        instance,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("录入非标准按键"),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                        325,
                        18,
                        150,
                        28,
                        hwnd,
                        ID_CAPTURE,
                        instance,
                    );
                    state.source_label = create_control(
                        w!("STATIC"),
                        if state.config.hotkey.binding.eq_ignore_ascii_case("raw") {
                            w!("当前绑定：非标准原始按键")
                        } else {
                            w!("当前绑定：标准快捷键")
                        },
                        WS_CHILD | WS_VISIBLE,
                        20,
                        65,
                        300,
                        24,
                        hwnd,
                        0,
                        instance,
                    );
                    state.status_label = create_control(
                        w!("STATIC"),
                        if state.config.hotkey.raw_vk_code == 0 {
                            w!("尚未录入非标准按键")
                        } else {
                            PCWSTR(
                                wide(&format!(
                                    "已录入：VK=0x{:X}, Scan=0x{:X}, Extended={}",
                                    state.config.hotkey.raw_vk_code,
                                    state.config.hotkey.raw_scan_code,
                                    state.config.hotkey.raw_extended
                                ))
                                .as_ptr(),
                            )
                        },
                        WS_CHILD | WS_VISIBLE,
                        20,
                        95,
                        455,
                        42,
                        hwnd,
                        0,
                        instance,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("使用标准快捷键"),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                        20,
                        155,
                        145,
                        30,
                        hwnd,
                        ID_STANDARD,
                        instance,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("使用非标准按键"),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                        175,
                        155,
                        145,
                        30,
                        hwnd,
                        ID_RAW,
                        instance,
                    );
                    state.trigger_button = create_control(
                        w!("BUTTON"),
                        if state.config.hotkey.raw_trigger.eq_ignore_ascii_case("hold") {
                            w!("触发模式：按住说话")
                        } else {
                            w!("触发模式：按下切换")
                        },
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                        330,
                        155,
                        145,
                        30,
                        hwnd,
                        ID_TRIGGER,
                        instance,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("保存"),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON,
                        300,
                        215,
                        80,
                        30,
                        hwnd,
                        ID_SAVE,
                        instance,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("取消"),
                        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON,
                        395,
                        215,
                        80,
                        30,
                        hwnd,
                        ID_CANCEL,
                        instance,
                    );
                    let _ = SetTimer(hwnd, ID_TIMER, 100, None);
                });
                LRESULT(0)
            }
            WM_COMMAND => {
                let command = (wparam.0 & 0xffff) as usize;
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    let Some(state) = state.as_mut() else { return };
                    match command {
                        ID_CAPTURE => {
                            set_text(state.status_label, "请在 10 秒内按下要绑定的按键...");
                            let (sender, receiver) = mpsc::channel();
                            state.capture_rx = Some(receiver);
                            thread::spawn(move || {
                                let result =
                                    HotkeyManager::capture_raw_key(Duration::from_secs(10));
                                let _ = sender.send(result);
                            });
                        }
                        ID_STANDARD => {
                            state.config.hotkey.binding = "standard".to_string();
                            set_text(state.source_label, "当前绑定：标准快捷键");
                        }
                        ID_RAW => {
                            state.config.hotkey.binding = "raw".to_string();
                            set_text(state.source_label, "当前绑定：非标准原始按键");
                        }
                        ID_TRIGGER => {
                            if state.config.hotkey.raw_trigger.eq_ignore_ascii_case("hold") {
                                state.config.hotkey.raw_trigger = "toggle".to_string();
                                set_text(state.trigger_button, "触发模式：按下切换");
                            } else {
                                state.config.hotkey.raw_trigger = "hold".to_string();
                                set_text(state.trigger_button, "触发模式：按住说话");
                            }
                        }
                        ID_SAVE => {
                            let previous_hotkey = state.config.hotkey.clone();
                            state.config.hotkey.combo_key = get_text(state.combo_edit);
                            if let Err(error) = state.manager.reconfigure(&state.config.hotkey) {
                                let message = wide(&format!("快捷键设置无效：{}", error));
                                let _ = MessageBoxW(
                                    hwnd,
                                    PCWSTR(message.as_ptr()),
                                    w!("设置失败"),
                                    MB_OK | MB_ICONERROR,
                                );
                            } else if let Err(error) = state.config.save() {
                                let _ = state.manager.reconfigure(&previous_hotkey);
                                state.config.hotkey = previous_hotkey;
                                let message = wide(&format!("保存配置失败：{}", error));
                                let _ = MessageBoxW(
                                    hwnd,
                                    PCWSTR(message.as_ptr()),
                                    w!("保存失败"),
                                    MB_OK | MB_ICONERROR,
                                );
                            } else {
                                PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                            }
                        }
                        ID_CANCEL => PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0)),
                        _ => {}
                    }
                });
                LRESULT(0)
            }
            WM_TIMER => {
                STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    let Some(state) = state.as_mut() else { return };
                    if let Some(receiver) = state.capture_rx.as_ref() {
                        match receiver.try_recv() {
                            Ok(Ok(binding)) => {
                                state.config.hotkey.raw_vk_code = binding.vk_code;
                                state.config.hotkey.raw_scan_code = binding.scan_code;
                                state.config.hotkey.raw_extended = binding.extended;
                                set_text(
                                    state.status_label,
                                    &format!(
                                        "已录入：VK=0x{:X}, Scan=0x{:X}, Extended={}",
                                        binding.vk_code, binding.scan_code, binding.extended
                                    ),
                                );
                                state.capture_rx = None;
                            }
                            Ok(Err(error)) => {
                                set_text(state.status_label, &format!("录入失败：{}", error));
                                state.capture_rx = None;
                            }
                            Err(mpsc::TryRecvError::Empty) => {}
                            Err(mpsc::TryRecvError::Disconnected) => state.capture_rx = None,
                        }
                    }
                });
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = KillTimer(hwnd, ID_TIMER);
                DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, _lparam),
        }
    }
}
