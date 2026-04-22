"""
CDP Browser Automation Base Module

Provides Chrome profile copying, CDP connection, and common operations.
Usage:

    from cdp import CDPBrowser

    async def main():
        async with CDPBrowser() as browser:
            sid = await browser.new_page("https://example.com")
            title = await browser.evaluate("document.title", sid=sid)
            print(title)
            await browser.screenshot("/tmp/screenshot.png", sid=sid)

    asyncio.run(main())
"""

import asyncio
import base64
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.request

try:
    import websockets
except ImportError:
    subprocess.check_call([sys.executable, "-m", "pip", "install", "websockets", "-q"])
    import websockets


def _log(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", file=sys.stderr, flush=True)


def _find_chrome():
    if sys.platform == "darwin":
        path = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        if os.path.exists(path):
            return path
    elif sys.platform == "win32":
        for p in [
            os.path.expandvars(r"%ProgramFiles%\Google\Chrome\Application\chrome.exe"),
            os.path.expandvars(r"%ProgramFiles(x86)%\Google\Chrome\Application\chrome.exe"),
            os.path.expandvars(r"%LocalAppData%\Google\Chrome\Application\chrome.exe"),
        ]:
            if os.path.exists(p):
                return p
    else:
        for name in ["google-chrome", "google-chrome-stable", "chromium-browser", "chromium"]:
            if shutil.which(name):
                return name
    raise FileNotFoundError("Google Chrome not found")


def _default_profile_dir():
    if sys.platform == "darwin":
        return os.path.expanduser("~/Library/Application Support/Google/Chrome")
    elif sys.platform == "win32":
        return os.path.expandvars(r"%LocalAppData%\Google\Chrome\User Data")
    return os.path.expanduser("~/.config/google-chrome")


# Files/dirs to copy from Chrome profile for login state preservation
_PROFILE_ITEMS = [
    "Default/Cookies",
    "Default/Login Data",
    "Default/Web Data",
    "Default/Preferences",
    "Default/Network",
    "Local State",
]


class CDPBrowser:
    """CDP browser automation with automatic profile copying and cleanup.

    Args:
        port: CDP debugging port (default: 9222)
        profile_dir: Chrome user-data-dir to copy from (default: system Chrome)
        headless: Run in headless mode (default: False)
        window_size: Window dimensions as "WxH" (default: "1280,900")
    """

    def __init__(self, port=9222, profile_dir=None, headless=False, window_size="1280,900"):
        self.port = port
        self.profile_dir = profile_dir or _default_profile_dir()
        self.headless = headless
        self.window_size = window_size
        self._ws = None
        self._msg_id = 0
        self._tmp_profile = None
        self._chrome_proc = None
        self._event_listeners = {}  # method -> [callback]
        self._pending_events = []

    async def __aenter__(self):
        await self.start()
        return self

    async def __aexit__(self, *exc):
        await self.cleanup()

    async def start(self):
        """Copy profile, launch Chrome, connect CDP."""
        self._copy_profile()
        self._launch_chrome()
        await self._wait_for_cdp()
        await self._connect()
        # Enable auto-attach for multi-tab support
        await self.send("Target.setAutoAttach", {
            "autoAttach": True,
            "waitForDebuggerOnStart": False,
            "flatten": True,
        })
        _log("Browser ready.")

    async def cleanup(self):
        """Close CDP, terminate Chrome, remove temp profile."""
        if self._ws:
            try:
                await self._ws.close()
            except Exception:
                pass
            self._ws = None
        if self._chrome_proc:
            self._chrome_proc.terminate()
            try:
                self._chrome_proc.wait(timeout=5)
            except Exception:
                self._chrome_proc.kill()
            self._chrome_proc = None
        if self._tmp_profile and os.path.exists(self._tmp_profile):
            shutil.rmtree(self._tmp_profile, ignore_errors=True)
            self._tmp_profile = None
        _log("Cleanup done.")

    # ── Profile management ──────────────────────────────────────────

    def _copy_profile(self):
        import tempfile
        self._tmp_profile = os.path.join(tempfile.gettempdir(), f"cdp_automation_{os.getpid()}")
        _log(f"Copying profile to {self._tmp_profile} ...")
        os.makedirs(f"{self._tmp_profile}/Default", exist_ok=True)
        for item in _PROFILE_ITEMS:
            src = os.path.join(self.profile_dir, item)
            dst = os.path.join(self._tmp_profile, item)
            if os.path.isdir(src):
                shutil.copytree(src, dst, dirs_exist_ok=True)
            elif os.path.isfile(src):
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                shutil.copy2(src, dst)
        _log("Profile copied.")

    # ── Chrome launch ───────────────────────────────────────────────

    def _launch_chrome(self):
        args = [
            _find_chrome(),
            f"--remote-debugging-port={self.port}",
            f"--user-data-dir={self._tmp_profile}",
            "--no-first-run",
            "--no-default-browser-check",
            f"--window-size={self.window_size}",
        ]
        if self.headless:
            args.append("--headless=new")
        _log("Launching Chrome ...")
        self._chrome_proc = subprocess.Popen(
            args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
        )

    # ── CDP connection ──────────────────────────────────────────────

    async def _wait_for_cdp(self, timeout=15):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json/version")
                return
            except Exception:
                await asyncio.sleep(0.5)
        raise TimeoutError("Chrome CDP not ready")

    async def _connect(self):
        resp = json.loads(
            urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json/version").read()
        )
        self._ws = await websockets.connect(
            resp["webSocketDebuggerUrl"], max_size=50 * 1024 * 1024
        )
        _log("CDP connected.")

    # ── Low-level CDP commands ──────────────────────────────────────

    async def send(self, method, params=None, sid=None):
        """Send a CDP command and wait for its response."""
        self._msg_id += 1
        msg = {"id": self._msg_id, "method": method, "params": params or {}}
        if sid:
            msg["sessionId"] = sid
        await self._ws.send(json.dumps(msg))
        target_id = self._msg_id
        while True:
            raw = await self._ws.recv()
            r = json.loads(raw)
            if r.get("id") == target_id:
                if "error" in r:
                    raise RuntimeError(f"CDP error in {method}: {r['error']}")
                return r.get("result", {})
            # Collect events while waiting
            if "method" in r and not r.get("id"):
                self._pending_events.append(r)

    # ── High-level helpers ──────────────────────────────────────────

    async def new_page(self, url, wait=3):
        """Navigate current page to url. Returns session ID."""
        targets = await self.send("Target.getTargets")
        page_target = None
        for t in targets.get("targetInfos", []):
            if t.get("type") == "page":
                page_target = t
                break
        if not page_target:
            # Create a new tab
            result = await self.send("Target.createTarget", {"url": url})
            target_id = result["targetId"]
        else:
            target_id = page_target["targetId"]

        attach = await self.send("Target.attachToTarget", {
            "targetId": target_id, "flatten": True
        })
        sid = attach["sessionId"]
        await self.send("Page.enable", sid=sid)

        if page_target:
            await self.send("Page.navigate", {"url": url}, sid=sid)

        if wait > 0:
            await asyncio.sleep(wait)
        return sid

    async def evaluate(self, expression, sid=None, return_by_value=True):
        """Execute JS expression and return result value."""
        result = await self.send("Runtime.evaluate", {
            "expression": expression,
            "returnByValue": return_by_value,
        }, sid=sid)
        r = result.get("result", {})
        if r.get("type") == "undefined":
            return None
        if "value" in r:
            return r["value"]
        return r

    async def click(self, selector, sid=None):
        """Click an element by CSS selector using real mouse events."""
        pos = await self.evaluate(f"""
            (() => {{
                const el = document.querySelector({json.dumps(selector)});
                if (!el) return null;
                const r = el.getBoundingClientRect();
                return {{x: r.x + r.width/2, y: r.y + r.height/2}};
            }})()
        """, sid=sid)
        if not pos:
            raise RuntimeError(f"Element not found: {selector}")
        x, y = pos["x"], pos["y"]
        await self.send("Input.dispatchMouseEvent", {
            "type": "mousePressed", "x": x, "y": y,
            "button": "left", "clickCount": 1
        }, sid=sid)
        await self.send("Input.dispatchMouseEvent", {
            "type": "mouseReleased", "x": x, "y": y,
            "button": "left", "clickCount": 1
        }, sid=sid)

    async def type_text(self, selector, text, sid=None):
        """Type text into a React-compatible input field."""
        await self.evaluate(f"""
            (() => {{
                const el = document.querySelector({json.dumps(selector)});
                if (!el) throw new Error('Element not found: {selector}');
                el.focus();
                const setter = Object.getOwnPropertyDescriptor(
                    el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype,
                    'value'
                )?.set;
                if (setter) {{
                    setter.call(el, {json.dumps(text)});
                }} else {{
                    el.value = {json.dumps(text)};
                }}
                el.dispatchEvent(new Event('input', {{bubbles: true}}));
                el.dispatchEvent(new Event('change', {{bubbles: true}}));
            }})()
        """, sid=sid)

    async def wait_for(self, selector, sid=None, timeout=10, interval=0.5):
        """Wait for an element to appear in the DOM."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            found = await self.evaluate(f"""
                !!document.querySelector({json.dumps(selector)})
            """, sid=sid)
            if found:
                return True
            await asyncio.sleep(interval)
        raise TimeoutError(f"Element not found after {timeout}s: {selector}")

    async def upload_file(self, trigger_selector, file_paths, sid=None):
        """Upload file(s) by clicking a trigger element and intercepting the file chooser.

        Strategy:
        1. Enable Page.setInterceptFileChooserDialog
        2. Click trigger with real mouse events (JS .click() often doesn't trigger file dialogs)
        3. Wait for Page.fileChooserOpened event → get backendNodeId
        4. Use DOM.setFileInputFiles to set files (more compatible than Page.handleFileChooser)
        5. Disable interception

        IMPORTANT: The trigger_selector must point to the actual clickable element (often a
        leaf-level <span> with cursor:pointer), not a wrapper div. Use get_visible_text_elements()
        or browser DevTools to find the right element.

        Args:
            trigger_selector: CSS selector for the element that opens the file dialog.
            file_paths: A single file path string, or a list of file paths.
            sid: CDP session ID.
        """
        if isinstance(file_paths, str):
            file_paths = [file_paths]
        abs_paths = [os.path.abspath(p) for p in file_paths]
        for p in abs_paths:
            if not os.path.exists(p):
                raise FileNotFoundError(f"File not found: {p}")

        await self.send("Page.setInterceptFileChooserDialog", {"enabled": True}, sid=sid)
        try:
            # Get click position
            pos = await self.evaluate(f"""
                (() => {{
                    const el = document.querySelector({json.dumps(trigger_selector)});
                    if (!el) return null;
                    const r = el.getBoundingClientRect();
                    return {{x: r.x + r.width/2, y: r.y + r.height/2}};
                }})()
            """, sid=sid)
            if not pos:
                raise RuntimeError(f"Upload trigger not found: {trigger_selector}")

            # Send real mouse click (press + release) without awaiting recv in between
            x, y = pos["x"], pos["y"]
            self._msg_id += 1
            press_id = self._msg_id
            await self._ws.send(json.dumps({
                "id": press_id, "method": "Input.dispatchMouseEvent",
                "params": {"type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1},
                **({"sessionId": sid} if sid else {})
            }))
            self._msg_id += 1
            release_id = self._msg_id
            await self._ws.send(json.dumps({
                "id": release_id, "method": "Input.dispatchMouseEvent",
                "params": {"type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1},
                **({"sessionId": sid} if sid else {})
            }))

            # Wait for fileChooserOpened event
            deadline = time.time() + 10
            backend_node_id = None
            while time.time() < deadline:
                try:
                    raw = await asyncio.wait_for(self._ws.recv(), timeout=5)
                    r = json.loads(raw)
                    if r.get("method") == "Page.fileChooserOpened":
                        backend_node_id = r["params"]["backendNodeId"]
                        break
                    if "method" in r and not r.get("id"):
                        self._pending_events.append(r)
                except asyncio.TimeoutError:
                    break

            if not backend_node_id:
                raise TimeoutError("File chooser did not open after clicking trigger")

            # Set files via DOM.setFileInputFiles (more compatible than Page.handleFileChooser)
            await self.send("DOM.enable", sid=sid)
            await self.send("DOM.setFileInputFiles", {
                "files": abs_paths,
                "backendNodeId": backend_node_id,
            }, sid=sid)
            _log(f"File uploaded: {abs_paths}")
        finally:
            try:
                await self.send("Page.setInterceptFileChooserDialog", {"enabled": False}, sid=sid)
            except Exception:
                pass

    async def screenshot(self, save_path="/tmp/screenshot.png", sid=None):
        """Take a screenshot and save to file. Returns the file path."""
        result = await self.send("Page.captureScreenshot", {"format": "png"}, sid=sid)
        with open(save_path, "wb") as f:
            f.write(base64.b64decode(result["data"]))
        _log(f"Screenshot saved: {save_path}")
        return save_path

    async def get_cookies(self, urls=None, sid=None):
        """Get browser cookies, optionally filtered by URLs."""
        params = {}
        if urls:
            params["urls"] = urls if isinstance(urls, list) else [urls]
        result = await self.send("Network.getCookies", params, sid=sid)
        return result.get("cookies", [])

    async def scroll_to_bottom(self, sid=None, step=500, delay=0.3):
        """Scroll page to bottom incrementally."""
        while True:
            result = await self.evaluate("""
                (() => {
                    const before = window.scrollY;
                    window.scrollBy(0, %d);
                    return { before, after: window.scrollY, max: document.body.scrollHeight - window.innerHeight };
                })()
            """ % step, sid=sid)
            if result["after"] >= result["max"] or result["after"] == result["before"]:
                break
            await asyncio.sleep(delay)

    # ── DOM inspection helpers ────────────────────────────────────

    async def get_visible_text_elements(self, sid=None, max_count=50):
        """Get all visible text elements with position info. Useful for understanding page layout."""
        return await self.evaluate("""
            (() => {
                const all = document.querySelectorAll('div, span, a, button, li, h1, h2, h3, h4, label, p');
                const results = [];
                for (const el of all) {
                    const directText = Array.from(el.childNodes)
                        .filter(n => n.nodeType === 3)
                        .map(n => n.textContent.trim())
                        .join('');
                    if (directText && directText.length > 0 && directText.length < 50) {
                        const rect = el.getBoundingClientRect();
                        if (rect.width > 0 && rect.height > 0 && rect.top < window.innerHeight) {
                            results.push({
                                tag: el.tagName, text: directText,
                                x: Math.round(rect.x), y: Math.round(rect.y),
                                w: Math.round(rect.width), h: Math.round(rect.height),
                                className: el.className?.toString().substring(0, 60) || ''
                            });
                        }
                    }
                    if (results.length >= %d) break;
                }
                return results;
            })()
        """ % max_count, sid=sid)

    async def get_form_elements(self, sid=None):
        """Get all interactive form elements (inputs, textareas, buttons, editors)."""
        return await self.evaluate("""
            (() => {
                const results = { inputs: [], buttons: [], editors: [] };
                // Inputs & textareas
                document.querySelectorAll('input:not([type="hidden"]), textarea').forEach(el => {
                    const r = el.getBoundingClientRect();
                    if (r.width > 0) results.inputs.push({
                        tag: el.tagName, type: el.type || '', placeholder: el.placeholder || '',
                        className: el.className.substring(0, 80),
                        x: Math.round(r.x), y: Math.round(r.y)
                    });
                });
                // Buttons
                document.querySelectorAll('button, [role="button"]').forEach(el => {
                    const text = el.textContent.trim();
                    if (text.length > 0 && text.length < 30) {
                        results.buttons.push({
                            text, className: el.className.substring(0, 80), disabled: el.disabled
                        });
                    }
                });
                // Contenteditable editors
                document.querySelectorAll('[contenteditable="true"]').forEach(el => {
                    const r = el.getBoundingClientRect();
                    results.editors.push({
                        tag: el.tagName, className: el.className.substring(0, 80),
                        x: Math.round(r.x), y: Math.round(r.y),
                        w: Math.round(r.width), h: Math.round(r.height)
                    });
                });
                return results;
            })()
        """, sid=sid)

    async def click_by_text(self, text, tag="*", sid=None):
        """Click the first visible element matching exact text content."""
        pos = await self.evaluate(f"""
            (() => {{
                const els = document.querySelectorAll({json.dumps(tag)});
                for (const el of els) {{
                    if (el.textContent.trim() === {json.dumps(text)}) {{
                        const r = el.getBoundingClientRect();
                        if (r.width > 0 && r.height > 0)
                            return {{x: r.x + r.width/2, y: r.y + r.height/2}};
                    }}
                }}
                return null;
            }})()
        """, sid=sid)
        if not pos:
            raise RuntimeError(f"Element with text '{text}' not found")
        x, y = pos["x"], pos["y"]
        await self.send("Input.dispatchMouseEvent", {
            "type": "mousePressed", "x": x, "y": y,
            "button": "left", "clickCount": 1
        }, sid=sid)
        await self.send("Input.dispatchMouseEvent", {
            "type": "mouseReleased", "x": x, "y": y,
            "button": "left", "clickCount": 1
        }, sid=sid)

    async def type_into_contenteditable(self, selector, text, sid=None):
        """Type text into a contenteditable editor (ProseMirror, TipTap, Draft.js, etc).

        Unlike type_text() which uses React value setter, this uses keyboard dispatch
        which works with rich text editors that listen to input/keydown events.
        """
        await self.click(selector, sid=sid)
        await asyncio.sleep(0.2)
        # Use Input.insertText for bulk text input — works with most editors
        await self.send("Input.insertText", {"text": text}, sid=sid)

    async def wait_for_url_change(self, old_url, sid=None, timeout=15):
        """Wait until the page URL changes from old_url."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            url = await self.evaluate("location.href", sid=sid)
            if url != old_url:
                return url
            await asyncio.sleep(0.5)
        raise TimeoutError(f"URL did not change from {old_url} after {timeout}s")

    async def dismiss_dialogs(self, button_texts=None, sid=None):
        """Dismiss modal dialogs/toasts by clicking buttons with matching text."""
        texts = button_texts or ["我知道了", "确定", "关闭", "OK", "Got it"]
        return await self.evaluate(f"""
            (() => {{
                const targets = {json.dumps(texts)};
                let dismissed = 0;
                document.querySelectorAll('button').forEach(btn => {{
                    if (targets.includes(btn.textContent.trim())) {{
                        btn.click();
                        dismissed++;
                    }}
                }});
                return dismissed;
            }})()
        """, sid=sid)
