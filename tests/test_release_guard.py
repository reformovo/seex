import pytest

from scripts import release_guard

_PACKAGES = {name: "0.1.0-beta.4" for name in ("seex", "seex-python", "seex-app", "seex-plot")}
_PYTHON = {"seex-0.1.0b4-cp311-abi3-macosx_11_0_arm64.whl", "seex-0.1.0b4.tar.gz"}
_VIEWER = {"Seex-macos-aarch64.zip", "Seex-macos-aarch64.zip.sha256"}


def test_release_guard_accepts_matching_isolated_artifacts() -> None:
    release_guard.validate_release("v0.1.0-beta.4", _PACKAGES, _PYTHON, _VIEWER)


def test_release_guard_rejects_source_identity_mismatches() -> None:
    with pytest.raises(ValueError, match="does not match"):
        release_guard.validate_release("v0.1.0-beta.3", _PACKAGES, _PYTHON, _VIEWER)
    packages = {**_PACKAGES, "seex-app": "0.1.0-beta.3"}
    with pytest.raises(ValueError, match="versions do not match"):
        release_guard.validate_release("v0.1.0-beta.4", packages, _PYTHON, _VIEWER)


def test_release_guard_rejects_crossed_artifact_channels() -> None:
    with pytest.raises(ValueError, match="invalid Python"):
        release_guard.validate_release("v0.1.0-beta.4", _PACKAGES, _PYTHON | {"Seex-macos-aarch64.zip"}, _VIEWER)
    with pytest.raises(ValueError, match="invalid Viewer"):
        release_guard.validate_release("v0.1.0-beta.4", _PACKAGES, _PYTHON, set())
