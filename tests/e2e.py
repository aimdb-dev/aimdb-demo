#!/usr/bin/env python3
"""The demo end to end on the example pages: the hub page and node pages in
separate headless browsers, joined through bus-forwarder (the relay's
/bus/<code> stand-in). Standard library only; drives ChromeDriver directly.

Needs: scripts/build.sh run first, CHROMEDRIVER (default: chromedriver on
PATH). Run from anywhere:
    python3 tests/e2e.py [path/to/bus-forwarder]
BUS_URL=ws://host:port/bus runs against another bus server (e.g. a relay)
instead of starting the forwarder.
"""
import json, os, subprocess, sys, time, urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
FWD = sys.argv[1] if len(sys.argv) > 1 else str(TARGET / "debug/bus-forwarder")
DRIVER = os.environ.get("CHROMEDRIVER", "chromedriver")
HTTP, BUS, WD = 8098, 9312, 9516
BUS_URL = os.environ.get("BUS_URL", f"ws://127.0.0.1:{BUS}/bus")
CODE = "e2e-example-" + str(int(time.time()))

procs = []
def start(*cmd, **kw):
    p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **kw)
    procs.append(p)
    return p

def wd(method, path, body=None):
    req = urllib.request.Request(f"http://127.0.0.1:{WD}{path}", method=method,
                                 data=json.dumps(body).encode() if body is not None else None,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())["value"]

class Browser:
    def __init__(self):
        caps = {"capabilities": {"alwaysMatch": {"browserName": "chrome", "goog:chromeOptions": {
            "args": ["--headless=new", "--no-sandbox", "--disable-dev-shm-usage"]}}}}
        self.id = wd("POST", "/session", caps)["sessionId"]
    def go(self, url): wd("POST", f"/session/{self.id}/url", {"url": url})
    def js(self, script, *args):
        return wd("POST", f"/session/{self.id}/execute/sync", {"script": script, "args": list(args)})
    def until(self, what, script, timeout=15):
        end = time.time() + timeout
        while time.time() < end:
            try:
                v = self.js(script)
                if v: return v
            except Exception:
                pass
            time.sleep(0.1)
        raise AssertionError(f"timed out: {what}")
    def quit(self): wd("DELETE", f"/session/{self.id}")

checks = []
def check(name, ok, detail=""):
    checks.append((name, ok))
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail}]" if detail else ""))

# The newest frame from `place` on the hub that satisfies `cond` (JS).
def newest(hub, place, cond="true"):
    return hub.until(f"frame from {place}", f"""
      const f = (window.lastView?.frames ?? []).find(f => f.place === {json.dumps(place)});
      return f && ({cond}) && f""")

browsers = []
def browser():
    b = Browser()
    browsers.append(b)
    return b

try:
    if "BUS_URL" not in os.environ:
        start(FWD, str(BUS))
    start(sys.executable, "-m", "http.server", str(HTTP), "--bind", "127.0.0.1", "--directory", str(ROOT))
    start(DRIVER, f"--port={WD}")
    time.sleep(1.5)

    hub = browser()
    hub.go(f"http://127.0.0.1:{HTTP}/examples/hub.html?code={CODE}&bus={BUS_URL}")
    hub.until("hub online", "return window.lastView?.link === 'Open'")
    link = lambda n: hub.js(f"return document.querySelector('[data-place=\"{n}\"]').href")

    # A node sends every second on its own, @2 by default.
    n1 = browser()
    n1.go(link(1))
    f = newest(hub, "1")
    check("node 1 @2 is admitted", f["verdict"] == "accepted", f["verdict"])
    ans = n1.until("answer", "const t = document.getElementById('answer').textContent; return t.includes('accepted') && t")
    check("node 1 is told: accepted, 326 W", '"admitted_w": 326' in ans, ans.splitlines()[0])

    # Every firmware gets its own verdict.
    for fw, want in [("v1", "upgraded"), ("v3", "too_new"), ("unbumped", "shape_mismatch"), ("none", "malformed")]:
        n1.js(f"const s = document.getElementById('fw'); s.value = '{fw}'; s.dispatchEvent(new Event('change'))")
        f = hub.until(f"{fw} verdict", f"""
          const f = (window.lastView?.frames ?? []).find(f => f.place === '1');
          return f && f.verdict === {json.dumps(want)} && f""", timeout=10)
        check(f"node 1 {fw}: {want}", True)

    # A second browser for place 1 is refused.
    late = browser()
    late.go(link(1))
    st = late.until("refusal", "const s = window.node?.linkState(); return s && s !== 'Connecting' && s")
    check("a taken place is refused (4009)", st == "Taken", st)

    # A raw socket at place 3: it cannot send as place 1, its own frame is
    # checked, and it receives nothing but its own feedback and the mode.
    late.js(f"""
      const ws = new WebSocket('{BUS_URL}/{CODE}?as=3');
      ws.binaryType = 'arraybuffer'; window.rxTopics = [];
      ws.onmessage = e => {{ const b = new Uint8Array(e.data); window.rxTopics.push(new TextDecoder().decode(b.slice(1, 1 + b[0]))); }};
      const frame = (topic, body) => {{
        const t = new TextEncoder().encode(topic), b = new TextEncoder().encode(body);
        const out = new Uint8Array(1 + t.length + b.length);
        out[0] = t.length; out.set(t, 1); out.set(b, 1 + t.length); return out;
      }};
      ws.onopen = () => {{
        ws.send(frame('1/power', '{{"power_mw":999000}}'));
        setTimeout(() => ws.send(frame('3/power', '{{"power_mw":1000}}')), 300);
      }};""")
    f3 = newest(hub, "3")
    check("place 3's own plain JSON is rejected malformed", f3["verdict"] == "malformed", f3["verdict"])
    time.sleep(0.5)
    forged = hub.js("return (window.lastView?.frames ?? []).filter(f => f.place === '1' && (f.text ?? '').includes('999000')).length")
    check("place 3 cannot send as place 1", forged == 0, f"{forged} forged frames")

    # Contracts off: node 1 goes back to @2 and its readings are read three ways.
    n1.js("const s = document.getElementById('fw'); s.value = 'v2'; s.dispatchEvent(new Event('change'))")
    hub.until("v2 again", "const f = (window.lastView?.frames ?? []).find(f => f.place === '1'); return f && f.verdict === 'accepted'")
    hub.js("document.getElementById('contracts').click()")
    f = hub.until("raw frame", "const f = (window.lastView?.frames ?? []).find(f => f.place === '1'); return f && f.readings && f")
    r = f["readings"]
    check("contracts off: three receivers, three numbers", (r.get("dashboard"), r["told"], r["lenient"]) == (None, 326, 326000), r)
    ans = n1.until("raw answer", "const t = document.getElementById('answer').textContent; return t.includes('readings') && t")
    check("node 1 is told what the receivers read", '"lenient": 326000' in ans, ans.splitlines()[0])
    topics = late.js("return [...new Set(window.rxTopics)].sort()")
    check("place 3 got only its own feedback and the mode", set(topics) <= {"3/feedback", "mode"} and "mode" in topics, topics)

    hub.js("document.getElementById('contracts').click()")
    f = hub.until("checked again", "const f = (window.lastView?.frames ?? []).find(f => f.place === '1'); return f && f.verdict && f")
    check("contracts on again: checked again", f["verdict"] == "accepted", f["verdict"])
finally:
    for b in browsers:
        try: b.quit()
        except Exception: pass
    for p in procs:
        p.terminate()
failed = [n for n, ok in checks if not ok]
print(f"{len(checks) - len(failed)} of {len(checks)} checks passed")
sys.exit(1 if failed else 0)
