import pytest

from scripts import release_guard

_PACKAGES = {name: "0.1.0-beta.1" for name in ("seex", "seex-python", "seex-app", "seex-plot")}
_PYTHON = {"seex-0.1.0b1-cp311-abi3-macosx_11_0_arm64.whl", "seex-0.1.0b1.tar.gz"}
_VIEWER = {"seex-app-macos-aarch64", "seex-app-macos-aarch64.sha256"}


def test_release_guard_accepts_matching_isolated_artifacts() -> None:
    release_guard.validate_release("v0.1.0-beta.1", _PACKAGES, _PYTHON, _VIEWER)


def test_release_guard_rejects_source_identity_mismatches() -> None:
    with pytest.raises(ValueError, match="does not match"):
        release_guard.validate_release("v0.1.0-beta.2", _PACKAGES, _PYTHON, _VIEWER)
    packages = {**_PACKAGES, "seex-app": "0.1.0-beta.2"}
    with pytest.raises(ValueError, match="versions do not match"):
        release_guard.validate_release("v0.1.0-beta.1", packages, _PYTHON, _VIEWER)


def test_release_guard_rejects_crossed_artifact_channels() -> None:
    with pytest.raises(ValueError, match="invalid Python"):
        release_guard.validate_release("v0.1.0-beta.1", _PACKAGES, _PYTHON | {"seex-app-macos-aarch64"}, _VIEWER)
    with pytest.raises(ValueError, match="invalid Viewer"):
        release_guard.validate_release("v0.1.0-beta.1", _PACKAGES, _PYTHON, set())
