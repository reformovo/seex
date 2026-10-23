"""Launch the independently installed Seex desktop application."""

from __future__ import annotations

import argparse
import os
import pathlib
import shutil
import sys
from collections.abc import Sequence

_APP_BINARY = "seex-app"


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="seex")
    resources = parser.add_subparsers(dest="resource", required=True)
    app = resources.add_parser("app", help="launch the Seex desktop application")
    app.add_argument("project_path", nargs="?", type=pathlib.Path)
    return parser


def _launch_app(project_path: pathlib.Path | None) -> int:
    binary = shutil.which(_APP_BINARY)
    if binary is None:
        print(
            "seex-app was not found on PATH; install the Seex desktop app and ensure its binary is available on PATH",
            file=sys.stderr,
        )
        return 1
    arguments = [binary]
    if project_path is not None:
        arguments.append(os.fspath(project_path))
    try:
        os.execv(binary, arguments)
    except OSError as error:
        print(f"failed to launch seex-app: {error}", file=sys.stderr)
        return 1
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    """Parse the launcher command and replace the process with Seex Viewer."""
    args = _build_parser().parse_args(argv)
    return _launch_app(args.project_path)


if __name__ == "__main__":
    raise SystemExit(main())
