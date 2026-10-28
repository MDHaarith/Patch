#!/usr/bin/env python3
"""Run with python3 scripts/test-install-linux.py; installs only into a temp dir."""
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parent.parent
APP_ID = "com.mdhaarith.Patch"


def executable(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("#!/bin/bash\nset -eu\n" + text)
    path.chmod(0o755)


def wait_for_launch(state, program, args):
    expected = b"\0".join(os.fsencode(arg) for arg in [program, *args, ""])
    deadline = time.monotonic() + 3
    while (not (state / "launched").exists() or (state / "launched").read_bytes() != expected) and time.monotonic() < deadline:
        time.sleep(0.05)
    assert (state / "launched").read_bytes() == expected


def check(binary_dir, prefix=None, shutdown=None):
    with tempfile.TemporaryDirectory(prefix="patch-install-test-") as tmp:
        tmp = Path(tmp)
        bundle = tmp / "bundle"
        (bundle / "scripts").mkdir(parents=True)
        installer = bundle / "scripts/install-linux.sh"
        shutil.copy2(ROOT / "scripts/install-linux.sh", installer)
        shutil.copytree(ROOT / "packaging", bundle / "packaging")
        state = tmp / "state"
        state.mkdir()
        (tmp / "run").mkdir(mode=0o700)
        env = dict(os.environ, HOME=str(tmp / "home"), XDG_DATA_HOME=str(tmp / "xdg data"),
                   XDG_CONFIG_HOME=str(tmp / "config"), XDG_RUNTIME_DIR=str(tmp / "run"),
                   CARGO_TARGET_DIR=str(bundle / "target"), STATE=str(state))
        # Keep the test away from the real session's services and caches.
        for tool in ["systemctl", "gtk-update-icon-cache", "update-desktop-database"]:
            executable(tmp / "tools" / tool, "exit 0\n")
        env["PATH"] = str(tmp / "tools") + os.pathsep + env["PATH"]
        binary = '''if [[ ${1:-} == --quit ]]; then
    echo "Patch isn't running."
else
    [[ ! -e $STATE/running ]] || { touch "$STATE/too-early"; exit 1; }
    printf '%s\\0' "$0" "$@" > "$STATE/launched"
fi
'''
        for name in ["patch", "patch-hub"]:
            executable(bundle / binary_dir / name, binary)
        args = [] if prefix is None else ["--prefix", str(tmp / prefix)]
        dest = Path(env["HOME"]) / ".local" if prefix is None else tmp / prefix
        share = Path(env["XDG_DATA_HOME"]) if prefix is None else dest / "share"
        if shutdown:
            (state / "running").touch()
            executable(dest / "bin/patch", '''if [[ -e $STATE/running ]]; then
    if [[ ! -e $STATE/stopping ]]; then
        touch "$STATE/stopping"
        (sleep 0.4; rm -f "$STATE/running") >/dev/null 2>&1 &
    fi
else
    echo "Patch isn't running."
fi
''' if shutdown == "delayed" else "exit 0\n")
        command = ["bash", str(installer), "--no-build", *args]
        result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=20)
        if shutdown == "stuck":
            assert result.returncode != 0, result.stdout
            assert "hasn't exited" in result.stderr, result.stderr
            assert not (state / "launched").exists()
            assert (dest / "bin/patch").read_text().endswith("exit 0\n"), "old binary was replaced"
            return
        assert result.returncode == 0, result.stdout + result.stderr
        desktop = share / f"applications/{APP_ID}.desktop"
        text = desktop.read_text()
        execs = [line[5:] for line in text.splitlines() if line.startswith("Exec=")]
        # shlex leaves the Exec escapes for dollar and backtick inside quotes.
        program = str(dest / "bin/patch")
        for line, suffix in zip(execs, [[], ["--quit"]]):
            expected = program.replace("%", "%%").replace("$", "\\$").replace("`", "\\`")
            assert shlex.split(line.replace("\\\\", "\\")) == [expected, *suffix], line
        unit = Path(env["XDG_CONFIG_HOME"]) / "systemd/user/patch-hub.service"
        systemd_exec = next(line.removeprefix("ExecStart=") for line in unit.read_text().splitlines()
                            if line.startswith("ExecStart="))
        assert shlex.split(systemd_exec) == ["/usr/bin/env", "--",
                                            str(dest / "bin/patch-hub").replace("%", "%%").replace("$", "$$")]
        if shutil.which("systemd-analyze"):
            subprocess.run(["systemd-analyze", "--user", "verify", str(unit)], env=env, check=True, capture_output=True)
        if shutil.which("desktop-file-validate"):
            subprocess.run(["desktop-file-validate", str(desktop)], check=True)
        if shutdown:
            wait_for_launch(state, program, ["--background"])
            (state / "launched").unlink()
        # GLib probes the executable before expanding %%; still validate its syntax.
        launch = shutil.which("gio") and "%" not in program
        if launch:
            launched = subprocess.run(["gio", "launch", str(desktop)], env=env, capture_output=True, text=True)
            assert launched.returncode == 0, launched.stderr
            wait_for_launch(state, program, [])
        assert not (state / "too-early").exists(), "restarted before old process exited"
        assert (share / f"metainfo/{APP_ID}.metainfo.xml").exists()
        assert (share / f"icons/hicolor/512x512/apps/{APP_ID}.png").exists()
        if prefix is None:
            assert not (dest / "share").exists(), "ignored XDG_DATA_HOME"
        else:
            assert not Path(env["XDG_DATA_HOME"]).exists(), "explicit prefix ignored"
        subprocess.run(["bash", str(installer), "--uninstall", *args], env=env, check=True, capture_output=True)
        assert not desktop.exists() and not unit.exists()
        assert not (dest / "bin/patch").exists()
        assert not (share / f"metainfo/{APP_ID}.metainfo.xml").exists()
        assert not (share / f"icons/hicolor/512x512/apps/{APP_ID}.png").exists()


check(".")  # Release archive root, default prefix and XDG_DATA_HOME.
check("scripts", 'My Apps & pipes|slash\\quote"dollar$back`')
check("scripts", "100%")
check("target/release", shutdown="delayed")
check(".", shutdown="stuck")
assert "ExecStart=/usr/bin/patch-hub\n" in (ROOT / "packaging/linux/patch-hub.service").read_text()
print("Installer checks passed (bundle layouts, quoting, XDG paths, uninstall, restart and timeout).")
