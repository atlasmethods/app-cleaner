#!/usr/bin/env python3
"""Drive the real Tauri webview through WebKitWebDriver and check the IPC path end to end.

Needs a display (run under `xvfb-run -a`) and a binary built with the `custom-protocol` feature
(otherwise the app loads devUrl instead of the embedded frontend):

    cargo build -p clearsweep-desktop --features custom-protocol
    xvfb-run -a python3 scripts/desktop-smoke.py target/debug/clearsweep-desktop

Opens #/tools/sysinfo (which calls `sysinfo.get` through Tauri IPC) and asserts the CPU card rendered.
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
    req("DELETE", f"/session/{sid}")
finally:
    driver.terminate()
print("IPC_OK" if ok else "IPC_FAIL")
sys.exit(0 if ok else 1)
