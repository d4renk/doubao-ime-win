//! Native user settings window.

use crate::business::HotkeyManager;
use crate::data::{AppConfig, AudioQuality, PunctuationMode};

/// Open the user settings window.
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
    use windows::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const ID_COMBO: usize = 101;
    const ID_CAPTURE: usize = 102;
    const ID_STANDARD: usize = 103;
    const ID_RAW: usize = 104;
    const ID_TRIGGER: usize = 105;
    const ID_SAVE: usize = 106;
    const ID_CANCEL: usize = 107;
    const ID_AUTO_START: usize = 108;
    const ID_FLOATING: usize = 109;
    const ID_VAD: usize = 110;
    const ID_AUDIO_QUALITY: usize = 111;
    const ID_PUNCTUATION: usize = 112;
    const ID_NER_ENABLED: usize = 113;
    const ID_AUTO_POLISH_ENABLED: usize = 114;
    const ID_TIMER: usize = 1;

    struct DialogState {
        config: AppConfig,
        manager: HotkeyManager,
        hwnd: HWND,
        combo_edit: HWND,
        status_label: HWND,
        source_label: HWND,
        trigger_button: HWND,
        auto_start_check: HWND,
        floating_check: HWND,
        vad_check: HWND,
        audio_quality_combo: HWND,
        punctuation_combo: HWND,
        ner_enabled_check: HWND,
        auto_polish_enabled_check: HWND,
        capture_rx: Option<Receiver<anyhow::Result<crate::business::RawKeyBinding>>>,
    }

    thread_local! {
        static STATE: RefCell<Option<DialogState>> = const { RefCell::new(None) };
    }

    pub fn show(config: AppConfig, manager: HotkeyManager) {
        let existing_window = STATE.with(|state| {
            state
                .borrow()
                .as_ref()
                .map(|state| state.hwnd)
                .filter(|hwnd| hwnd.0 != 0)
        });

        if let Some(hwnd) = existing_window {
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            return;
        }

        STATE.with(|state| {
            *state.borrow_mut() = Some(DialogState {
                config,
                manager,
                hwnd: HWND::default(),
                combo_edit: HWND::default(),
                status_label: HWND::default(),
                source_label: HWND::default(),
                trigger_button: HWND::default(),
                auto_start_check: HWND::default(),
                floating_check: HWND::default(),
                vad_check: HWND::default(),
                audio_quality_combo: HWND::default(),
                punctuation_combo: HWND::default(),
                ner_enabled_check: HWND::default(),
                auto_polish_enabled_check: HWND::default(),
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
                w!("豆包语音输入 - 用户设置 / VoiceUtility Settings"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                760,
                580,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            );
            if hwnd.0 == 0 {
                tracing::error!("Failed to create the settings window");
                STATE.with(|state| *state.borrow_mut() = None);
                return;
            }

            STATE.with(|state| {
                if let Some(state) = state.borrow_mut().as_mut() {
                    state.hwnd = hwnd;
                }
            });
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    #[allow(clippy::too_many_arguments)]
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

    unsafe fn set_checked(hwnd: HWND, checked: bool) {
        let value = if checked {
            BST_CHECKED.0 as usize
        } else {
            BST_UNCHECKED.0 as usize
        };
        let _ = SendMessageW(hwnd, BM_SETCHECK, WPARAM(value), LPARAM(0));
    }

    unsafe fn is_checked(hwnd: HWND) -> bool {
        SendMessageW(hwnd, BM_GETCHECK, WPARAM(0), LPARAM(0)).0 == BST_CHECKED.0 as isize
    }

    unsafe fn add_combo_item(hwnd: HWND, value: &str) {
        let value = wide(value);
        let _ = SendMessageW(
            hwnd,
            CB_ADDSTRING,
            WPARAM(0),
            LPARAM(value.as_ptr() as isize),
        );
    }

    unsafe fn set_combo_selection(hwnd: HWND, index: usize) {
        let _ = SendMessageW(hwnd, CB_SETCURSEL, WPARAM(index), LPARAM(0));
    }

    unsafe fn combo_selection(hwnd: HWND) -> usize {
        SendMessageW(hwnd, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize
    }

    fn audio_quality_index(quality: AudioQuality) -> usize {
        match quality {
            AudioQuality::Standard => 0,
            AudioQuality::HighQuality => 1,
        }
    }

    fn audio_quality_from_index(index: usize) -> AudioQuality {
        match index {
            1 => AudioQuality::HighQuality,
            _ => AudioQuality::Standard,
        }
    }

    fn punctuation_index(mode: PunctuationMode) -> usize {
        match mode {
            PunctuationMode::Smart => 0,
            PunctuationMode::Spaces => 1,
            PunctuationMode::NoSentenceFinal => 2,
            PunctuationMode::Preserve => 3,
        }
    }

    fn punctuation_from_index(index: usize) -> PunctuationMode {
        match index {
            1 => PunctuationMode::Spaces,
            2 => PunctuationMode::NoSentenceFinal,
            3 => PunctuationMode::Preserve,
            _ => PunctuationMode::Smart,
        }
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
                    // Creating child controls can synchronously re-enter this
                    // window procedure through WM_COMMAND notifications.
                    let Ok(mut state) = state.try_borrow_mut() else {
                        return;
                    };
                    let Some(state) = state.as_mut() else { return };

                    create_control(
                        w!("STATIC"),
                        w!("设置 / Settings"),
                        WS_CHILD | WS_VISIBLE,
                        20,
                        15,
                        500,
                        28,
                        hwnd,
                        0,
                        instance,
                    );
                    create_control(
                        w!("STATIC"),
                        w!("常规设置 / General"),
                        WS_CHILD | WS_VISIBLE,
                        20,
                        55,
                        500,
                        24,
                        hwnd,
                        0,
                        instance,
                    );
                    state.auto_start_check = create_control(
                        w!("BUTTON"),
                        w!("开机自启 / Auto-start"),
                        WINDOW_STYLE(
                            WS_CHILD.0
                                | WS_VISIBLE.0
                                | WS_TABSTOP.0
                                | WS_GROUP.0
                                | BS_AUTOCHECKBOX as u32,
                        ),
                        20,
                        82,
                        220,
                        28,
                        hwnd,
                        ID_AUTO_START,
                        instance,
                    );
                    set_checked(state.auto_start_check, state.config.general.auto_start);
                    state.floating_check = create_control(
                        w!("BUTTON"),
                        w!("显示悬浮按钮 / Floating"),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
                        ),
                        260,
                        82,
                        240,
                        28,
                        hwnd,
                        ID_FLOATING,
                        instance,
                    );
                    set_checked(state.floating_check, state.config.floating_button.enabled);
                    state.vad_check = create_control(
                        w!("BUTTON"),
                        w!("VAD 静音检测 / Voice Activity"),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
                        ),
                        520,
                        82,
                        220,
                        28,
                        hwnd,
                        ID_VAD,
                        instance,
                    );
                    set_checked(state.vad_check, state.config.asr.vad_enabled);
                    create_control(
                        w!("STATIC"),
                        w!("热键配置 / Hotkey Configuration"),
                        WS_CHILD | WS_VISIBLE,
                        20,
                        125,
                        500,
                        24,
                        hwnd,
                        0,
                        instance,
                    );
                    create_control(
                        w!("STATIC"),
                        w!("标准组合键："),
                        WS_CHILD | WS_VISIBLE,
                        20,
                        158,
                        110,
                        24,
                        hwnd,
                        0,
                        instance,
                    );
                    state.combo_edit = create_control(
                        w!("EDIT"),
                        PCWSTR(wide(&state.config.hotkey.combo_key).as_ptr()),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32,
                        ),
                        135,
                        154,
                        180,
                        28,
                        hwnd,
                        ID_COMBO,
                        instance,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("录入非标准按键"),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                        ),
                        385,
                        154,
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
                        200,
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
                        230,
                        455,
                        42,
                        hwnd,
                        0,
                        instance,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("使用标准快捷键"),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                        ),
                        20,
                        290,
                        145,
                        30,
                        hwnd,
                        ID_STANDARD,
                        instance,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("使用非标准按键"),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                        ),
                        190,
                        290,
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
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                        ),
                        360,
                        290,
                        145,
                        30,
                        hwnd,
                        ID_TRIGGER,
                        instance,
                    );
                    create_control(
                        w!("STATIC"),
                        w!("语音识别 / Speech Recognition"),
                        WS_CHILD | WS_VISIBLE,
                        20,
                        345,
                        500,
                        24,
                        hwnd,
                        0,
                        instance,
                    );
                    create_control(
                        w!("STATIC"),
                        w!("采集音质："),
                        WS_CHILD | WS_VISIBLE,
                        20,
                        380,
                        110,
                        24,
                        hwnd,
                        0,
                        instance,
                    );
                    state.audio_quality_combo = create_control(
                        w!("COMBOBOX"),
                        w!(""),
                        WINDOW_STYLE(
                            WS_CHILD.0
                                | WS_VISIBLE.0
                                | WS_TABSTOP.0
                                | WS_VSCROLL.0
                                | CBS_DROPDOWNLIST as u32,
                        ),
                        135,
                        375,
                        300,
                        140,
                        hwnd,
                        ID_AUDIO_QUALITY,
                        instance,
                    );
                    add_combo_item(state.audio_quality_combo, "标准识别（16 kHz 单声道）");
                    add_combo_item(state.audio_quality_combo, "高清识别（24 kHz 单声道）");
                    set_combo_selection(
                        state.audio_quality_combo,
                        audio_quality_index(state.config.asr.audio_quality),
                    );
                    create_control(
                        w!("STATIC"),
                        w!("标点展示："),
                        WS_CHILD | WS_VISIBLE,
                        20,
                        425,
                        110,
                        24,
                        hwnd,
                        0,
                        instance,
                    );
                    state.punctuation_combo = create_control(
                        w!("COMBOBOX"),
                        w!(""),
                        WINDOW_STYLE(
                            WS_CHILD.0
                                | WS_VISIBLE.0
                                | WS_TABSTOP.0
                                | WS_VSCROLL.0
                                | CBS_DROPDOWNLIST as u32,
                        ),
                        135,
                        420,
                        300,
                        180,
                        hwnd,
                        ID_PUNCTUATION,
                        instance,
                    );
                    add_combo_item(state.punctuation_combo, "智能添加标点");
                    add_combo_item(state.punctuation_combo, "空格代替标点");
                    add_combo_item(state.punctuation_combo, "句末不添加标点");
                    add_combo_item(state.punctuation_combo, "保留所有标点");
                    set_combo_selection(
                        state.punctuation_combo,
                        punctuation_index(state.config.asr.punctuation_mode),
                    );
                    create_control(
                        w!("STATIC"),
                        w!("云端增强 / Cloud Enhancement"),
                        WS_CHILD | WS_VISIBLE,
                        465,
                        345,
                        270,
                        24,
                        hwnd,
                        0,
                        instance,
                    );
                    state.ner_enabled_check = create_control(
                        w!("BUTTON"),
                        w!("实体识别 / NER"),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
                        ),
                        465,
                        375,
                        260,
                        28,
                        hwnd,
                        ID_NER_ENABLED,
                        instance,
                    );
                    set_checked(state.ner_enabled_check, state.config.cloud.ner_enabled);
                    state.auto_polish_enabled_check = create_control(
                        w!("BUTTON"),
                        w!("3 秒去口水词 / Filler cleanup"),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32,
                        ),
                        465,
                        420,
                        260,
                        28,
                        hwnd,
                        ID_AUTO_POLISH_ENABLED,
                        instance,
                    );
                    set_checked(
                        state.auto_polish_enabled_check,
                        state.config.cloud.auto_polish_enabled,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("保存"),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
                        ),
                        550,
                        490,
                        80,
                        30,
                        hwnd,
                        ID_SAVE,
                        instance,
                    );
                    create_control(
                        w!("BUTTON"),
                        w!("取消"),
                        WINDOW_STYLE(
                            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                        ),
                        645,
                        490,
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
                let command = wparam.0 & 0xffff;
                STATE.with(|state| {
                    let Ok(mut state) = state.try_borrow_mut() else {
                        return;
                    };
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
                            let previous_config = state.config.clone();
                            let previous_hotkey = previous_config.hotkey.clone();
                            state.config.general.auto_start = is_checked(state.auto_start_check);
                            state.config.floating_button.enabled = is_checked(state.floating_check);
                            state.config.asr.vad_enabled = is_checked(state.vad_check);
                            state.config.asr.audio_quality = audio_quality_from_index(
                                combo_selection(state.audio_quality_combo),
                            );
                            state.config.asr.punctuation_mode =
                                punctuation_from_index(combo_selection(state.punctuation_combo));
                            state.config.cloud.ner_enabled = is_checked(state.ner_enabled_check);
                            state.config.cloud.auto_polish_enabled =
                                is_checked(state.auto_polish_enabled_check);
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
                                state.config = previous_config;
                                let message = wide(&format!("保存配置失败：{}", error));
                                let _ = MessageBoxW(
                                    hwnd,
                                    PCWSTR(message.as_ptr()),
                                    w!("保存失败"),
                                    MB_OK | MB_ICONERROR,
                                );
                            } else {
                                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                            }
                        }
                        ID_CANCEL => {
                            let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                        }
                        _ => {}
                    }
                });
                LRESULT(0)
            }
            WM_TIMER => {
                STATE.with(|state| {
                    let Ok(mut state) = state.try_borrow_mut() else {
                        return;
                    };
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
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                STATE.with(|state| *state.borrow_mut() = None);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, message, wparam, _lparam),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{
            audio_quality_from_index, audio_quality_index, punctuation_from_index,
            punctuation_index,
        };
        use crate::data::{AudioQuality, PunctuationMode};

        #[test]
        fn audio_quality_combo_mapping_round_trips() {
            for quality in [AudioQuality::Standard, AudioQuality::HighQuality] {
                assert_eq!(
                    audio_quality_from_index(audio_quality_index(quality)),
                    quality
                );
            }
        }

        #[test]
        fn punctuation_combo_mapping_round_trips() {
            for mode in [
                PunctuationMode::Smart,
                PunctuationMode::Spaces,
                PunctuationMode::NoSentenceFinal,
                PunctuationMode::Preserve,
            ] {
                assert_eq!(punctuation_from_index(punctuation_index(mode)), mode);
            }
        }
    }
}
