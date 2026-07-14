//! Tao/Wry desktop shell for the tray, settings window and recording HUD.

use anyhow::{anyhow, Result};
use rust_embed::RustEmbed;
use serde::Serialize;
use std::{
    borrow::Cow,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy},
    platform::windows::{WindowBuilderExtWindows, WindowExtWindows},
    window::{Window, WindowBuilder},
};
use tokio::sync::Mutex;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIconBuilder,
};
use wry::{http::Response, PageLoadEvent, WebContext, WebView, WebViewBuilder};

use crate::{
    audio::AudioCapture,
    business::{HotkeyEvent, HotkeyManager, RawKeyBinding, VoiceController},
    data::AppConfig,
};

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct FrontendAssets;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum VoiceState {
    Idle,
    Recording,
    Processing,
}

enum UserEvent {
    Ipc(IpcCommand),
    Hotkey(HotkeyEvent),
    Start,
    Stop,
    SetState(VoiceState),
    CaptureResult(std::result::Result<RawKeyBinding, String>),
    DragHud,
}

enum IpcCommand {
    GetConfig,
    SaveConfig(AppConfig),
    CaptureRawKey,
    ShowSettings,
    GetVoiceState,
    StartRecording,
    StopRecording,
    DragHud,
}

/// Runs the UI shell on the main thread. Audio and network work remain on Tokio.
pub fn run_app(
    config: AppConfig,
    voice_controller: Arc<Mutex<VoiceController>>,
    hotkey_manager: HotkeyManager,
    audio_capture: Arc<AudioCapture>,
) -> Result<()> {
    let event_loop: EventLoop<UserEvent> = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let settings_window = WindowBuilder::new()
        .with_title("豆包语音输入 - 设置")
        .with_inner_size(LogicalSize::new(1120.0, 820.0))
        .with_min_inner_size(LogicalSize::new(760.0, 640.0))
        .build(&event_loop)?;
    let hud_window = WindowBuilder::new()
        .with_title("豆包语音输入")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top(true)
        .with_skip_taskbar(true)
        .with_inner_size(LogicalSize::new(360.0, 156.0))
        .with_position(tao::dpi::LogicalPosition::new(
            config.floating_button.position_x as f64,
            config.floating_button.position_y as f64,
        ))
        .build(&event_loop)?;
    settings_window.set_visible(false);
    hud_window.set_visible(false);
    set_hud_no_activate(&hud_window);

    let webview_data_dir = webview_data_directory();
    std::fs::create_dir_all(&webview_data_dir).map_err(|error| {
        anyhow!(
            "Unable to create WebView2 data directory {}: {error}",
            webview_data_dir.display()
        )
    })?;
    tracing::info!(
        "Using WebView2 data directory: {}",
        webview_data_dir.display()
    );
    let mut web_context = WebContext::new(Some(webview_data_dir));
    let settings_webview = build_webview(&settings_window, false, proxy.clone(), &mut web_context)?;
    let hud_webview = build_webview(&hud_window, true, proxy.clone(), &mut web_context)?;

    let menu = Menu::new();
    let start_item = MenuItem::new("开始语音输入", true, None);
    let stop_item = MenuItem::new("停止语音输入", true, None);
    let settings_item = MenuItem::new("设置...", true, None);
    let quit_item = MenuItem::new("退出", true, None);
    let start_id = start_item.id().clone();
    let stop_id = stop_item.id().clone();
    let settings_id = settings_item.id().clone();
    let quit_id = quit_item.id().clone();
    menu.append(&start_item)?;
    menu.append(&stop_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&settings_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("豆包语音输入 - 双击 Ctrl 开始/停止")
        .with_icon(load_icon()?)
        .build()?;

    let hotkey_proxy = proxy.clone();
    hotkey_manager.on_event(move |event| {
        let _ = hotkey_proxy.send_event(UserEvent::Hotkey(event));
    });

    let settings_id_window = settings_window.id();
    let hud_id_window = hud_window.id();
    let mut state = VoiceState::Idle;
    let mut last_meter_at = Instant::now();
    let runtime = tokio::runtime::Handle::current();
    let menu_rx = MenuEvent::receiver();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(16));
        match event {
            Event::UserEvent(command) => match command {
                UserEvent::Ipc(command) => handle_ipc(command, &settings_webview, &hud_webview, &settings_window, &proxy, &hotkey_manager, &runtime, voice_controller.clone(), &mut state, &hud_window),
                UserEvent::Hotkey(HotkeyEvent::Toggle) => {
                    if matches!(state, VoiceState::Recording) { let _ = proxy.send_event(UserEvent::Stop); } else { let _ = proxy.send_event(UserEvent::Start); }
                }
                UserEvent::Hotkey(HotkeyEvent::Press) => { let _ = proxy.send_event(UserEvent::Start); }
                UserEvent::Hotkey(HotkeyEvent::Release) => { let _ = proxy.send_event(UserEvent::Stop); }
                UserEvent::Start => start_recording(voice_controller.clone(), runtime.clone(), proxy.clone()),
                UserEvent::Stop => stop_recording(voice_controller.clone(), runtime.clone(), proxy.clone()),
                UserEvent::SetState(next) => {
                    state = next;
                    send_event(&hud_webview, &serde_json::json!({"type":"voice_state", "state": state}));
                    let hud_enabled = AppConfig::load_or_default().map(|config| config.floating_button.enabled).unwrap_or(false);
                    hud_window.set_visible(!matches!(state, VoiceState::Idle) && hud_enabled);
                }
                UserEvent::CaptureResult(result) => match result { Ok(binding) => send_event(&settings_webview, &serde_json::json!({"type":"capture_result", "message":format!("已录入：VK {} / 扫描码 {}", binding.vk_code, binding.scan_code), "binding":{"vk_code":binding.vk_code,"scan_code":binding.scan_code,"extended":binding.extended}})), Err(error) => send_event(&settings_webview, &serde_json::json!({"type":"capture_result", "message":format!("录入失败：{error}")})) },
                UserEvent::DragHud => { let _ = hud_window.drag_window(); }
            },
            Event::MainEventsCleared => {
                while let Ok(menu_event) = menu_rx.try_recv() {
                    if menu_event.id == start_id { let _ = proxy.send_event(UserEvent::Start); }
                    else if menu_event.id == stop_id { let _ = proxy.send_event(UserEvent::Stop); }
                    else if menu_event.id == settings_id { settings_window.set_visible(true); }
                    else if menu_event.id == quit_id { *control_flow = ControlFlow::Exit; }
                }
                if matches!(state, VoiceState::Recording) && last_meter_at.elapsed() >= Duration::from_millis(33) {
                    last_meter_at = Instant::now();
                    send_event(&hud_webview, &serde_json::json!({"type":"meter", "value":audio_capture.meter_level()}));
                }
            }
            Event::WindowEvent { window_id, event: WindowEvent::Moved(position), .. } if window_id == hud_id_window => {
                if let Ok(mut current) = AppConfig::load_or_default() { current.floating_button.position_x = position.x; current.floating_button.position_y = position.y; let _ = current.save(); }
            }
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } if window_id == settings_id_window => settings_window.set_visible(false),
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } if window_id == hud_id_window => { let _ = proxy.send_event(UserEvent::Stop); },
            _ => {}
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn handle_ipc(
    command: IpcCommand,
    settings: &WebView,
    hud: &WebView,
    settings_window: &Window,
    proxy: &EventLoopProxy<UserEvent>,
    hotkeys: &HotkeyManager,
    runtime: &tokio::runtime::Handle,
    controller: Arc<Mutex<VoiceController>>,
    state: &mut VoiceState,
    hud_window: &Window,
) {
    match command {
        IpcCommand::GetConfig => match AppConfig::load_or_default() {
            Ok(config) => send_event(
                settings,
                &serde_json::json!({"type":"config", "config":config}),
            ),
            Err(error) => send_error(settings, error),
        },
        IpcCommand::SaveConfig(config) => {
            if let Err(error) = hotkeys
                .reconfigure(&config.hotkey)
                .and_then(|_| config.save())
            {
                send_error(settings, error);
            } else {
                send_event(
                    settings,
                    &serde_json::json!({"type":"config", "config":config}),
                );
            }
        }
        IpcCommand::CaptureRawKey => {
            let proxy = proxy.clone();
            std::thread::spawn(move || {
                let result = HotkeyManager::capture_raw_key(Duration::from_secs(10))
                    .map_err(|error| error.to_string());
                let _ = proxy.send_event(UserEvent::CaptureResult(result));
            });
        }
        IpcCommand::ShowSettings => {
            settings_window.set_visible(true);
        }
        IpcCommand::GetVoiceState => send_event(
            hud,
            &serde_json::json!({"type":"voice_state", "state":state}),
        ),
        IpcCommand::StartRecording => start_recording(controller, runtime.clone(), proxy.clone()),
        IpcCommand::StopRecording => stop_recording(controller, runtime.clone(), proxy.clone()),
        IpcCommand::DragHud => {
            let _ = proxy.send_event(UserEvent::DragHud);
        }
    }
    let _ = hud_window;
}

fn start_recording(
    controller: Arc<Mutex<VoiceController>>,
    runtime: tokio::runtime::Handle,
    proxy: EventLoopProxy<UserEvent>,
) {
    runtime.spawn(async move {
        let mut controller = controller.lock().await;
        match controller.start().await {
            Ok(()) => {
                let _ = proxy.send_event(UserEvent::SetState(VoiceState::Recording));
            }
            Err(error) => tracing::error!("Unable to start voice input: {error:#}"),
        }
    });
}

fn stop_recording(
    controller: Arc<Mutex<VoiceController>>,
    runtime: tokio::runtime::Handle,
    proxy: EventLoopProxy<UserEvent>,
) {
    let _ = proxy.send_event(UserEvent::SetState(VoiceState::Processing));
    runtime.spawn(async move {
        let mut controller = controller.lock().await;
        if let Err(error) = controller.stop().await {
            tracing::error!("Unable to stop voice input: {error:#}");
        }
        let _ = proxy.send_event(UserEvent::SetState(VoiceState::Idle));
    });
}

fn webview_data_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("DoubaoVoiceInput")
        .join("WebView2")
}

fn build_webview(
    window: &Window,
    hud: bool,
    proxy: EventLoopProxy<UserEvent>,
    web_context: &mut WebContext,
) -> Result<WebView> {
    let url = if hud {
        "doubao://localhost/index.html?view=hud"
    } else {
        "doubao://localhost/index.html"
    };
    let view_name = if hud { "HUD" } else { "settings" };
    let builder = WebViewBuilder::new_with_web_context(web_context)
        .with_transparent(hud)
        .with_custom_protocol("doubao".into(), |_, request| {
            asset_response(request.uri().path())
        })
        .with_on_page_load_handler(move |event, url| match event {
            PageLoadEvent::Started => tracing::debug!("{view_name} page loading: {url}"),
            PageLoadEvent::Finished => tracing::info!("{view_name} page loaded: {url}"),
        })
        .with_ipc_handler(move |request| match parse_ipc(request.body()) {
            Ok(command) => {
                let _ = proxy.send_event(UserEvent::Ipc(command));
            }
            Err(error) => tracing::warn!("Ignoring invalid UI IPC request: {error:#}"),
        })
        .with_url(url);
    builder.build(window).map_err(Into::into)
}

fn parse_ipc(message: &str) -> Result<IpcCommand> {
    let value: serde_json::Value = serde_json::from_str(message)?;
    match value.get("command").and_then(serde_json::Value::as_str) {
        Some("get_config") => Ok(IpcCommand::GetConfig),
        Some("capture_raw_key") => Ok(IpcCommand::CaptureRawKey),
        Some("show_settings") => Ok(IpcCommand::ShowSettings),
        Some("get_voice_state") => Ok(IpcCommand::GetVoiceState),
        Some("start_recording") => Ok(IpcCommand::StartRecording),
        Some("stop_recording") => Ok(IpcCommand::StopRecording),
        Some("drag_hud") => Ok(IpcCommand::DragHud),
        Some("save_config") => Ok(IpcCommand::SaveConfig(serde_json::from_value(
            value
                .get("params")
                .and_then(|params| params.get("config"))
                .cloned()
                .ok_or_else(|| anyhow!("save_config requires params.config"))?,
        )?)),
        _ => Err(anyhow!("unknown IPC command")),
    }
}

fn asset_response(path: &str) -> Response<Cow<'static, [u8]>> {
    let path = match path.trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    match FrontendAssets::get(path) {
        Some(asset) => Response::builder()
            .header(
                "content-type",
                mime_guess::from_path(path).first_or_octet_stream().as_ref(),
            )
            .body(Cow::Owned(asset.data.into_owned()))
            .unwrap(),
        None => Response::builder()
            .status(404)
            .body(Cow::Owned(Vec::new()))
            .unwrap(),
    }
}

fn send_event(webview: &WebView, value: &serde_json::Value) {
    let payload = value.to_string().replace('<', "\\u003c");
    let _ = webview.evaluate_script(&format!(
        "window.__doubaoEvent && window.__doubaoEvent({payload});"
    ));
}
fn send_error(webview: &WebView, error: impl std::fmt::Display) {
    send_event(
        webview,
        &serde_json::json!({"type":"error", "message":error.to_string()}),
    );
}

#[cfg(target_os = "windows")]
fn set_hud_no_activate(window: &Window) {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            GetWindowLongW, SetWindowLongW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    };
    unsafe {
        let hwnd = HWND(window.hwnd() as isize);
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        SetWindowLongW(
            hwnd,
            GWL_EXSTYLE,
            style | WS_EX_NOACTIVATE.0 as i32 | WS_EX_TOOLWINDOW.0 as i32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn set_hud_no_activate(_window: &Window) {}

fn load_icon() -> Result<tray_icon::Icon> {
    let mut rgba = vec![0; 32 * 32 * 4];
    for y in 0..32 {
        for x in 0..32 {
            let dx = x as i32 - 16;
            let dy = y as i32 - 16;
            if dx * dx + dy * dy < 220 {
                let index = (y * 32 + x) * 4;
                rgba[index..index + 4].copy_from_slice(&[124, 58, 237, 255]);
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, 32, 32).map_err(Into::into)
}
