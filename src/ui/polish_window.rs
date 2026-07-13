//! Completed polish result confirmation window.

use std::sync::Arc;

use crate::business::{PolishPresentation, TextInserter, VoiceSessionStore};

pub fn show_polish_result(
    presentation: PolishPresentation,
    sessions: Arc<VoiceSessionStore>,
    text_inserter: Arc<TextInserter>,
) {
    #[cfg(target_os = "windows")]
    windows_polish::show(presentation, sessions, text_inserter);

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (presentation, sessions, text_inserter);
        tracing::info!("Polish confirmation is only available on Windows");
    }
}

#[cfg(target_os = "windows")]
mod windows_polish {
    use super::*;
    use std::cell::RefCell;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::*;

    const ID_REPLACE: usize = 201;
    const ID_KEEP: usize = 202;
    const ID_TIMER: usize = 1;

    struct WindowState {
        hwnd: HWND,
        presentation: PolishPresentation,
        sessions: Arc<VoiceSessionStore>,
        text_inserter: Arc<TextInserter>,
    }

    thread_local! {
        static STATE: RefCell<Option<WindowState>> = const { RefCell::new(None) };
    }

    pub fn show(
        presentation: PolishPresentation,
        sessions: Arc<VoiceSessionStore>,
        text_inserter: Arc<TextInserter>,
    ) {
        let old_window = STATE.with(|state| state.borrow().as_ref().map(|value| value.hwnd));
        if let Some(hwnd) = old_window.filter(|hwnd| hwnd.0 != 0) {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }

        STATE.with(|state| {
            *state.borrow_mut() = Some(WindowState {
                hwnd: HWND::default(),
                presentation,
                sessions,
                text_inserter,
            });
        });

        unsafe {
            let Ok(instance) = GetModuleHandleW(None) else {
                STATE.with(|state| *state.borrow_mut() = None);
                return;
            };
            let class_name = w!("DoubaoPolishResult");
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: class_name,
                ..Default::default()
            };
            RegisterClassExW(&class);

            let hwnd = CreateWindowExW(
                WS_EX_DLGMODALFRAME | WS_EX_TOPMOST,
                class_name,
                w!("豆包语音 - 润色结果"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                720,
                540,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            );
            if hwnd.0 == 0 {
                STATE.with(|state| *state.borrow_mut() = None);
                tracing::error!("Failed to create the polish result window");
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

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
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
    ) -> HWND {
        let Ok(instance) = GetModuleHandleW(None) else {
            return HWND::default();
        };
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

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match message {
            WM_CREATE => {
                let (original, rewritten) = STATE.with(|state| {
                    state
                        .borrow()
                        .as_ref()
                        .map(|state| {
                            (
                                wide(&state.presentation.session.text),
                                wide(&state.presentation.rewritten_text),
                            )
                        })
                        .unwrap_or_else(|| (wide(""), wide("")))
                });
                create_control(
                    w!("STATIC"),
                    w!("原始语音文本"),
                    WS_CHILD | WS_VISIBLE,
                    18,
                    14,
                    650,
                    24,
                    hwnd,
                    0,
                );
                create_control(
                    w!("EDIT"),
                    PCWSTR(original.as_ptr()),
                    WINDOW_STYLE(
                        WS_CHILD.0
                            | WS_VISIBLE.0
                            | WS_VSCROLL.0
                            | ES_MULTILINE as u32
                            | ES_AUTOVSCROLL as u32
                            | ES_READONLY as u32,
                    ),
                    18,
                    42,
                    666,
                    145,
                    hwnd,
                    0,
                );
                create_control(
                    w!("STATIC"),
                    w!("润色结果"),
                    WS_CHILD | WS_VISIBLE,
                    18,
                    202,
                    650,
                    24,
                    hwnd,
                    0,
                );
                create_control(
                    w!("EDIT"),
                    PCWSTR(rewritten.as_ptr()),
                    WINDOW_STYLE(
                        WS_CHILD.0
                            | WS_VISIBLE.0
                            | WS_VSCROLL.0
                            | ES_MULTILINE as u32
                            | ES_AUTOVSCROLL as u32
                            | ES_READONLY as u32,
                    ),
                    18,
                    230,
                    666,
                    205,
                    hwnd,
                    0,
                );
                create_control(
                    w!("BUTTON"),
                    w!("替换"),
                    WINDOW_STYLE(
                        WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
                    ),
                    490,
                    462,
                    90,
                    32,
                    hwnd,
                    ID_REPLACE,
                );
                create_control(
                    w!("BUTTON"),
                    w!("保留原文"),
                    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32),
                    594,
                    462,
                    90,
                    32,
                    hwnd,
                    ID_KEEP,
                );
                let _ = SetTimer(hwnd, ID_TIMER, 250, None);
                LRESULT(0)
            }
            WM_COMMAND => {
                match wparam.0 & 0xffff {
                    ID_REPLACE => {
                        let result = STATE.with(|state| {
                            let state = state.borrow();
                            let Some(state) = state.as_ref() else {
                                return Ok(());
                            };
                            if !state
                                .sessions
                                .is_current(state.presentation.session.generation)
                            {
                                anyhow::bail!("该语音结果已失效");
                            }
                            state.text_inserter.replace_recent(
                                state.presentation.session.target_window,
                                state.presentation.session.inserted_chars,
                                &state.presentation.rewritten_text,
                            )
                        });
                        match result {
                            Ok(()) => {
                                let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                            }
                            Err(error) => {
                                let message = wide(&format!("无法替换目标文本：{error}"));
                                let _ = MessageBoxW(
                                    hwnd,
                                    PCWSTR(message.as_ptr()),
                                    w!("替换失败"),
                                    MB_OK | MB_ICONERROR,
                                );
                            }
                        }
                    }
                    ID_KEEP => {
                        let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_TIMER => {
                let current = STATE.with(|state| {
                    state
                        .borrow()
                        .as_ref()
                        .map(|state| {
                            state
                                .sessions
                                .is_current(state.presentation.session.generation)
                        })
                        .unwrap_or(false)
                });
                if !current {
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
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
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}
