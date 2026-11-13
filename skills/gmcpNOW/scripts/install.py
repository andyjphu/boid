#!/usr/bin/env python3
"""Install a versioned local skill and CLI wrappers without restarting Boid."""
import hashlib
import json
from pathlib import Path
import os
import shlex
import shutil
import subprocess
import sys


def run():
    source = Path(__file__).resolve().parents[1]
    home = Path.home()
    base = home / ".local/share/gmcpNOW"
    target = home / ".agents/skills/gmcpNOW"
    bin_dir = home / ".local/bin"
    names = [name + suffix for name in ("gauth", "glist", "gread", "gedit") for suffix in ("", ".py")]
    marker = "# Installed by gmcpNOW"
    for name in names:
        path = bin_dir / name
        if path.exists() and not any(m in path.read_text(errors="replace") for m in (marker, "# Installed by gtools")):
            raise RuntimeError(f"Refusing to replace an unrelated command: {path}")
    if target.exists() and not target.is_symlink():
        raise RuntimeError(f"Existing skill directory must be preserved/moved before installing: {target}")
    base.mkdir(parents=True, exist_ok=True)
    venv = base / "venv"
    python = venv / "bin/python"
    uv = shutil.which("uv")
    if not python.exists():
        command = [uv, "venv", "--python", sys.executable, str(venv)] if uv else [sys.executable, "-m", "venv", str(venv)]
        subprocess.run(command, check=True, stdout=sys.stderr)
    command = [uv, "pip", "install", "--python", str(python)] if uv else [str(python), "-m", "pip", "install"]
    subprocess.run(command + ["-r", str(source / "requirements.txt")], check=True, stdout=sys.stderr)
    files = sorted(p for p in source.rglob("*") if p.is_file() and "__pycache__" not in p.parts and p.suffix != ".pyc")
    digest = hashlib.sha256()
    for path in files:
        digest.update(str(path.relative_to(source)).encode() + b"\0" + path.read_bytes())
    package = base / "packages" / digest.hexdigest()[:16]
    if not package.exists():
        package.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source, package, ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".venv"))
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary = target.with_name(f".gmcpNOW-{os.getpid()}")
    temporary.symlink_to(package, target_is_directory=True)
    os.replace(temporary, target)
    bin_dir.mkdir(parents=True, exist_ok=True)
    for name in names:
        script = name if name.endswith(".py") else name + ".py"
        path = bin_dir / name
        body = f'#!/bin/sh\n{marker}\nexec {shlex.quote(str(python))} {shlex.quote(str(target / "scripts" / script))} "$@"\n'
        staging = bin_dir / ("." + name + ".gmcpNOW-next")
        staging.write_text(body)
        staging.chmod(0o755)
        os.replace(staging, path)
    print(json.dumps({"ok": True, "skill": str(target), "package": str(package), "python": str(python),
                      "commands": [str(bin_dir / name) for name in names], "restart_required": False}))


if __name__ == "__main__":
    try:
        run()
    except Exception as error:
        print(json.dumps({"error": {"code": 1, "message": str(error)}}), file=sys.stderr)
        raise SystemExit(1)
