"""Check deterministic sparse Run/key workload generation."""

import pytest

from scripts import p0_workload


@pytest.mark.parametrize("runs,keys,budget", [(4, 3, 12), (5, 3, 8), (3, 5, 8)])
def test_series_cover_runs_and_keys_without_duplicates(runs: int, keys: int, budget: int) -> None:
    profile = p0_workload._profile("correctness", {"runs": runs, "metric_keys": keys, "actual_sequences": budget})

    series = [
        (run_index, key_index)
        for run_index in range(runs)
        for _, key_index in p0_workload._series_for_run(run_index, profile)
    ]

    assert len(series) == len(set(series)) == budget
    assert {run_index for run_index, _ in series} == set(range(runs))
    assert {key_index for _, key_index in series} == set(range(keys))


def test_scale_series_enumeration_stays_sparse() -> None:
    profile = p0_workload._profile("scale")

    assert list(p0_workload._series_for_run(0, profile)) == [(batch * 1_000_000, batch) for batch in range(10)]
    assert len(list(p0_workload._series_for_run(999_999, profile))) == 10


def test_profile_rejects_impossible_sequence_counts() -> None:
    with pytest.raises(ValueError, match="actual_sequences"):
        p0_workload._profile("correctness", {"runs": 5, "metric_keys": 3, "actual_sequences": 4})
