import "./style.css";
import "./backend.css";

type VoiceState = "idle" | "recording" | "processing";
type Config = {
  general: { auto_start: boolean; language: string };
  hotkey: { binding: string; mode: string; combo_key: string; double_tap_key: string; double_tap_interval: number; raw_vk_code: number; raw_scan_code: number; raw_extended: boolean };
  floating_button: { enabled: boolean; position_x: number; position_y: number };
  asr: { vad_enabled: boolean; aec_enabled: boolean; audio_quality: string; punctuation_mode: string; end_smooth_window_ms: number; post_ratio_gain: number };
  cloud: { ner_enabled: boolean; auto_polish_enabled: boolean; llm_context_enabled: boolean; llm_custom_api_enabled?: boolean | null; llm_base_url: string; llm_api_key: string; llm_model: string; llm_prompt: string; llm_thinking_mode: string; llm_reasoning_effort: string };
};

declare global { interface Window { __doubaoEvent?: (event: Record<string, unknown>) => void; ipc?: { postMessage(message: string): void } } }

const app = document.querySelector<HTMLDivElement>("#app")!;
const isHud = new URLSearchParams(location.search).get("view") === "hud";
let config: Config | null = null;
let state: VoiceState = "idle";
let meter = 0;
const post = (command: string, params: Record<string, unknown> = {}) => window.ipc?.postMessage(JSON.stringify({ command, params }));
const esc = (value: unknown) => String(value ?? "").replace(/[&<>'"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;" }[c]!));
const field = (id: string, label: string, value: unknown, type = "text") => `<label class="field"><span>${label}</span><input id="${id}" type="${type}" value="${esc(value)}" /></label>`;
const toggle = (id: string, label: string, on: boolean, hint = "") => `<label class="toggle"><span><b>${label}</b>${hint && `<small>${hint}</small>`}</span><input id="${id}" type="checkbox" ${on ? "checked" : ""}/><i></i></label>`;
const select = (id: string, label: string, value: string, options: [string, string][]) => `<label class="field"><span>${label}</span><select id="${id}">${options.map(([v, text]) => `<option value="${v}" ${v === value ? "selected" : ""}>${text}</option>`).join("")}</select></label>`;

function renderSettings() {
  if (!config) { app.innerHTML = `<main class="loading">正在加载设置...</main>`; return; }
  const c = config;
  const hotkeyKind = c.hotkey.binding === "raw" ? "raw" : c.hotkey.mode;
  const customApiEnabled = c.cloud.llm_custom_api_enabled ?? Boolean(c.cloud.llm_base_url.trim());
  app.innerHTML = `<div class="shell"><div class="window-titlebar" data-window-drag><div class="window-title"><strong>D</strong><span>豆包语音输入</span></div><div class="window-controls"><button id="minimize" title="最小化" aria-label="最小化">&#8212;</button><button id="maximize" title="最大化" aria-label="最大化">&#9633;</button><button class="close" id="close" title="关闭" aria-label="关闭">&#10005;</button></div></div><aside><div class="brand"><strong>D</strong><div><b>豆包语音输入</b><small>VOICE UTILITY</small></div></div><nav aria-label="设置分类"><button class="active" data-page="general">常规设置 <small>GENERAL</small></button><button data-page="hotkeys">热键配置 <small>HOTKEYS</small></button><button data-page="floating">悬浮窗口 <small>HUD</small></button><button data-page="asr">识别引擎 <small>ASR</small></button><button data-page="cloud">云端增强 <small>CLOUD</small></button></nav><div class="ready"><i></i>服务已就绪<br/><small>双击 Ctrl 开始输入</small></div></aside><main class="settings-main"><header><div><p>SETTINGS / 控制台</p><h1 id="page-title">常规设置</h1></div><div class="header-actions"><button class="primary" id="save">保存更改</button></div></header><div class="page-stack">
  <section class="settings-page active" id="general"><p class="eyebrow">01 / GENERAL</p><h2>常规设置</h2><div class="grid two">${toggle("auto_start", "开机自动启动", c.general.auto_start, "登录 Windows 后启动服务")}${field("language", "界面语言", c.general.language)}</div><div class="row"><button class="secondary" id="open-logs">打开日志文件夹</button><span>按日期保存运行日志，便于排查问题</span></div></section>
  <section class="settings-page" id="hotkeys"><p class="eyebrow">02 / HOTKEYS</p><h2>热键配置</h2><div class="grid two">${select("hotkey_kind", "触发类型", hotkeyKind, [["double_tap", "双击"], ["combo", "组合键"], ["raw", "非标准按键"]])}<div data-hotkey-kind="combo">${field("combo_key", "组合键", c.hotkey.combo_key)}</div><div data-hotkey-kind="double_tap">${field("double_tap_key", "双击按键", c.hotkey.double_tap_key)}</div><div data-hotkey-kind="double_tap">${field("double_tap_interval", "双击间隔（毫秒）", c.hotkey.double_tap_interval, "number")}</div></div><div class="row" data-hotkey-kind="raw"><button class="secondary" id="capture">录入非标准按键</button><span id="capture-status">${c.hotkey.binding === "raw" ? `已录入：VK ${c.hotkey.raw_vk_code} / 扫描码 ${c.hotkey.raw_scan_code}` : "可录入小爱键、媒体键等厂商按键"}</span></div></section>
  <section class="settings-page" id="floating"><p class="eyebrow">03 / HUD</p><h2>悬浮窗口</h2><div class="grid two">${toggle("floating_enabled", "启用录音 HUD", c.floating_button.enabled, "录音时显示置顶状态窗口")}<div class="preview"><b>●</b><span><strong>正在聆听</strong><small>实时音频电平</small></span><em></em></div></div></section>
  <section class="settings-page" id="asr"><p class="eyebrow">04 / ASR</p><h2>识别引擎</h2><div class="grid two">${toggle("vad", "本地语音活动检测", c.asr.vad_enabled)}${toggle("aec", "回声消除（实验性）", c.asr.aec_enabled)}${select("audio_quality", "音频质量", c.asr.audio_quality, [["standard", "标准 16 kHz"], ["high_quality", "高质量 24 kHz"]])}${select("punctuation", "标点模式", c.asr.punctuation_mode, [["smart", "智能标点"], ["spaces", "空格分词"], ["no_sentence_final", "无句末标点"], ["preserve", "保留服务端结果"]])}${field("smooth", "尾音平滑（毫秒）", c.asr.end_smooth_window_ms, "number")}${field("gain", "麦克风增益", c.asr.post_ratio_gain, "number")}</div></section>
  <section class="settings-page" id="cloud"><p class="eyebrow">05 / CLOUD</p><h2>云端增强</h2><div class="grid two">${toggle("ner", "实体识别", c.cloud.ner_enabled)}${toggle("polish", "自动语音校正", c.cloud.auto_polish_enabled)}${toggle("context", "读取光标上下文", c.cloud.llm_context_enabled, "仅用于校正请求")}${toggle("custom_api", "使用自定义 API", customApiEnabled, "关闭时使用项目内置的豆包润色服务")}</div><div class="backend-note" id="backend-note"></div><div class="custom-api-fields" id="custom-api-fields" ${customApiEnabled ? "" : "hidden"}><div class="grid two">${field("llm_url", "OpenAI 兼容接口地址", c.cloud.llm_base_url)}${field("llm_model", "模型名称", c.cloud.llm_model)}${field("llm_key", "API 密钥", c.cloud.llm_api_key, "password")}${select("thinking", "深度思考", c.cloud.llm_thinking_mode, [["omit", "不发送参数"], ["disabled", "关闭"], ["enabled", "开启"]])}${select("reasoning", "推理强度", c.cloud.llm_reasoning_effort, [["", "不发送参数"], ["low", "低"], ["medium", "中"], ["high", "高"]])}</div></div><label class="field full"><span>校正提示词（留空使用内置规则）</span><textarea id="llm_prompt">${esc(c.cloud.llm_prompt)}</textarea></label></section></div></main></div>`;
  document.querySelector("#save")?.addEventListener("click", save);
  document.querySelector("#close")?.addEventListener("click", () => post("hide_settings"));
  document.querySelector("#minimize")?.addEventListener("click", () => post("minimize_settings"));
  document.querySelector("#maximize")?.addEventListener("click", () => post("toggle_settings_maximize"));
  document.querySelectorAll<HTMLButtonElement>("nav [data-page]").forEach(button => button.addEventListener("click", () => {
    const page = button.dataset.page;
    if (!page) return;
    document.querySelectorAll("nav [data-page], .settings-page").forEach(node => node.classList.remove("active"));
    button.classList.add("active");
    document.querySelector(`#${page}`)?.classList.add("active");
    const title = document.querySelector<HTMLElement>("#page-title");
    if (title) title.textContent = button.childNodes[0]?.textContent?.trim() || "设置";
  }));
  document.querySelectorAll<HTMLElement>("[data-window-drag]").forEach(node => node.addEventListener("mousedown", event => {
    if (event.button !== 0 || event.detail > 1 || (event.target as HTMLElement).closest("button, input, select, textarea, label")) return;
    post("drag_settings");
  }));
  document.querySelector(".window-titlebar")?.addEventListener("dblclick", event => {
    if (!(event.target as HTMLElement).closest("button")) post("toggle_settings_maximize");
  });
  document.querySelector("#capture")?.addEventListener("click", () => { post("capture_raw_key"); setCapture("请在 10 秒内按下要绑定的按键..."); });
  document.querySelector("#hotkey_kind")?.addEventListener("change", syncHotkeyFields);
  syncHotkeyFields();
  document.querySelector("#open-logs")?.addEventListener("click", () => post("open_logs"));
  const customApiToggle = document.querySelector<HTMLInputElement>("#custom_api");
  const updateBackendMode = () => {
    const custom = Boolean(customApiToggle?.checked);
    const fields = document.querySelector<HTMLElement>("#custom-api-fields");
    if (fields) fields.hidden = !custom;
    const note = document.querySelector<HTMLElement>("#backend-note");
    if (note) note.innerHTML = custom
      ? `<b>自定义 OpenAI 兼容 API</b><small>请求将发送到下方地址，并使用你提供的 API Key 和模型。</small>`
      : `<b>内置豆包润色</b><small>使用已注册的本机设备凭据调用现有 Scene 5 服务，无需 API Key。</small>`;
  };
  customApiToggle?.addEventListener("change", updateBackendMode);
  updateBackendMode();
  post("get_settings_window_state");
}
function setCapture(message: string) { const node = document.querySelector("#capture-status"); if (node) node.textContent = message; }
function syncHotkeyFields() { const kind = document.querySelector<HTMLSelectElement>("#hotkey_kind")?.value; document.querySelectorAll<HTMLElement>("[data-hotkey-kind]").forEach(node => { node.hidden = node.dataset.hotkeyKind !== kind; }); }
function value(id: string) { return document.querySelector<HTMLInputElement | HTMLSelectElement>(`#${id}`)!.value; }
function enabled(id: string) { return document.querySelector<HTMLInputElement>(`#${id}`)!.checked; }
function save() {
  if (!config) return;
  Object.assign(config.general, { auto_start: enabled("auto_start"), language: value("language") });
  const hotkeyKind = value("hotkey_kind");
  Object.assign(config.hotkey, { binding: hotkeyKind === "raw" ? "raw" : "standard", combo_key: value("combo_key"), double_tap_key: value("double_tap_key"), double_tap_interval: Number(value("double_tap_interval")), mode: hotkeyKind === "double_tap" ? "double_tap" : "combo" });
  Object.assign(config.floating_button, { enabled: enabled("floating_enabled") });
  Object.assign(config.asr, { vad_enabled: enabled("vad"), aec_enabled: enabled("aec"), audio_quality: value("audio_quality"), punctuation_mode: value("punctuation"), end_smooth_window_ms: Number(value("smooth")), post_ratio_gain: Number(value("gain")) });
  Object.assign(config.cloud, { ner_enabled: enabled("ner"), auto_polish_enabled: enabled("polish"), llm_context_enabled: enabled("context"), llm_custom_api_enabled: enabled("custom_api"), llm_base_url: value("llm_url"), llm_model: value("llm_model"), llm_api_key: value("llm_key"), llm_thinking_mode: value("thinking"), llm_reasoning_effort: value("reasoning"), llm_prompt: document.querySelector<HTMLTextAreaElement>("#llm_prompt")!.value });
  post("save_config", { config });
}
function renderHud() {
  const recording = state === "recording";
  app.innerHTML = `<div class="hud ${state}" id="hud"><div class="hud-top"><div><b><i>${recording ? "●" : "◌"}</i>${recording ? "正在聆听" : "正在处理"}</b><small>${recording ? "VOICE INPUT ACTIVE" : "FINALIZING RESULT"}</small></div><button id="stop" ${recording ? "" : "disabled"}>停止</button></div><div class="wave">${Array.from({ length: 18 }, () => "<i></i>").join("")}</div><footer><button id="settings">设置</button><span>${recording ? "双击 Ctrl 停止" : "请稍候"}</span></footer></div>`;
  document.querySelector("#stop")?.addEventListener("click", () => post("stop_recording")); document.querySelector("#settings")?.addEventListener("click", () => post("show_settings")); document.querySelector("#hud")?.addEventListener("mousedown", event => { if ((event.target as HTMLElement).closest("button")) return; post("drag_hud"); }); paintMeter();
}
function paintMeter() { document.querySelectorAll<HTMLElement>(".wave i").forEach((bar, index) => { const curve = 0.5 + Math.abs(8.5 - index) / 15; bar.style.height = `${5 + meter * 34 * curve}px`; }); }
window.__doubaoEvent = event => { if (event.type === "config") { config = event.config as Config; renderSettings(); } if (event.type === "voice_state") { state = event.state as VoiceState; if (isHud) renderHud(); } if (event.type === "window_state") { const button = document.querySelector<HTMLButtonElement>("#maximize"); const maximized = Boolean(event.maximized); if (button) { button.innerHTML = maximized ? "&#10064;" : "&#9633;"; button.title = maximized ? "还原" : "最大化"; button.setAttribute("aria-label", button.title); } } if (event.type === "meter") { meter = Number(event.value) || 0; if (isHud) paintMeter(); } if (event.type === "capture_result") { const binding = event.binding as { vk_code: number; scan_code: number; extended: boolean } | undefined; if (binding && config) { Object.assign(config.hotkey, { binding: "raw", raw_vk_code: binding.vk_code, raw_scan_code: binding.scan_code, raw_extended: binding.extended }); const kind = document.querySelector<HTMLSelectElement>("#hotkey_kind"); if (kind) kind.value = "raw"; syncHotkeyFields(); } setCapture(String(event.message)); } if (event.type === "error") alert(String(event.message)); };
if (isHud) { renderHud(); post("get_voice_state"); } else { renderSettings(); post("get_config"); }
