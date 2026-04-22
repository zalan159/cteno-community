use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;
use uuid::Uuid;

const SKILL_STORE_WINDOW_LABEL: &str = "skill-store";
const SKILL_STORE_URL: &str = "https://skillsmp.com/zh";
const SOURCE_META_FILE: &str = ".cteno-source.json";

lazy_static! {
    static ref SKILL_STORE_PENDING_BRIDGE: Mutex<HashMap<String, oneshot::Sender<Value>>> =
        Mutex::new(HashMap::new());
}

const SKILL_STORE_INJECTION_SCRIPT: &str = r#"
(() => {
  const MARK_ATTR = 'data-cteno-skill-install-ready';
  const WGET_MARK_ATTR = 'data-cteno-wget-install-ready';
  const BTN_CLASS = 'cteno-skill-install-btn';
  const NAV_BTN_CLASS = 'cteno-skill-nav-btn';
  const STYLE_ID = 'cteno-skill-install-style';
  const NAV_ROOT_ID = 'cteno-skill-nav-root';
  const SKILL_STORE_HOME = 'https://skillsmp.com/zh';
  const WINDOW_OPEN_PATCHED = '__cteno_window_open_patched__';
  const LINK_PATCHED = '__cteno_link_patched__';
  const INVOKE_PATCHED = '__cteno_invoke_patched__';
  const SHELL_PATCHED = '__cteno_shell_patched__';
  const debug = () => {};

  const style = `
    .${BTN_CLASS} {
      margin-left: 10px;
      border: 0;
      border-radius: 10px;
      padding: 9px 16px;
      font-size: 14px;
      font-weight: 600;
      line-height: 1.2;
      min-height: 40px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      cursor: pointer;
      background: #0f766e;
      color: #fff;
      vertical-align: middle;
    }
    .${BTN_CLASS}[disabled] {
      opacity: 0.7;
      cursor: progress;
    }
    .${NAV_BTN_CLASS} {
      border: 0;
      border-radius: 999px;
      padding: 10px 16px;
      font-size: 14px;
      font-weight: 700;
      line-height: 1;
      min-height: 40px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      cursor: pointer;
      background: rgba(15, 23, 42, 0.92);
      color: #fff;
      box-shadow: 0 8px 20px rgba(15, 23, 42, 0.28);
      backdrop-filter: blur(4px);
    }
    .${NAV_BTN_CLASS}:hover {
      background: rgba(15, 23, 42, 1);
    }
  `;

  const ensureStyle = () => {
    if (document.getElementById(STYLE_ID)) return;
    const el = document.createElement('style');
    el.id = STYLE_ID;
    el.textContent = style;
    document.head.appendChild(el);
  };

  const toAbsoluteUrl = (href) => {
    if (!href) return null;
    try {
      return new URL(href, window.location.href).toString();
    } catch (_) {
      return null;
    }
  };

  const shouldStayInWebview = (url) => {
    if (!url) return false;
    return /^https?:\/\//i.test(url);
  };

  const extractGithubUrl = (href) => {
    if (!href) return null;
    try {
      const u = new URL(href, window.location.href);
      if (/^github\.com$/i.test(u.hostname) || /^www\.github\.com$/i.test(u.hostname)) {
        if (/\/(?:tree|blob)\//i.test(u.pathname)) {
          return u.toString();
        }
        return null;
      }

      if (/manus\.im$/i.test(u.hostname) && /import-skills/.test(u.pathname)) {
        const gh = u.searchParams.get('githubUrl');
        if (!gh) return null;
        const parsed = new URL(gh);
        if (
          (/^github\.com$/i.test(parsed.hostname) || /^www\.github\.com$/i.test(parsed.hostname)) &&
          /\/(?:tree|blob)\//i.test(parsed.pathname)
        ) {
          return parsed.toString();
        }
      }
    } catch (_) {}
    return null;
  };

  const resolveInstallTargetFromHref = (href) => {
    if (!href) return null;
    const resolved = toAbsoluteUrl(href);
    if (!resolved) return null;

    const githubUrl = extractGithubUrl(resolved);
    if (githubUrl) {
      return { kind: 'github', value: githubUrl };
    }

    return null;
  };

  const decodeHtmlText = (text) => {
    if (!text) return '';
    return text.replace(/&amp;/gi, '&');
  };

  const extractGithubUrlFromText = (text) => {
    if (!text) return null;
    const source = decodeHtmlText(text);

    const direct = source.match(/https?:\/\/(?:www\.)?github\.com\/[^\s"'<>]+\/(?:tree|blob)\/[^\s"'<>]+/i);
    if (direct && direct[0]) {
      return direct[0];
    }

    const manus = source.match(/https?:\/\/(?:www\.)?manus\.im\/import-skills\?[^\s"'<>]+/i);
    if (manus && manus[0]) {
      return extractGithubUrl(manus[0]);
    }

    const encoded = source.match(/githubUrl=([^\s"'<>]+)/i);
    if (encoded && encoded[1]) {
      try {
        const decoded = decodeURIComponent(encoded[1]);
        return extractGithubUrl(decoded);
      } catch (_) {}
    }

    return null;
  };

  const inferInstallTarget = (seed) => {
    const inspected = new Set();
    const nodes = [];
    let preferredTarget = null;

    const pickPreferredTarget = (target) => {
      if (!target) return;
      if (!preferredTarget || (target.kind === 'github' && preferredTarget.kind !== 'github')) {
        preferredTarget = target;
      }
    };

    let current = seed instanceof Element ? seed : null;
    for (let depth = 0; current && depth < 6; depth += 1) {
      nodes.push(current);
      current = current.parentElement;
    }

    nodes.push(document.body || document.documentElement);

    const attrs = ['href', 'data-href', 'data-url', 'data-link', 'onclick'];
    for (const node of nodes) {
      if (!(node instanceof Element)) continue;
      if (inspected.has(node)) continue;
      inspected.add(node);

      for (const attr of attrs) {
        const value = node.getAttribute(attr);
        const target = resolveInstallTargetFromHref(value);
        pickPreferredTarget(target);
      }

      const anchor = node.closest('a[href]');
      if (anchor) {
        const target = resolveInstallTargetFromHref(anchor.getAttribute('href'));
        pickPreferredTarget(target);
      }

      const childAnchors = node.querySelectorAll('a[href]');
      for (const child of childAnchors) {
        const target = resolveInstallTargetFromHref(child.getAttribute('href'));
        pickPreferredTarget(target);
      }
    }

    const allAnchors = document.querySelectorAll('a[href]');
    for (const anchor of allAnchors) {
      const target = resolveInstallTargetFromHref(anchor.getAttribute('href'));
      pickPreferredTarget(target);
    }

    const html = document.documentElement ? document.documentElement.innerHTML : '';
    const githubFromHtml = extractGithubUrlFromText(html);
    if (githubFromHtml) {
      pickPreferredTarget({ kind: 'github', value: githubFromHtml });
    }

    return preferredTarget;
  };

  const isWgetSkillZipButton = (element) => {
    if (!(element instanceof HTMLElement)) return false;
    const text = (element.textContent || '').toLowerCase().replace(/\s+/g, ' ').trim();
    if (!text.includes('wget') || !text.includes('skill.zip')) return false;

    const className = typeof element.className === 'string' ? element.className : '';
    if (className.includes('cursor-pointer') || className.includes('bg-primary')) {
      return true;
    }

    const role = (element.getAttribute('role') || '').toLowerCase();
    return element.tagName === 'BUTTON' || role === 'button';
  };

  const setButtonState = (btn, text, disabled) => {
    btn.textContent = text;
    btn.disabled = !!disabled;
  };

  const TARGET_STATUS_CACHE = new Map();
  const BRIDGE_TIMEOUT_MS = 45000;
  let BRIDGE_SEQ = 0;

  const nextBridgeRequestId = () => {
    BRIDGE_SEQ += 1;
    return `skill-store-${Date.now()}-${BRIDGE_SEQ}`;
  };

  const ensureReactNativeBridge = () => {
    if (window.__ctenoSkillStoreRNBridgeReady) return;
    window.__ctenoSkillStoreRNBridgeReady = true;
    window.__ctenoSkillStoreRNPending = new Map();
    window.__ctenoSkillStoreReceiveResponse = (requestId, result) => {
      const pending = window.__ctenoSkillStoreRNPending.get(requestId);
      if (!pending) return;
      window.__ctenoSkillStoreRNPending.delete(requestId);
      clearTimeout(pending.timer);
      pending.resolve(result);
    };
  };

  const callBridge = async (action, payload) => {
    const requestId = nextBridgeRequestId();
    const request = { requestId, action, payload };

    const internals = window.__TAURI_INTERNALS__;
    if (internals && typeof internals.invoke === 'function') {
      return internals.invoke('skill_store_bridge_request', { request });
    }

    const rnBridge = window.ReactNativeWebView;
    if (rnBridge && typeof rnBridge.postMessage === 'function') {
      ensureReactNativeBridge();
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          window.__ctenoSkillStoreRNPending.delete(requestId);
          reject(new Error('Bridge request timed out'));
        }, BRIDGE_TIMEOUT_MS);
        window.__ctenoSkillStoreRNPending.set(requestId, { resolve, reject, timer });
        rnBridge.postMessage(
          JSON.stringify({
            type: 'cteno-skill-store-request',
            requestId,
            action,
            payload,
          })
        );
      });
    }

    throw new Error('Bridge unavailable in this environment');
  };

  const targetCacheKey = (target) => {
    if (!target || !target.kind || !target.value) return '';
    return `${target.kind}:${target.value}`;
  };

  const resolveButtonLabel = (status) => {
    if (status?.installed && status?.hasUpdate) {
      return '升级到 Cteno';
    }
    if (status?.installed) {
      return '已安装';
    }
    return '安装到 Cteno';
  };

  const applyStatusToButton = (btn, status) => {
    const disabled = !!(status?.installed && !status?.hasUpdate);
    setButtonState(btn, resolveButtonLabel(status), disabled);
    if (status?.installed) {
      const local = status?.localVersion || 'unknown';
      const remote = status?.remoteVersion || 'unknown';
      btn.title = status?.hasUpdate
        ? `本地版本 ${local}，远端版本 ${remote}`
        : `已安装，本地版本 ${local}`;
    } else {
      btn.title = '';
    }
  };

  const loadInstallStatus = async (target) => {
    if (!target || target.kind !== 'github') return null;
    const key = targetCacheKey(target);
    if (TARGET_STATUS_CACHE.has(key)) {
      return TARGET_STATUS_CACHE.get(key);
    }

    try {
      const status = await callBridge('get-install-status', {
        githubUrl: target.value,
      });
      TARGET_STATUS_CACHE.set(key, status || null);
      return status || null;
    } catch (_) {
      return null;
    }
  };

  const refreshButtonStatus = async (btn, target) => {
    if (!btn || !target) return;
    if (target.kind !== 'github') {
      setButtonState(btn, '安装到 Cteno', false);
      return;
    }

    setButtonState(btn, '检查版本中...', true);
    const status = await loadInstallStatus(target);
    applyStatusToButton(btn, status);
  };

  const installFromTarget = async (target, btn) => {
    debug('installFromTarget', target);

    const previous = btn.textContent || 'Install';
    setButtonState(btn, 'Installing...', true);
    try {
      let result = null;
      if (target?.kind === 'github') {
        result = await callBridge('install-from-github', { githubUrl: target.value });
      } else {
        throw new Error('Unsupported install target');
      }
      if (result?.success === false) {
        throw new Error(result?.error || 'Install failed');
      }
      const skillId = result?.skillId || result?.skill_id || 'unknown';
      TARGET_STATUS_CACHE.delete(targetCacheKey(target));
      await refreshButtonStatus(btn, target);
      alert(`Skill installed: ${skillId}`);
    } catch (err) {
      console.error('[Skill Store] Install failed:', err);
      setButtonState(btn, previous, false);
      alert(`Install failed: ${String(err)}`);
    }
  };

  const decorateAnchor = (anchor) => {
    if (!anchor || anchor.getAttribute(MARK_ATTR) === '1') return;

    const rawHref = anchor.getAttribute('href') || '';
    const target = resolveInstallTargetFromHref(rawHref);
    if (!target) return;

    anchor.setAttribute(MARK_ATTR, '1');

    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = BTN_CLASS;
    btn.textContent = '检查版本中...';
    btn.addEventListener('click', (e) => {
      e.preventDefault();
      e.stopPropagation();
      installFromTarget(target, btn);
    });

    anchor.insertAdjacentElement('afterend', btn);
    refreshButtonStatus(btn, target);
  };

  const decorateWgetButton = (element) => {
    if (!isWgetSkillZipButton(element)) return;
    if (element.getAttribute(WGET_MARK_ATTR) === '1') return;
    element.setAttribute(WGET_MARK_ATTR, '1');
    debug('decorateWgetButton attached', element.tagName, element.className || '');

    const installBtn = document.createElement('button');
    installBtn.type = 'button';
    installBtn.className = BTN_CLASS;
    installBtn.textContent = '安装到 Cteno';

    const initialTarget = inferInstallTarget(element);
    if (initialTarget) {
      refreshButtonStatus(installBtn, initialTarget);
    }

    installBtn.addEventListener('click', (event) => {
      event.preventDefault();
      event.stopPropagation();
      const target = inferInstallTarget(element);
      debug('install button clicked', target);
      if (!target) {
        alert('未找到可安装的链接');
        return;
      }
      installFromTarget(target, installBtn);
    });

    element.insertAdjacentElement('afterend', installBtn);

    element.addEventListener(
      'click',
      (event) => {
        debug('wget button clicked; redirecting to install flow');
        event.preventDefault();
        event.stopPropagation();
        if (typeof event.stopImmediatePropagation === 'function') {
          event.stopImmediatePropagation();
        }
        installBtn.click();
      },
      true
    );
  };

  const scan = () => {
    ensureStyle();
    ensureNavigationControls();
    document.querySelectorAll('a[href]').forEach((a) => decorateAnchor(a));
    document
      .querySelectorAll('button, [role="button"], div[class*="cursor-pointer"]')
      .forEach((element) => decorateWgetButton(element));
  };

  const navigateBackOrHome = () => {
    const home = toAbsoluteUrl(SKILL_STORE_HOME) || SKILL_STORE_HOME;
    const before = toAbsoluteUrl(window.location.href) || window.location.href;
    if (window.history.length > 1) {
      window.history.back();
      setTimeout(() => {
        const after = toAbsoluteUrl(window.location.href) || window.location.href;
        if (after === before) {
          window.location.assign(home);
        }
      }, 360);
      return;
    }
    window.location.assign(home);
  };

  const ensureNavigationControls = () => {
    if (document.getElementById(NAV_ROOT_ID)) return;
    const root = document.createElement('div');
    root.id = NAV_ROOT_ID;
    root.style.position = 'fixed';
    root.style.left = '14px';
    root.style.top = '14px';
    root.style.zIndex = '2147483647';
    root.style.display = 'flex';
    root.style.gap = '8px';
    root.style.pointerEvents = 'auto';

    const backBtn = document.createElement('button');
    backBtn.type = 'button';
    backBtn.className = NAV_BTN_CLASS;
    backBtn.textContent = '返回';
    backBtn.title = '返回上一页；若无历史则回到技能商店首页';
    backBtn.addEventListener('click', (event) => {
      event.preventDefault();
      event.stopPropagation();
      navigateBackOrHome();
    });

    root.appendChild(backBtn);
    const mount = document.body || document.documentElement;
    mount.appendChild(root);
  };

  const patchWindowOpen = () => {
    if (window[WINDOW_OPEN_PATCHED]) return;
    window[WINDOW_OPEN_PATCHED] = true;

    const originalOpen = window.open.bind(window);
    const patchedOpen = function(url, target, features) {
      const raw = url == null ? null : String(url);
      debug('window.open called', raw, target);
      if (!raw) {
        const targetText = target == null ? '' : String(target).toLowerCase();
        if (!targetText || targetText === '_blank' || targetText === '_new') {
          debug('window.open blocked empty url');
          return window;
        }
      }
      const resolved = raw ? toAbsoluteUrl(raw) : null;
      if (resolved && shouldStayInWebview(resolved)) {
        debug('window.open redirected in-webview', resolved);
        window.location.assign(resolved);
        return window;
      }
      return originalOpen(url, target, features);
    };
    window.open = patchedOpen;

    try {
      Object.defineProperty(window, 'open', {
        configurable: true,
        get: () => patchedOpen,
        set: () => {},
      });
    } catch (_) {}
  };

  const patchBlankLinks = () => {
    if (window[LINK_PATCHED]) return;
    window[LINK_PATCHED] = true;

    const handler = (event) => {
      const target = event.target;
      if (!(target instanceof Element)) return;

      const anchor = target.closest('a[href]');
      if (!anchor) return;

      const rawHref = anchor.getAttribute('href');
      const resolved = toAbsoluteUrl(rawHref);
      if (!resolved || !shouldStayInWebview(resolved)) return;

      const targetBlank = (anchor.getAttribute('target') || '').toLowerCase() === '_blank';
      const hasWindowOpen = (anchor.getAttribute('onclick') || '').includes('window.open');
      const isDownload = anchor.hasAttribute('download');
      if (!targetBlank && !hasWindowOpen && !isDownload) return;

      debug('anchor click intercepted', resolved, {
        targetBlank,
        hasWindowOpen,
        isDownload,
      });
      event.preventDefault();
      event.stopPropagation();
      if (typeof event.stopImmediatePropagation === 'function') {
        event.stopImmediatePropagation();
      }
      window.location.assign(resolved);
    };

    document.addEventListener('click', handler, true);
    document.addEventListener('auxclick', handler, true);
  };

  const isBlockedInvokeCommand = (command) => {
    const cmd = String(command || '').toLowerCase();
    if (!cmd) return false;
    if (cmd === 'open_url') return true;
    if (cmd === 'plugin:shell|open') return true;
    if (cmd.includes('plugin:shell') && cmd.includes('open')) return true;
    return false;
  };

  const findHttpUrl = (value, depth = 0) => {
    if (depth > 4 || value == null) return null;
    if (typeof value === 'string') {
      const text = value.trim();
      if (/^https?:\/\//i.test(text)) return text;
      return null;
    }
    if (Array.isArray(value)) {
      for (const item of value) {
        const url = findHttpUrl(item, depth + 1);
        if (url) return url;
      }
      return null;
    }
    if (typeof value === 'object') {
      for (const key of Object.keys(value)) {
        const url = findHttpUrl(value[key], depth + 1);
        if (url) return url;
      }
    }
    return null;
  };

  const navigateInPlace = (rawUrl) => {
    const resolved = toAbsoluteUrl(rawUrl == null ? '' : String(rawUrl));
    if (resolved && shouldStayInWebview(resolved)) {
      debug('navigateInPlace', resolved);
      window.location.assign(resolved);
      return true;
    }
    return false;
  };

  const patchInvoke = () => {
    const internals = window.__TAURI_INTERNALS__;
    if (!internals || typeof internals.invoke !== 'function') return;
    if (window[INVOKE_PATCHED]) return;
    window[INVOKE_PATCHED] = true;

    const originalInvoke = internals.invoke.bind(internals);
    const patchedInvoke = (command, ...rest) => {
      if (isBlockedInvokeCommand(command)) {
        const maybeUrl = findHttpUrl(rest);
        debug('blocked invoke command', String(command || ''), maybeUrl || 'no-url');
        if (maybeUrl) {
          navigateInPlace(maybeUrl);
          return Promise.resolve(null);
        }
        return Promise.resolve(null);
      }
      return originalInvoke(command, ...rest);
    };

    try {
      internals.invoke = patchedInvoke;
    } catch (_) {}

    try {
      Object.defineProperty(internals, 'invoke', {
        configurable: true,
        get: () => patchedInvoke,
        set: () => {},
      });
    } catch (_) {}

    if (window.__TAURI__ && typeof window.__TAURI__ === 'object') {
      try {
        window.__TAURI__.invoke = patchedInvoke;
      } catch (_) {}
    }
  };

  const patchTauriShellOpen = () => {
    if (window[SHELL_PATCHED]) return;
    window[SHELL_PATCHED] = true;

    const fallbackNavigate = (url) => {
      navigateInPlace(url);
      return Promise.resolve();
    };

    const tauriObj = window.__TAURI__;
    if (tauriObj && tauriObj.shell && typeof tauriObj.shell.open === 'function') {
      tauriObj.shell.open = (url) => {
        debug('blocked __TAURI__.shell.open', String(url || ''));
        return fallbackNavigate(url);
      };
    }

    const internals = window.__TAURI_INTERNALS__;
    if (internals && internals.plugins && internals.plugins.shell && typeof internals.plugins.shell.open === 'function') {
      internals.plugins.shell.open = (url) => {
        debug('blocked __TAURI_INTERNALS__.plugins.shell.open', String(url || ''));
        return fallbackNavigate(url);
      };
    }
  };

  const start = () => {
    patchInvoke();
    patchTauriShellOpen();
    patchWindowOpen();
    patchBlankLinks();
    scan();
    const observer = new MutationObserver(() => scan());
    observer.observe(document.documentElement || document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['href', 'class'],
    });
  };

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start, { once: true });
  } else {
    start();
  }
})();
"#;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallResult {
    pub success: bool,
    pub skill_id: String,
    pub install_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInstallStatus {
    pub installed: bool,
    pub has_update: bool,
    pub skill_id: Option<String>,
    pub local_version: Option<String>,
    pub remote_version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SkillSourceMeta {
    pub source_type: String,
    pub source_key: String,
    pub github_url: Option<String>,
    pub installed_version: Option<String>,
    pub installed_at: String,
}

struct GithubInstallMeta<'a> {
    github_url: &'a str,
    source_key: &'a str,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SkillStoreBridgeRequest {
    pub request_id: String,
    pub action: String,
    #[serde(default)]
    pub payload: Value,
}

#[tauri::command]
pub fn open_skill_store_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(SKILL_STORE_WINDOW_LABEL) {
        if let Err(err) = window.eval(SKILL_STORE_INJECTION_SCRIPT) {
            log::warn!(
                "[SkillStore] failed to inject script into existing window: {}",
                err
            );
        }
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    let url = SKILL_STORE_URL
        .parse()
        .map_err(|e| format!("Invalid skill store URL: {}", e))?;
    let app_for_new_window = app.clone();

    tauri::WebviewWindowBuilder::new(
        &app,
        SKILL_STORE_WINDOW_LABEL,
        tauri::WebviewUrl::External(url),
    )
    .title("Skill Store")
    .inner_size(1200.0, 860.0)
    .resizable(true)
    .center()
    .initialization_script(SKILL_STORE_INJECTION_SCRIPT)
    .on_navigation(|_url| true)
    .on_page_load(|window, payload| {
        match payload.event() {
            tauri::webview::PageLoadEvent::Started => {
                if let Err(err) = window.eval(SKILL_STORE_INJECTION_SCRIPT) {
                    log::warn!("[SkillStore] pre-inject script failed: {}", err);
                }
            }
            tauri::webview::PageLoadEvent::Finished => {
                // Re-inject on every full page load to survive SPA/full reload transitions.
                if let Err(err) = window.eval(SKILL_STORE_INJECTION_SCRIPT) {
                    log::warn!("[SkillStore] re-inject script failed: {}", err);
                }
            }
        }
    })
    .on_new_window(move |url, _features| {
        if let Some(window) = app_for_new_window.get_webview_window(SKILL_STORE_WINDOW_LABEL) {
            let _ = window.navigate(url);
            let _ = window.set_focus();
        }
        tauri::webview::NewWindowResponse::Deny
    })
    .build()
    .map_err(|e| format!("Failed to open skill store window: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn skill_store_bridge_request(
    window: tauri::WebviewWindow,
    app: AppHandle,
    request: SkillStoreBridgeRequest,
) -> Result<Value, String> {
    if window.label() != SKILL_STORE_WINDOW_LABEL {
        return Err("Bridge request is only allowed from skill-store window".to_string());
    }
    if request.request_id.trim().is_empty() {
        return Err("requestId is required".to_string());
    }

    let (tx, rx) = oneshot::channel::<Value>();
    {
        let mut pending = SKILL_STORE_PENDING_BRIDGE
            .lock()
            .map_err(|_| "Bridge pending lock poisoned".to_string())?;
        pending.insert(request.request_id.clone(), tx);
    }

    if let Err(err) = app.emit("skill-store://request", &request) {
        if let Ok(mut pending) = SKILL_STORE_PENDING_BRIDGE.lock() {
            pending.remove(&request.request_id);
        }
        return Err(format!("Failed to emit bridge request: {}", err));
    }

    match tokio::time::timeout(std::time::Duration::from_secs(45), rx).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) => Err("Bridge response channel dropped".to_string()),
        Err(_) => {
            if let Ok(mut pending) = SKILL_STORE_PENDING_BRIDGE.lock() {
                pending.remove(&request.request_id);
            }
            Err("Bridge request timeout".to_string())
        }
    }
}

#[tauri::command]
pub fn skill_store_bridge_respond(request_id: String, result: Value) -> Result<(), String> {
    if request_id.trim().is_empty() {
        return Err("requestId is required".to_string());
    }
    let sender = {
        let mut pending = SKILL_STORE_PENDING_BRIDGE
            .lock()
            .map_err(|_| "Bridge pending lock poisoned".to_string())?;
        pending.remove(&request_id)
    };

    if let Some(tx) = sender {
        let _ = tx.send(result);
    }
    Ok(())
}

#[tauri::command]
pub async fn get_skill_install_status(github_url: String) -> Result<SkillInstallStatus, String> {
    get_skill_install_status_impl(github_url).await
}

pub async fn get_skill_install_status_impl(
    github_url: String,
) -> Result<SkillInstallStatus, String> {
    if github_url.trim().is_empty() {
        return Err("github_url is required".to_string());
    }

    let spec = parse_github_tree_url(&github_url)?;
    let source_key = github_source_key(&spec);
    let install_root = resolve_community_skill_dir()?;
    let remote_meta = fetch_remote_skill_meta(&spec).await;

    let mut installed_skill_id = None;
    let mut local_version = None;

    if install_root.exists() {
        let entries = fs::read_dir(&install_root)
            .map_err(|e| format!("Failed to read skill directory {:?}: {}", install_root, e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let matches_source = read_source_meta(&path)
                .map(|meta| {
                    meta.source_type.eq_ignore_ascii_case("github")
                        && meta.source_key.eq_ignore_ascii_case(&source_key)
                })
                .unwrap_or(false);

            if matches_source {
                installed_skill_id = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|s| s.to_string());
                local_version = read_skill_version_from_dir(&path);
                break;
            }
        }
    }

    if installed_skill_id.is_none() {
        let mut fallback_candidates: Vec<String> = Vec::new();
        if let Some(last) = spec.sub_path.last() {
            if !is_skill_markdown_name(last) {
                fallback_candidates.push(last.clone());
            } else if spec.sub_path.len() >= 2 {
                fallback_candidates.push(spec.sub_path[spec.sub_path.len() - 2].clone());
            }
        }
        fallback_candidates.push(spec.repo.clone());
        if let Some(name) = remote_meta
            .as_ref()
            .and_then(|meta| meta.name.as_ref())
            .cloned()
        {
            fallback_candidates.push(name);
        }

        for candidate in fallback_candidates {
            let fallback_skill_id = sanitize_skill_id(&candidate);
            if fallback_skill_id.is_empty() {
                continue;
            }
            let fallback_dir = install_root.join(&fallback_skill_id);
            if fallback_dir.join("SKILL.md").is_file() {
                installed_skill_id = Some(fallback_skill_id);
                local_version = read_skill_version_from_dir(&fallback_dir);
                break;
            }
        }
    }

    let remote_version = remote_meta.and_then(|meta| meta.version);
    let has_update = match (&local_version, &remote_version) {
        (Some(local), Some(remote)) => compare_version_strings(local, remote)
            .map(|ord| ord == Ordering::Less)
            .unwrap_or(false),
        _ => false,
    };

    Ok(SkillInstallStatus {
        installed: installed_skill_id.is_some(),
        has_update,
        skill_id: installed_skill_id,
        local_version,
        remote_version,
    })
}

#[tauri::command]
pub async fn install_skill_from_github_url(
    app: AppHandle,
    github_url: String,
) -> Result<SkillInstallResult, String> {
    let result = install_skill_from_github_url_impl(github_url).await?;
    crate::agent_sync_bridge::reconcile_global_skills_now().await;
    let _ = app.emit("skills://installed", &result);
    Ok(result)
}

pub async fn install_skill_from_github_url_impl(
    github_url: String,
) -> Result<SkillInstallResult, String> {
    if github_url.trim().is_empty() {
        return Err("github_url is required".to_string());
    }

    let spec = parse_github_tree_url(&github_url)?;
    let archive_url = format!(
        "https://api.github.com/repos/{}/{}/zipball/{}",
        spec.owner,
        spec.repo,
        urlencoding::encode(&spec.branch)
    );
    let client = reqwest::Client::new();
    let bytes = download_bytes_with_github_mirror(&client, &archive_url)
        .await
        .map_err(|e| format!("Failed to download GitHub archive: {}", e))?;

    let fallback_name = spec
        .sub_path
        .last()
        .cloned()
        .unwrap_or_else(|| spec.repo.clone());

    let source_key = github_source_key(&spec);
    install_from_zip_bytes(
        bytes.as_ref(),
        &fallback_name,
        Some(&spec.sub_path),
        Some(GithubInstallMeta {
            github_url: &github_url,
            source_key: &source_key,
        }),
    )
}

struct GithubTreeSpec {
    owner: String,
    repo: String,
    branch: String,
    sub_path: Vec<String>,
}

fn parse_github_tree_url(raw: &str) -> Result<GithubTreeSpec, String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("Invalid GitHub URL: {}", e))?;
    let host = url
        .host_str()
        .ok_or_else(|| "Missing URL host".to_string())?
        .to_ascii_lowercase();

    if host != "github.com" && host != "www.github.com" {
        return Err("Only github.com tree/blob URLs are supported".to_string());
    }

    let parts: Vec<String> = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    if parts.len() < 4 {
        return Err("GitHub URL format not supported".to_string());
    }

    let mode = parts[2].to_ascii_lowercase();
    if mode != "tree" && mode != "blob" {
        return Err("Expected GitHub tree/blob URL".to_string());
    }

    let owner = parts[0].clone();
    let repo = parts[1].clone();
    let branch = parts[3].clone();
    let mut sub_path = if parts.len() > 4 {
        parts[4..].to_vec()
    } else {
        Vec::new()
    };

    if mode == "blob" && sub_path.is_empty() {
        return Err("GitHub blob URL must include a file path".to_string());
    }
    if sub_path
        .last()
        .map(|part| is_skill_markdown_name(part))
        .unwrap_or(false)
    {
        sub_path.pop();
    }

    if owner.is_empty() || repo.is_empty() || branch.is_empty() {
        return Err("GitHub URL is missing required fields".to_string());
    }

    Ok(GithubTreeSpec {
        owner,
        repo,
        branch,
        sub_path,
    })
}

fn github_source_key(spec: &GithubTreeSpec) -> String {
    let mut key = format!(
        "{}/{}",
        spec.owner.to_ascii_lowercase(),
        spec.repo.to_ascii_lowercase()
    );

    if !spec.sub_path.is_empty() {
        let suffix = spec
            .sub_path
            .iter()
            .map(|part| part.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("/");
        key.push('/');
        key.push_str(&suffix);
    }

    key
}

fn github_mirror_prefixes() -> Vec<String> {
    let mut prefixes = vec![String::new()];
    if let Ok(raw) = std::env::var("CTENO_GITHUB_MIRROR") {
        for part in raw.split(',') {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                prefixes.push(trimmed.to_string());
            }
        }
    }

    for default_prefix in ["https://ghproxy.com/", "https://mirror.ghproxy.com/"] {
        if !prefixes.iter().any(|item| item == default_prefix) {
            prefixes.push(default_prefix.to_string());
        }
    }

    prefixes
}

fn apply_mirror_prefix(prefix: &str, url: &str) -> String {
    let trimmed = prefix.trim();
    if trimmed.is_empty() {
        return url.to_string();
    }
    if trimmed.ends_with('/') {
        format!("{}{}", trimmed, url)
    } else {
        format!("{}/{}", trimmed, url)
    }
}

async fn download_bytes_with_github_mirror(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<u8>, String> {
    let mut last_error = String::new();
    for prefix in github_mirror_prefixes() {
        let candidate = apply_mirror_prefix(&prefix, url);
        let response = client
            .get(&candidate)
            .header("User-Agent", "CtenoSkillInstaller/1.0")
            .send()
            .await;

        match response {
            Ok(resp) => match resp.error_for_status() {
                Ok(ok_resp) => {
                    return ok_resp.bytes().await.map(|b| b.to_vec()).map_err(|e| {
                        format!("Failed to read response body from {}: {}", candidate, e)
                    });
                }
                Err(err) => {
                    last_error = format!("{} (url: {})", err, candidate);
                }
            },
            Err(err) => {
                last_error = format!("{} (url: {})", err, candidate);
            }
        }
    }

    if last_error.is_empty() {
        Err("No available GitHub URL candidates".to_string())
    } else {
        Err(last_error)
    }
}

fn install_from_zip_bytes(
    zip_bytes: &[u8],
    fallback_name: &str,
    preferred_sub_path: Option<&[String]>,
    github_meta: Option<GithubInstallMeta<'_>>,
) -> Result<SkillInstallResult, String> {
    let install_root = resolve_community_skill_dir()?;
    fs::create_dir_all(&install_root)
        .map_err(|e| format!("Failed to create skill directory {:?}: {}", install_root, e))?;

    let temp_root = std::env::temp_dir().join(format!("cteno-skill-install-{}", Uuid::new_v4()));
    let extract_dir = temp_root.join("extract");
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Failed to create temp extraction directory: {}", e))?;

    extract_zip_bytes(zip_bytes, &extract_dir)?;

    let source_skill_dir = if let Some(sub_path) = preferred_sub_path {
        if sub_path.is_empty() {
            find_skill_dir(&extract_dir)
        } else {
            find_skill_dir_by_sub_path(&extract_dir, sub_path)
                .or_else(|| find_skill_dir(&extract_dir))
        }
    } else {
        find_skill_dir(&extract_dir)
    }
    .ok_or_else(|| "No SKILL.md found in downloaded package".to_string())?;

    let source_name = source_skill_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();

    let mut skill_id = sanitize_skill_id(source_name);
    if skill_id.is_empty() {
        skill_id = sanitize_skill_id(fallback_name);
    }
    if skill_id.is_empty() {
        skill_id = format!("skill_{}", Uuid::new_v4().simple());
    }

    let target_dir = install_root.join(&skill_id);
    if target_dir.exists() {
        fs::remove_dir_all(&target_dir)
            .map_err(|e| format!("Failed to replace existing skill {:?}: {}", target_dir, e))?;
    }

    copy_dir_recursive(&source_skill_dir, &target_dir)?;

    if let Some(meta) = github_meta {
        let source_meta = SkillSourceMeta {
            source_type: "github".to_string(),
            source_key: meta.source_key.to_string(),
            github_url: Some(meta.github_url.to_string()),
            installed_version: read_skill_version_from_dir(&target_dir),
            installed_at: chrono::Utc::now().to_rfc3339(),
        };
        let _ = write_source_meta(&target_dir, &source_meta);
    }

    let result = SkillInstallResult {
        success: true,
        skill_id: skill_id.clone(),
        install_path: target_dir.to_string_lossy().to_string(),
    };

    let _ = fs::remove_dir_all(&temp_root);

    Ok(result)
}

fn resolve_community_skill_dir() -> Result<PathBuf, String> {
    if let Some(home) = dirs::home_dir() {
        return Ok(home.join(".agents").join("skills"));
    }

    Err("Failed to resolve home directory".to_string())
}

fn extract_zip_bytes(zip_bytes: &[u8], extract_dir: &Path) -> Result<(), String> {
    let cursor = Cursor::new(zip_bytes.to_vec());
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to parse zip archive: {}", e))?;

    for idx in 0..archive.len() {
        let mut entry = archive
            .by_index(idx)
            .map_err(|e| format!("Failed to read zip entry: {}", e))?;

        let enclosed = entry
            .enclosed_name()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| format!("Invalid zip entry path: {}", entry.name()))?;

        let out_path = extract_dir.join(enclosed);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("Failed to create directory {:?}: {}", out_path, e))?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory {:?}: {}", parent, e))?;
        }

        let mut out_file = fs::File::create(&out_path)
            .map_err(|e| format!("Failed to create file {:?}: {}", out_path, e))?;
        std::io::copy(&mut entry, &mut out_file)
            .map_err(|e| format!("Failed to extract file {:?}: {}", out_path, e))?;
    }

    Ok(())
}

fn find_skill_dir(root: &Path) -> Option<PathBuf> {
    if root.join("SKILL.md").is_file() {
        return Some(root.to_path_buf());
    }

    let mut queue = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0usize));

    while let Some((dir, depth)) = queue.pop_front() {
        if dir.join("SKILL.md").is_file() {
            return Some(dir);
        }

        if depth >= 4 {
            continue;
        }

        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                queue.push_back((path, depth + 1));
            }
        }
    }

    None
}

fn find_skill_dir_by_sub_path(root: &Path, sub_path: &[String]) -> Option<PathBuf> {
    if sub_path.is_empty() {
        return None;
    }

    let mut queue = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0usize));

    while let Some((dir, depth)) = queue.pop_front() {
        let candidate = sub_path
            .iter()
            .fold(dir.clone(), |acc, part| acc.join(part));
        if candidate.join("SKILL.md").is_file() {
            return Some(candidate);
        }

        if depth >= 4 {
            continue;
        }

        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                queue.push_back((path, depth + 1));
            }
        }
    }

    None
}

fn sanitize_skill_id(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;

    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_sep = false;
        } else if (ch == '-' || ch == '_' || ch == ' ' || ch == '.') && !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }

    out.trim_matches('_').to_string()
}

fn is_skill_markdown_name(raw: &str) -> bool {
    raw.eq_ignore_ascii_case("SKILL.md")
}

fn extract_yaml_frontmatter(content: &str) -> Option<&str> {
    if !content.starts_with("---") {
        return None;
    }

    let rest = &content[3..];
    let end_pos = rest.find("\n---")?;
    Some(&rest[..end_pos])
}

fn extract_skill_version_from_content(content: &str) -> Option<String> {
    let yaml = extract_yaml_frontmatter(content)?;
    let value: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    let version = value.get("version")?.as_str()?.trim();
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

fn extract_skill_name_from_content(content: &str) -> Option<String> {
    let yaml = extract_yaml_frontmatter(content)?;
    let value: serde_yaml::Value = serde_yaml::from_str(yaml).ok()?;
    let name = value.get("name")?.as_str()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn read_skill_version_from_dir(skill_dir: &Path) -> Option<String> {
    let content = fs::read_to_string(skill_dir.join("SKILL.md")).ok()?;
    extract_skill_version_from_content(&content)
}

fn parse_numeric_version(version: &str) -> Option<Vec<u64>> {
    let trimmed = version.trim().trim_start_matches('v');
    if trimmed.is_empty() {
        return None;
    }

    let mut out = Vec::new();
    for part in trimmed.split('.') {
        let mut digits = String::new();
        for ch in part.chars() {
            if ch.is_ascii_digit() {
                digits.push(ch);
            } else {
                break;
            }
        }
        if digits.is_empty() {
            return None;
        }
        out.push(digits.parse::<u64>().ok()?);
    }

    Some(out)
}

fn compare_version_strings(local: &str, remote: &str) -> Option<Ordering> {
    let local_parts = parse_numeric_version(local)?;
    let remote_parts = parse_numeric_version(remote)?;
    let max_len = local_parts.len().max(remote_parts.len());
    for idx in 0..max_len {
        let l = *local_parts.get(idx).unwrap_or(&0);
        let r = *remote_parts.get(idx).unwrap_or(&0);
        match l.cmp(&r) {
            Ordering::Equal => {}
            ord => return Some(ord),
        }
    }
    Some(Ordering::Equal)
}

fn source_meta_path(skill_dir: &Path) -> PathBuf {
    skill_dir.join(SOURCE_META_FILE)
}

fn write_source_meta(skill_dir: &Path, meta: &SkillSourceMeta) -> Result<(), String> {
    let content = serde_json::to_string_pretty(meta)
        .map_err(|e| format!("Failed to serialize source meta: {}", e))?;
    fs::write(source_meta_path(skill_dir), content)
        .map_err(|e| format!("Failed to write source meta: {}", e))
}

fn read_source_meta(skill_dir: &Path) -> Option<SkillSourceMeta> {
    let content = fs::read_to_string(source_meta_path(skill_dir)).ok()?;
    serde_json::from_str::<SkillSourceMeta>(&content).ok()
}

struct RemoteSkillMeta {
    version: Option<String>,
    name: Option<String>,
}

async fn fetch_remote_skill_meta(spec: &GithubTreeSpec) -> Option<RemoteSkillMeta> {
    let mut skill_path = spec.sub_path.join("/");
    if !skill_path.is_empty() {
        skill_path.push('/');
    }
    skill_path.push_str("SKILL.md");

    let raw_url = format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        spec.owner, spec.repo, spec.branch, skill_path
    );

    let client = reqwest::Client::new();
    let body_bytes = download_bytes_with_github_mirror(&client, &raw_url)
        .await
        .ok()?;
    let body = String::from_utf8(body_bytes).ok()?;
    Some(RemoteSkillMeta {
        version: extract_skill_version_from_content(&body),
        name: extract_skill_name_from_content(&body),
    })
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target)
        .map_err(|e| format!("Failed to create directory {:?}: {}", target, e))?;

    let entries = fs::read_dir(source)
        .map_err(|e| format!("Failed to read directory {:?}: {}", source, e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());

        if source_path.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path).map_err(|e| {
                format!(
                    "Failed to copy file from {:?} to {:?}: {}",
                    source_path, target_path, e
                )
            })?;
        }
    }

    Ok(())
}
