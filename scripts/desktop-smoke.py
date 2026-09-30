#!/usr/bin/env python3
"""Drive the real Tauri webview through WebKitWebDriver and check the IPC path end to end.

Needs a display (run under `xvfb-run -a`) and a binary built with the `custom-protocol` feature
(otherwise the app loads devUrl instead of the embedded frontend):

    cargo build -p clearsweep-desktop --features custom-protocol
    xvfb-run -a python3 scripts/desktop-smoke.py target/debug/clearsweep-desktop

Opens #/tools/sysinfo (which calls `sysinfo.get` through Tauri IPC) and asserts the CPU card rendered,
then opens #/settings and checks the Smart Cleaning section (whose agent status goes through IPC too).

Two more checks need no WebDriver: the executable runs the headless commands (`call`, `agent --once`)
without opening a window, and `--hidden` starts (minimised to the tray when one exists) without crashing.
"""
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

BIN = os.path.abspath(sys.argv[1])
BASE = "http://127.0.0.1:4444"


def req(method, path, body=None):
    r = urllib.request.Request(
        BASE + path,
        method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Content-Type": "application/json"},
    )
    try:
        return json.load(urllib.request.urlopen(r, timeout=60))
    except urllib.error.HTTPError as e:
        print("WebDriver error", e.code, e.read().decode()[:500])
        raise


def script(sid, js):
    return req("POST", f"/session/{sid}/execute/sync", {"script": js, "args": []})["value"]


os.environ["TAURI_WEBVIEW_AUTOMATION"] = "true"  # makes wry allow WebKit automation
driver = subprocess.Popen(["WebKitWebDriver", "--port=4444"])
ok = False
try:
    time.sleep(2)
    caps = {"capabilities": {"alwaysMatch": {"webkitgtk:browserOptions": {"binary": BIN, "args": []}}}}
    sid = req("POST", "/session", caps)["value"]["sessionId"]
    time.sleep(3)
    script(sid, "location.hash='#/tools/sysinfo'")
    time.sleep(3)
    text = script(sid, "return document.body.innerText")
    print(text)
    ok = "logical cores" in text and bool(script(sid, "return '__TAURI_INTERNALS__' in window"))
    script(sid, "location.hash='#/settings'")
    time.sleep(3)
    settings_text = script(sid, "return document.body.innerText")
    smart_ok = "smart cleaning" in settings_text.lower() and "close to tray" in settings_text.lower()
    print("settings page ok:", smart_ok)
    ok = ok and smart_ok
    req("DELETE", f"/session/{sid}")
finally:
    driver.terminate()

# The desktop executable doubles as the headless CLI (OS schedulers and autostart launch it).
sandbox = os.path.join(os.environ.get("TMPDIR", "/tmp"), "clearsweep-smoke-%d" % os.getpid())
env = dict(os.environ, CLEARSWEEP_DATA_DIR=sandbox, CLEARSWEEP_FAKE_PROCESSES="")
r = subprocess.run([BIN, "call", "sysinfo.get"], env=env, capture_output=True, text=True, timeout=60)
cli_ok = r.returncode == 0 and '"cores"' in r.stdout
print("headless call ok:", cli_ok, r.stderr[:200])
r = subprocess.run([BIN, "agent", "--once"], env=env, capture_output=True, text=True, timeout=120)
agent_ok = r.returncode == 0
print("headless agent --once ok:", agent_ok, r.stderr[:200])
ok = ok and cli_ok and agent_ok

# --hidden: starts, stays up (tray or not), exits on SIGTERM.
p = subprocess.Popen([BIN, "--hidden"], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
time.sleep(5)
hidden_ok = p.poll() is None
p.terminate()
try:
    _, err = p.communicate(timeout=15)
except subprocess.TimeoutExpired:
    p.kill()
    _, err = p.communicate()
print("--hidden stayed up:", hidden_ok, err.decode(errors="replace")[:300])
ok = ok and hidden_ok and "panicked" not in err.decode(errors="replace")
print("IPC_OK" if ok else "IPC_FAIL")
sys.exit(0 if ok else 1)
