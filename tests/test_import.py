"""Verify import smoke behavior."""

from __future__ import annotations


def test_import_seex() -> None:
    import seex

    assert hasattr(seex, "__all__")
    assert seex.ArrowTable.__name__ == "ArrowTable"
    assert "ArrowTable" in seex.__all__
