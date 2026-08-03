"""`scripts/start.py`'s parent-death watchdog, which the desktop shell depends on.

Both properties below were regressions once: the watchdog left its pipe on fd 0, so the Alembic
migration subprocess inherited it and blocked for its full 120s timeout on a first run.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import pytest

START_PY = Path(__file__).resolve().parents[2] / "scripts" / "start.py"

_LOAD_WATCHDOG = f"""
import importlib.util, subprocess, sys
spec = importlib.util.spec_from_file_location("start", r"{START_PY}")
start = importlib.util.module_from_spec(spec)
spec.loader.exec_module(start)
start.exit_when_parent_dies()
"""


def test_children_do_not_inherit_the_parent_pipe():
    """A subprocess must see an immediately-closed stdin, not the shell's pipe.

    The pipe is held open for the duration: closing it is what tells the watchdog to exit, so a
    closed pipe would end the probe before it could report.
    """
    probe = _LOAD_WATCHDOG + (
        "out = subprocess.run([sys.executable, '-c', 'import sys; sys.stdout.write(sys.stdin.read())'],"
        " capture_output=True, text=True, timeout=20)\n"
        "print('CHILD_STDIN=' + repr(out.stdout), flush=True)\n"
        "import time; time.sleep(30)\n"
    )
    child = subprocess.Popen(
        [sys.executable, "-c", probe],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert child.stdout is not None
        reported = child.stdout.readline()
        assert "CHILD_STDIN=''" in reported, reported
    finally:
        child.kill()
        child.wait(timeout=30)


def test_watchdog_exits_when_the_parent_closes_the_pipe():
    child = subprocess.Popen(
        [sys.executable, "-c", _LOAD_WATCHDOG + "import time; time.sleep(60)\n"],
        stdin=subprocess.PIPE,
    )
    try:
        assert child.stdin is not None
        child.stdin.close()  # what the shell dying looks like from in here
        assert child.wait(timeout=30) == 0
    except subprocess.TimeoutExpired:
        child.kill()
        pytest.fail("the watchdog did not exit after its parent closed the pipe")
    finally:
        if child.poll() is None:
            child.kill()
