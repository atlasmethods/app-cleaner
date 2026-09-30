#!/usr/bin/env python3
"""Drive the real Tauri webview through WebKitWebDriver and check the IPC path end to end.

Needs a display (run under `xvfb-run -a`), a binary built with the `custom-protocol` feature
(otherwise the app loads devUrl instead of the embedded frontend) and, for a realistic machine,
the `clearsweep` test binary that has the hidden `dev-fixture` command:

    cargo build -p sweep-cli --features testutil
    cargo build -p clearsweep-desktop --features custom-protocol
    xvfb-run -a python3 scripts/desktop-smoke.py target/debug/clearsweep-desktop [target/debug/clearsweep]

What it checks, in the real webview at 380x640 on a sandbox HOME (fixture machine):
  1. every bottom tab and every Tools tile renders its page root test id with no error banner;
  2. a real action over Tauri IPC with progress: `cleaner.analyze` streams progress events through the
     Channel and returns a report (the Clean page's Analyze button is driven as well);
  3. cancellation over IPC: `api_cancel` stops a long `api.sleep` call with code `Cancelled`;
  4. the smoke checks of earlier phases: Settings sections, headless `call` / `agent --once`, `--hidden`;
  5. without a display the executable falls back to browser mode (serves the UI over HTTP).
"""
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

BIN = os.path.abspath(sys.argv[1])
FIXTURE_BIN = os.path.abspath(sys.argv[2]) if len(sys.argv) > 2 else None
BASE = "http://127.0.0.1:4444"

TABS = ["home", "clean", "tools", "performance", "settings"]
TOOLS = ["uninstall", "updater", "drivers", "startup", "plugins", "disk", "duplicates", "restore",
         "wiper", "shredder", "registry", "sysinfo", "cookies"]
HASHES = [("#/", "page-home"), ("#/clean", "page-clean"), ("#/tools", "page-tools"),
          ("#/performance", "page-performance"), ("#/settings", "page-settings")] + \
         [("#/tools/" + t, "page-" + t) for t in TOOLS]

failures = []


def check(name, cond, detail=""):
    print(("ok   " if cond else "FAIL ") + name + ((" - " + str(detail)[:300]) if detail and not cond else ""))
    if not cond:
        failures.append(name)
    return cond


def req(method, path, body=None):
    r = urllib.request.Request(
        BASE + path,
        method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Content-Type": "application/json"},
    )
    try:
        return json.load(urllib.request.urlopen(r, timeout=90))
    except urllib.error.HTTPError as e:
        print("WebDriver error", e.code, e.read().decode()[:500])
        raise


def script(sid, js, args=None):
    return req("POST", f"/session/{sid}/execute/sync", {"script": js, "args": args or []})["value"]


def script_async(sid, js, args=None):
    """`js` receives its `done` callback as the last argument."""
    return req("POST", f"/session/{sid}/execute/async", {"script": js, "args": args or []})["value"]


def wait_for(sid, js, timeout=20, step=0.25):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        last = script(sid, js)
        if last:
            return last
        time.sleep(step)
    return last


# --------------------------------------------------------------- sandbox machine
tmp = tempfile.mkdtemp(prefix="clearsweep-test-smoke-")
dirs = {
    "HOME": os.path.join(tmp, "home"),
    "XDG_CONFIG_HOME": os.path.join(tmp, "home", ".config"),
    "XDG_CACHE_HOME": os.path.join(tmp, "home", ".cache"),
    "XDG_DATA_HOME": os.path.join(tmp, "home", ".local", "share"),
    "CLEARSWEEP_ROOT": os.path.join(tmp, "root"),
    "CLEARSWEEP_DATA_DIR": os.path.join(tmp, "data"),
    "TMPDIR": os.path.join(tmp, "root", "tmp"),
}
for d in dirs.values():
    os.makedirs(d, exist_ok=True)
if FIXTURE_BIN and os.path.exists(FIXTURE_BIN):
    r = subprocess.run([FIXTURE_BIN, "dev-fixture", tmp], capture_output=True, text=True)
    print("fixture:", "built" if r.returncode == 0 else "FAILED " + r.stderr[:200])
else:
    print("fixture: skipped (no clearsweep test binary given); the machine is empty")
env = dict(os.environ, **dirs, CLEARSWEEP_FAKE_PROCESSES="", CLEARSWEEP_TEST_IGNORE_CTIME="1", TAURI_WEBVIEW_AUTOMATION="true")  # wry: allow WebKit automation
os.environ.update(env)

driver = subprocess.Popen(["WebKitWebDriver", "--port=4444"])
try:
    time.sleep(2)
    caps = {"capabilities": {"alwaysMatch": {"webkitgtk:browserOptions": {"binary": BIN, "args": []}}}}
    sid = req("POST", "/session", caps)["value"]["sessionId"]
    time.sleep(3)
    try:
        req("POST", f"/session/{sid}/window/rect", {"width": 380, "height": 640})
    except Exception as e:  # some drivers refuse to resize; the window starts at 380x640 anyway
        print("could not resize the window:", e)
    time.sleep(1)
    size = script(sid, "return [window.innerWidth, window.innerHeight]")
    print("viewport:", size)
    check("running inside Tauri", bool(script(sid, "return '__TAURI_INTERNALS__' in window")))
    check("window is phone sized (compact layout)", size[0] <= 420 and bool(script(sid, "return !!document.querySelector('[data-testid=tabbar]')")), size)

    # ---- 1. every tab and every tool
    for h, root in HASHES:
        script(sid, f"location.hash = '{h}'")
        found = wait_for(sid, f"return !!document.querySelector('[data-testid=\"{root}\"]')", timeout=20)
        time.sleep(0.8)  # let the first API call of the page settle
        banner = script(sid, "var b = document.querySelector('[data-testid=error-banner]'); return b ? b.innerText : null")
        no_hscroll = script(sid, "return document.documentElement.scrollWidth <= window.innerWidth")
        check(f"{h} renders {root}", bool(found))
        check(f"{h} has no error banner", banner is None, banner)
        check(f"{h} does not scroll sideways", bool(no_hscroll))

    script(sid, "location.hash='#/tools/sysinfo'")
    wait_for(sid, "return document.body.innerText.indexOf('logical cores') >= 0")
    check("sysinfo.get over IPC shows the CPU card", "logical cores" in script(sid, "return document.body.innerText"))
    script(sid, "location.hash='#/settings'")
    wait_for(sid, "return document.body.innerText.toLowerCase().indexOf('close to tray') >= 0")
    st = script(sid, "return document.body.innerText").lower()
    check("settings page shows Smart Cleaning and Close to tray", "smart cleaning" in st and "close to tray" in st)

    # ---- 2. a real action over IPC with progress
    ipc = """
      var done = arguments[arguments.length - 1];
      var method = arguments[0], params = arguments[1], callId = arguments[2], cancelAfter = arguments[3];
      var events = [];
      var id = window.__TAURI_INTERNALS__.transformCallback(function (m) { events.push(m); });
      var t0 = Date.now();
      var p = window.__TAURI_INTERNALS__.invoke('api_call', {
        callId: callId, method: method, params: params, onProgress: '__CHANNEL__:' + id });
      if (cancelAfter > 0) setTimeout(function () {
        window.__TAURI_INTERNALS__.invoke('api_cancel', { callId: callId });
      }, cancelAfter);
      p.then(function (v) { done({ ok: true, value: v, events: events, ms: Date.now() - t0 }); },
             function (e) { done({ ok: false, error: e, events: events, ms: Date.now() - t0 }); });
    """
    r = script_async(sid, ipc, ["cleaner.analyze", {}, "smoke-analyze", 0])
    progress = [e for e in r["events"] if "stage" in json.dumps(e)]
    check("cleaner.analyze succeeds over IPC", r["ok"], r)
    check("cleaner.analyze streams progress events through the Channel", len(progress) > 0, r["events"][:3])
    if r["ok"]:
        has_junk = FIXTURE_BIN is None or r["value"]["totalBytes"] > 0
        check("the report found the fixture's junk", has_junk, r["value"].get("totalBytes"))

    # the Clean page drives the same path from the UI
    script(sid, "location.hash='#/clean'")
    wait_for(sid, "return !!document.querySelector('[data-testid=btn-analyze]') && !document.querySelector('[data-testid=btn-analyze]').disabled")
    script(sid, """
      window.__seen = [];
      new MutationObserver(function () {
        var m = document.querySelector('[data-testid=clean-progress-message]');
        if (m) window.__seen.push(m.textContent);
      }).observe(document.body, { childList: true, subtree: true, characterData: true });
      document.querySelector('[data-testid=btn-analyze]').click();
    """)
    ok = wait_for(sid, "return !!document.querySelector('[data-testid=btn-clean]') && !document.querySelector('[data-testid=btn-clean]').disabled", timeout=60)
    seen = script(sid, "return window.__seen")
    check("Analyze on the Clean page shows its progress and then the results", bool(seen) and (bool(ok) or FIXTURE_BIN is None), seen[:3])
    check("Analyze on the Clean page raised no error banner", script(sid, "return !document.querySelector('[data-testid=error-banner]')"))

    # ---- 3. cancel over IPC
    r = script_async(sid, ipc, ["api.sleep", {"ms": 20000}, "smoke-sleep", 400])
    code = (r.get("error") or {}).get("code") if isinstance(r.get("error"), dict) else r.get("error")
    check("api_cancel stops a running call", (not r["ok"]) and code == "Cancelled" and r["ms"] < 10000, r)

    req("DELETE", f"/session/{sid}")
finally:
    driver.terminate()
    try:
        driver.wait(timeout=10)
    except subprocess.TimeoutExpired:
        driver.kill()

# ---- 4. the desktop executable doubles as the headless CLI (OS schedulers and autostart launch it)
cli_env = dict(os.environ, **dirs, CLEARSWEEP_FAKE_PROCESSES="", CLEARSWEEP_TEST_IGNORE_CTIME="1")
r = subprocess.run([BIN, "call", "sysinfo.get"], env=cli_env, capture_output=True, text=True, timeout=60)
check("headless `call` works without a window", r.returncode == 0 and '"cores"' in r.stdout, r.stderr)
r = subprocess.run([BIN, "agent", "--once"], env=cli_env, capture_output=True, text=True, timeout=120)
check("headless `agent --once` works", r.returncode == 0, r.stderr)

# --hidden: starts, stays up (tray or not), exits on SIGTERM.
p = subprocess.Popen([BIN, "--hidden"], env=cli_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
time.sleep(5)
hidden_ok = p.poll() is None
p.terminate()
try:
    _, err = p.communicate(timeout=15)
except subprocess.TimeoutExpired:
    p.kill()
    _, err = p.communicate()
check("--hidden stays up", hidden_ok, err.decode(errors="replace")[:300])
check("--hidden does not panic", "panicked" not in err.decode(errors="replace"))

# ---- 5. no display / no WebView: browser-mode fallback
fb_env = dict(cli_env, BROWSER="true")  # never open a real browser here
for k in ("DISPLAY", "WAYLAND_DISPLAY"):
    fb_env.pop(k, None)
p = subprocess.Popen([BIN], env=fb_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
url = None
end = time.time() + 20
while time.time() < end and url is None:
    line = p.stdout.readline()
    if "ClearSweep running at " in line:
        url = line.split("running at ", 1)[1].strip()
    elif p.poll() is not None:
        break
check("without a display the desktop executable starts browser mode", url is not None, p.stderr.read() if p.poll() is not None else "")
if url:
    try:
        body = urllib.request.urlopen(url, timeout=10).read().decode()
        check("fallback serves the frontend with the token link", "<div id=\"root\">" in body)
    except Exception as e:
        check("fallback serves the frontend with the token link", False, e)
p.send_signal(signal.SIGINT)
try:
    p.wait(timeout=10)
except subprocess.TimeoutExpired:
    p.kill()

shutil.rmtree(tmp, ignore_errors=True)
print("IPC_OK" if not failures else "IPC_FAIL: " + "; ".join(failures))
sys.exit(0 if not failures else 1)
