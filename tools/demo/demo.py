"""Retakes every README screenshot with one command.

    python tools/demo/demo.py                       # build, run every session, write docs/screenshots
    python tools/demo/demo.py --only 04-details     # only these shots (every session still runs)
    python tools/demo/demo.py serve [session]       # open a session's window to look around (Quit ends it)
    python tools/demo/demo.py reset [--all]         # forget the sessions' state (--all: the build too)

It copies the working tree (`git ls-files -co --exclude-standard`) into `.demo/code`, builds the
`demo` example there (crates/app/examples/demo.rs: the real controller, windows and tray icon with
pretend Windows parts), starts a pretend Google quota service (google.py, answering from
accounts.json), and runs each session of shots.json against it. Every credential lives in memory
and every folder under `.demo/sessions/<session>`: the PC's Credential Manager, Antigravity, desktop
and Startup folder are never touched. The windows appear on screen while it runs; the tray-menu
shot moves the mouse pointer for a moment and puts it back.

Requires Windows, the Rust toolchain of rust-toolchain.toml, Python 3.11+ with Pillow.
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import queue
import shutil
import subprocess
import sys
import threading
import time
from ctypes import wintypes
from pathlib import Path

from google import PretendGoogle

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
DEMO = ROOT / ".demo"
CODE = DEMO / "code"
TARGET = DEMO / "target"
RUN = DEMO / "run"
SESSIONS = DEMO / "sessions"
OUT = ROOT / "docs" / "screenshots"
EXE = TARGET / "debug" / "examples" / "demo.exe"
SESSION_LIMIT = 240  # seconds


# --- the working-tree copy and the build -------------------------------------------------------


def sync() -> None:
    """Mirrors the working tree into .demo/code, copying only what changed (keeps builds incremental)."""
    listed = subprocess.run(
        ["git", "ls-files", "-co", "--exclude-standard", "-z"], cwd=ROOT, check=True, capture_output=True
    ).stdout.decode("utf-8").split("\0")
    wanted = {name for name in listed if name and (ROOT / name).is_file()}
    copied = 0
    for name in wanted:
        source, target = ROOT / name, CODE / name
        if target.is_file() and target.stat().st_size == source.stat().st_size and target.read_bytes() == source.read_bytes():
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        copied += 1
    removed = 0
    if CODE.exists():
        for path in sorted(CODE.rglob("*"), reverse=True):
            relative = path.relative_to(CODE).as_posix()
            if path.is_file() and relative not in wanted:
                path.unlink()
                removed += 1
            elif path.is_dir() and not any(path.iterdir()):
                path.rmdir()
    print(f"working tree copied to .demo/code ({copied} changed, {removed} removed)")


def build() -> None:
    env = dict(os.environ, CARGO_TARGET_DIR=str(TARGET), SLINT_EMIT_DEBUG_INFO="1")
    print("building the demo (the first build takes a few minutes) ...", flush=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "app", "--example", "demo"], cwd=CODE, env=env, check=True)


# --- the screen and the tray menu ----------------------------------------------------------------

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))  # per-monitor aware: physical pixels
user32.FindWindowExW.argtypes = [wintypes.HWND, wintypes.HWND, wintypes.LPCWSTR, wintypes.LPCWSTR]
user32.FindWindowExW.restype = wintypes.HWND
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
ENUM = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user32.EnumWindows.argtypes = [ENUM, wintypes.LPARAM]

HWND_MESSAGE = wintypes.HWND(-3)
WM_TRAYICON = 0x8000 + 1  # WM_APP + 1 in Slint's Windows tray (i-slint-core items/system_tray/windows.rs)
WM_RBUTTONUP = 0x0205
WM_KEYDOWN = 0x0100
VK_ESCAPE = 0x1B


def screen_size() -> tuple[int, int]:
    return user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)


def _pid_of(hwnd) -> int:
    pid = wintypes.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    return pid.value


def _tray_window(pid: int):
    hwnd = None
    while True:
        hwnd = user32.FindWindowExW(HWND_MESSAGE, hwnd, "SlintSystemTrayWindow", None)
        if not hwnd or _pid_of(hwnd) == pid:
            return hwnd


def _menu_window(pid: int):
    found = []

    def visit(hwnd, _):
        name = ctypes.create_unicode_buffer(32)
        user32.GetClassNameW(hwnd, name, 32)
        if name.value == "#32768" and user32.IsWindowVisible(hwnd) and _pid_of(hwnd) == pid:
            found.append(hwnd)
        return True

    user32.EnumWindows(ENUM(visit), 0)
    return found[0] if found else None


def capture_tray_menu(pid: int, path: Path) -> None:
    """Opens the demo's tray menu the way a right-click does, photographs it, and closes it."""
    from PIL import Image, ImageDraw, ImageGrab

    tray = _tray_window(pid)
    if not tray:
        raise RuntimeError("the demo has no tray icon")
    saved = wintypes.POINT()
    user32.GetCursorPos(ctypes.byref(saved))
    width, height = screen_size()
    user32.SetCursorPos(width // 2, height * 3 // 4)  # the menu opens above the pointer
    try:
        user32.PostMessageW(tray, WM_TRAYICON, 0, WM_RBUTTONUP)
        menu = None
        for _ in range(60):
            time.sleep(0.05)
            menu = _menu_window(pid)
            if menu:
                break
        if not menu:
            raise RuntimeError("the tray menu did not open")
        time.sleep(0.6)  # the menu fades in
        rect = wintypes.RECT()
        user32.GetWindowRect(menu, ctypes.byref(rect))
        box = (rect.left, rect.top, rect.right, rect.bottom)
        image = ImageGrab.grab(bbox=box, all_screens=True).convert("RGBA")
        # Windows 11 rounds menu corners: whatever is behind them is not part of the menu.
        radius = round(8 * user32.GetDpiForWindow(menu) / 96)
        mask = Image.new("L", image.size, 0)
        ImageDraw.Draw(mask).rounded_rectangle((0, 0, image.width - 1, image.height - 1), radius=radius, fill=255)
        image.putalpha(mask)
        image.save(path)
        print(f"SHOT {path.stem} {image.width}x{image.height}")
    finally:
        menu = _menu_window(pid)
        if menu:
            user32.PostMessageW(menu, WM_KEYDOWN, VK_ESCAPE, 0)
        for _ in range(40):
            if not _menu_window(pid):
                break
            time.sleep(0.05)
        user32.SetCursorPos(saved.x, saved.y)


# --- sessions ------------------------------------------------------------------------------------


def session_file(session: dict, plan: dict, data: dict, google: PretendGoogle, only: list[str], serve: bool) -> Path:
    root = SESSIONS / session["name"]
    shutil.rmtree(root, ignore_errors=True)
    signed_in = session.get("signed_in")
    accounts = [
        {
            "name": account["name"],
            "email": account["email"],
            "refresh": f"demo-refresh-{account['name']}",
            "signed_in": account["name"] == signed_in,
        }
        for account in data["accounts"]
        if account.get("saved", True) and session.get("accounts", True)
    ]
    spec = {
        "root": str(root),
        "api": google.url,
        "out": str(OUT),
        "now": data["now"],
        "window": session.get("window", plan.get("window", {})),
        "main": session.get("main", True),
        "hidden": session.get("hidden", False),
        "antigravity_open": session.get("antigravity_open", False),
        "accounts": accounts,
        "steps": [] if serve else session.get("steps", []),
        "serve": serve,
        "only": only,
    }
    RUN.mkdir(parents=True, exist_ok=True)
    path = RUN / f"{session['name']}.json"
    path.write_text(json.dumps(spec, indent=2), encoding="utf-8")
    return path


def scale_for(window: dict) -> float:
    """2x when the window fits on this screen at 2x, else the largest scale that fits."""
    width, height = screen_size()
    return round(min(2.0, width / window.get("width", 1180), height / window.get("height", 700)), 3)


def run_session(session: dict, plan: dict, data: dict, google: PretendGoogle, only: list[str], serve=False) -> int:
    google.phase = session.get("phase", "normal")
    google.release = session.get("release")
    scale = scale_for(session.get("window", plan.get("window", {})))
    spec = session_file(session, plan, data, google, only, serve)
    print(f"\n== {session['name']}", flush=True)
    env = dict(os.environ, SLINT_SCALE_FACTOR=str(scale))
    child = subprocess.Popen(
        [str(EXE), str(spec)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8", env=env
    )
    lines: queue.Queue[str | None] = queue.Queue()

    def read():
        for line in child.stdout:
            lines.put(line.rstrip("\n"))
        lines.put(None)

    threading.Thread(target=read, daemon=True).start()
    deadline = time.monotonic() + (10**9 if serve else SESSION_LIMIT)
    failures = 0
    while True:
        try:
            line = lines.get(timeout=1)
        except queue.Empty:
            if time.monotonic() > deadline:
                print("  the session took too long; stopping it")
                child.kill()
                return 1
            continue
        if line is None:
            break
        if line.startswith("EXTERNAL "):
            task, *args = line.removeprefix("EXTERNAL ").split()
            try:
                if task == "phase":
                    google.phase = args[0]
                elif task == "tray-menu":
                    if not only or args[0] in only:
                        capture_tray_menu(child.pid, OUT / f"{args[0]}.png")
                else:
                    raise RuntimeError(f"unknown task {task}")
            except Exception as error:  # report and carry on, like a failed shot
                print(f"  FAILED {task}: {error}")
                failures += 1
            child.stdin.write("done\n")
            child.stdin.flush()
        else:
            print(f"  {line}", flush=True)
    code = child.wait()
    return failures + (1 if code else 0)


def optimize(names: list[str]) -> None:
    from PIL import Image

    for name in names:
        path = OUT / f"{name}.png"
        if path.is_file():
            with Image.open(path) as image:
                image.load()
                image.save(path, optimize=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", nargs="?", default="run", choices=["run", "serve", "reset"])
    parser.add_argument("session", nargs="?", default="accounts", help="for serve: the session to open")
    parser.add_argument("--only", default="", help="comma-separated shot names")
    parser.add_argument("--all", action="store_true", help="for reset: also forget the copy and the build")
    args = parser.parse_args()

    if args.command == "reset":
        for path in [SESSIONS, RUN] + ([CODE, TARGET] if args.all else []):
            shutil.rmtree(path, ignore_errors=True)
        print("demo state forgotten")
        return 0

    plan = json.loads((HERE / "shots.json").read_text(encoding="utf-8"))
    data = json.loads((HERE / "accounts.json").read_text(encoding="utf-8"))
    sync()
    build()
    only = [name for name in args.only.split(",") if name]

    with PretendGoogle(data) as google:
        if args.command == "serve":
            session = next((s for s in plan["sessions"] if s["name"] == args.session), None)
            if not session:
                print(f"no session {args.session}; sessions: {', '.join(s['name'] for s in plan['sessions'])}")
                return 2
            return run_session(session, plan, data, google, only, serve=True)
        OUT.mkdir(parents=True, exist_ok=True)
        failures = sum(run_session(session, plan, data, google, only) for session in plan["sessions"])

    names = [step["shot"] for s in plan["sessions"] for step in s.get("steps", []) if "shot" in step]
    names += [step["external"].split()[1] for s in plan["sessions"] for step in s.get("steps", [])
              if step.get("external", "").startswith("tray-menu")]
    optimize([name for name in names if not only or name in only])
    print(f"\n{'all shots taken' if not failures else f'{failures} problem(s)'}: {OUT.relative_to(ROOT)}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
