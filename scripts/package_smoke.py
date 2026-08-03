"""Verify the standalone crates.io package for the public Seex SDK."""

from __future__ import annotations

import pathlib
import subprocess
import tarfile
import tempfile
import tomllib
from collections.abc import Iterator, Mapping
from typing import Any, cast

_DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")
_FORBIDDEN_DEPENDENCIES = frozenset({"gpui", "pyo3", "seex-app", "seex-plot", "seex-python"})
_FORBIDDEN_SOURCE_SEGMENTS = frozenset({"seex-app", "seex-plot", "seex-python"})


def _run(command: list[str], *, cwd: pathlib.Path) -> None:
    subprocess.run(command, cwd=cwd, check=True)


def _load_toml(path: pathlib.Path) -> dict[str, Any]:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def _package_version(root: pathlib.Path) -> str:
    manifest = _load_toml(root / "crates/seex/Cargo.toml")
    package = cast(Mapping[str, object], manifest.get("package"))
    version = package.get("version")
    if not isinstance(version, str) or not version:
        raise ValueError("crates/seex/Cargo.toml has no package version")
    return version


def _dependency_tables(manifest: Mapping[str, Any]) -> Iterator[Mapping[str, Any]]:
    for name in _DEPENDENCY_TABLES:
        table = manifest.get(name)
        if isinstance(table, dict):
            yield table
    targets = manifest.get("target")
    if not isinstance(targets, dict):
        return
    for target in targets.values():
        if not isinstance(target, dict):
            continue
        for name in _DEPENDENCY_TABLES:
            table = target.get(name)
            if isinstance(table, dict):
                yield table


def _validate_manifest(manifest: Mapping[str, Any], *, expected_version: str) -> None:
    package = manifest.get("package")
    if not isinstance(package, dict):
        raise TypeError("packaged manifest has no [package] table")
    if package.get("name") != "seex" or package.get("version") != expected_version:
        raise ValueError("packaged manifest has the wrong package identity")
    for dependencies in _dependency_tables(manifest):
        forbidden = _FORBIDDEN_DEPENDENCIES.intersection(dependencies)
        if forbidden:
            raise ValueError(f"packaged manifest contains forbidden dependencies: {sorted(forbidden)}")
        for name, specification in dependencies.items():
            if isinstance(specification, dict) and "path" in specification:
                raise ValueError(f"packaged dependency {name!r} contains a path")


def _read_archive_toml(archive: tarfile.TarFile, member_name: str) -> dict[str, Any]:
    member = archive.extractfile(member_name)
    if member is None:
        raise ValueError(f"package archive is missing {member_name}")
    with member:
        return tomllib.loads(member.read().decode("utf-8"))


def _validate_members(members: list[tarfile.TarInfo], *, prefix: str) -> None:
    for member in members:
        path = pathlib.PurePosixPath(member.name)
        if not path.parts or path.parts[0] != prefix:
            raise ValueError(f"package member escapes its expected prefix: {member.name!r}")
        forbidden = _FORBIDDEN_SOURCE_SEGMENTS.intersection(path.parts)
        if forbidden:
            raise ValueError(f"package contains private workspace source: {member.name!r}")
        if member.issym() or member.islnk():
            raise ValueError(f"package contains an unsupported link: {member.name!r}")


def _verify_archive(crate_path: pathlib.Path, *, version: str) -> None:
    prefix = f"seex-{version}"
    with tarfile.open(crate_path, mode="r:gz") as archive:
        members = archive.getmembers()
        _validate_members(members, prefix=prefix)
        _validate_manifest(
            _read_archive_toml(archive, f"{prefix}/Cargo.toml"),
            expected_version=version,
        )
        _validate_manifest(
            _read_archive_toml(archive, f"{prefix}/Cargo.toml.orig"),
            expected_version=version,
        )
        with tempfile.TemporaryDirectory(prefix="seex-package-smoke-") as temporary:
            temporary_root = pathlib.Path(temporary)
            archive.extractall(temporary_root)
            unpacked = temporary_root / prefix
            _run(
                ["cargo", "test", "--locked", "--all-features"],
                cwd=unpacked,
            )


def main() -> None:
    """Packages, inspects, and independently tests the public Rust crate."""
    root = pathlib.Path(__file__).resolve().parents[1]
    version = _package_version(root)
    _run(
        ["cargo", "package", "-p", "seex", "--allow-dirty", "--no-verify"],
        cwd=root,
    )
    crate_path = root / f"target/package/seex-{version}.crate"
    if not crate_path.is_file():
        raise FileNotFoundError(f"cargo package did not create {crate_path}")
    _verify_archive(crate_path, version=version)
    print(f"verified standalone seex {version} package")


if __name__ == "__main__":
    main()
