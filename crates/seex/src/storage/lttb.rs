//! Seex-native LTTB selection for an already ordered observation stream.
//!
//! The selector returns positions in the caller's input. It does not copy or
//! persist observations, so duplicate axes remain distinct observations.

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SelectionError {
    NonFiniteValue { index: usize },
}

/// Selects at most `max_points` input positions using Largest Triangle Three
/// Buckets. Input axes must be non-decreasing; equal axes retain input order.
/// Values must be finite in this first native implementation.
pub(crate) fn select_indices(
    axes: &[i64],
    values: &[f64],
    max_points: usize,
) -> Result<Vec<usize>, SelectionError> {
    assert_eq!(
        axes.len(),
        values.len(),
        "LTTB inputs must have equal lengths"
    );
    if max_points == 0 || axes.is_empty() {
        return Ok(Vec::new());
    }
    if let Some((index, _)) = values
        .iter()
        .enumerate()
        .find(|(_, value)| !value.is_finite())
    {
        return Err(SelectionError::NonFiniteValue { index });
    }
    if axes.len() <= max_points {
        return Ok((0..axes.len()).collect());
    }
    if max_points == 1 {
        return Ok(vec![0]);
    }
    if max_points == 2 {
        return Ok(vec![0, axes.len() - 1]);
    }

    let count = axes.len();
    let every = (count - 2) as f64 / (max_points - 2) as f64;
    let mut selected = Vec::with_capacity(max_points);
    selected.push(0);
    let mut previous = 0usize;

    for bucket in 0..(max_points - 2) {
        let average_start = ((bucket + 1) as f64 * every).floor() as usize + 1;
        let average_end = ((bucket + 2) as f64 * every).floor() as usize + 1;
        let average_end = average_end.min(count);
        let (average_x, average_y) = if average_start < average_end {
            let length = (average_end - average_start) as f64;
            let x = (average_start..average_end)
                .map(|index| relative_axis(axes[index], axes[0]))
                .sum::<f64>()
                / length;
            let y = values[average_start..average_end].iter().sum::<f64>() / length;
            (x, y)
        } else {
            (relative_axis(axes[count - 1], axes[0]), values[count - 1])
        };

        let current_start = (bucket as f64 * every).floor() as usize + 1;
        let current_end = (((bucket + 1) as f64 * every).floor() as usize + 1).min(count - 1);
        let mut best = current_start.min(count - 1);
        let mut best_area = f64::NEG_INFINITY;
        for index in current_start..current_end.max(current_start + 1).min(count - 1) {
            let area = triangle_area(
                relative_axis(axes[previous], axes[0]),
                values[previous],
                relative_axis(axes[index], axes[0]),
                values[index],
                average_x,
                average_y,
            );
            if area > best_area {
                best_area = area;
                best = index;
            }
        }
        selected.push(best);
        previous = best;
    }
    selected.push(count - 1);
    Ok(selected)
}

fn relative_axis(axis: i64, origin: i64) -> f64 {
    (axis as i128 - origin as i128) as f64
}

fn triangle_area(ax: f64, ay: f64, bx: f64, by: f64, cx: f64, cy: f64) -> f64 {
    ((ax - cx) * (by - ay) - (ax - bx) * (cy - ay)).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_endpoints_and_budget() {
        let axes = (0..100).collect::<Vec<_>>();
        let values = axes
            .iter()
            .map(|axis| (*axis as f64).sin())
            .collect::<Vec<_>>();
        let selected = select_indices(&axes, &values, 16).unwrap();
        assert_eq!(selected.len(), 16);
        assert_eq!(selected.first(), Some(&0));
        assert_eq!(selected.last(), Some(&99));
        assert!(selected.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn preserves_duplicate_axis_observations_when_selected() {
        let axes = [0, 1, 1, 1, 2, 3];
        let values = [0.0, -3.0, 2.0, 8.0, 0.0, 0.0];
        let selected = select_indices(&axes, &values, 6).unwrap();
        assert_eq!(selected, (0..axes.len()).collect::<Vec<_>>());
        let selected = select_indices(&axes, &values, 4).unwrap();
        assert_eq!(selected.first(), Some(&0));
        assert_eq!(selected.last(), Some(&5));
        assert!(selected.iter().any(|index| *index == 3));
    }

    #[test]
    fn handles_small_budgets_and_empty_input() {
        let axes = [0, 1, 2, 3];
        let values = [0.0, 2.0, -1.0, 0.0];
        assert!(select_indices(&[], &[], 4).unwrap().is_empty());
        assert_eq!(select_indices(&axes, &values, 1).unwrap(), vec![0]);
        assert_eq!(select_indices(&axes, &values, 2).unwrap(), vec![0, 3]);
        assert_eq!(select_indices(&axes, &values, 8).unwrap(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn rejects_non_finite_values_without_silent_loss() {
        let error = select_indices(&[0, 1], &[0.0, f64::NAN], 2).unwrap_err();
        assert_eq!(error, SelectionError::NonFiniteValue { index: 1 });
        let error = select_indices(&[0, 1], &[0.0, f64::INFINITY], 2).unwrap_err();
        assert_eq!(error, SelectionError::NonFiniteValue { index: 1 });
    }

    #[test]
    fn translates_large_axes_without_float_roundtrip_of_absolute_values() {
        let origin = 9_007_199_254_740_993_i64;
        let axes = [origin, origin + 1, origin + 2, origin + 3];
        let values = [0.0, 10.0, -10.0, 0.0];
        assert_eq!(select_indices(&axes, &values, 2).unwrap(), vec![0, 3]);
    }
}
