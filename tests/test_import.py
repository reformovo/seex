"""Verify import smoke behavior."""

from __future__ import annotations


def test_import_seex() -> None:
    import seex

    assert hasattr(seex, "__all__")
    assert seex.ArrowTable.__name__ == "ArrowTable"
    assert seex.Api.__name__ == "Api"
    assert seex.MetricSeries.__name__ == "MetricSeries"
    assert seex.RunRecord.__name__ == "RunRecord"
    assert seex.Settings.__name__ == "Settings"
    assert "ArrowTable" in seex.__all__
