"""Verify the app-only Seex CLI."""

from __future__ import annotations

import os
import pathlib
import shutil
import typing

import pytest
from seex import cli


def test_cli_app_replaces_process_with_seex_app(
    tmp_path: pathlib.Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project_path = tmp_path / "project"
    monkeypatch.setattr(shutil, "which", lambda _: "/Applications/Seex.app/Contents/MacOS/seex-app")

    class ExecCalled(Exception):
        pass

    def execv(path: str, arguments: list[str]) -> typing.NoReturn:
        assert path.endswith("seex-app")
        assert arguments == [path, os.fspath(project_path)]
        raise ExecCalled

    monkeypatch.setattr(os, "execv", execv)
    with pytest.raises(ExecCalled):
        cli.main(["app", os.fspath(project_path)])


def test_cli_app_reports_missing_binary(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setattr(shutil, "which", lambda _: None)
    assert cli.main(["app"]) == 1
    assert "seex-app was not found on PATH" in capsys.readouterr().err


@pytest.mark.parametrize("command", ["projects", "runs", "metrics", "autoresearch"])
def test_removed_read_commands_return_usage_error(
    command: str,
    capsys: pytest.CaptureFixture[str],
) -> None:
    with pytest.raises(SystemExit) as raised:
        cli.main([command])
    assert raised.value.code == 2
    assert "usage" in capsys.readouterr().err.lower()
