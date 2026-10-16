//! Seex-native LTTB selection for an already ordered observation stream.
//!
//! The selector returns positions in the caller's input. It does not copy or
//! persist observations, so duplicate axes remain distinct observations.

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SelectionError {
    NonFiniteValue { index: usize },
    UnsortedAxis { index: usize },
    CountMismatch { expected: usize, actual: usize },
    ArithmeticOverflow,
}

pub(crate) trait Point {
    fn axis(&self) -> i64;
    fn value(&self) -> f64;
}

/// Ordered finite input with known length. Keeps output plus two adjacent buckets,
/// O(budget + count/budget) points, not constant memory for very small budgets.
pub(crate) struct OrderedLttb<T> {
    count: usize,
    budget: usize,
    seen: usize,
    bucket: usize,
    last_axis: Option<i64>,
    buffer: std::collections::VecDeque<T>,
    selected: Vec<T>,
    peak: usize,
    every: f64,
    origin: Option<i64>,
    next_axis_sum: f64,
    next_value_sum: f64,
    next_count: usize,
}

impl<T: Point> OrderedLttb<T> {
    pub(crate) fn new(count: usize, budget: usize) -> Self {
        Self {
            count,
            budget,
            seen: 0,
            bucket: 0,
            last_axis: None,
            buffer: std::collections::VecDeque::new(),
            selected: Vec::new(),
            peak: 0,
            every: if budget > 2 {
                (count.saturating_sub(2)) as f64 / (budget - 2) as f64
            } else {
                0.0
            },
            origin: None,
            next_axis_sum: 0.0,
            next_value_sum: 0.0,
            next_count: 0,
        }
    }

    pub(crate) fn peak_buffered(&self) -> usize {
        self.peak
    }

    pub(crate) fn push(&mut self, point: T) -> Result<(), SelectionError> {
        if self.seen == self.count {
            return Err(SelectionError::CountMismatch {
                expected: self.count,
                actual: self.seen + 1,
            });
        }
        if !point.value().is_finite() {
            return Err(SelectionError::NonFiniteValue { index: self.seen });
        }
        let axis = point.axis();
        if self.last_axis.is_some_and(|last| axis < last) {
            return Err(SelectionError::UnsortedAxis { index: self.seen });
        }
        self.origin.get_or_insert(axis);
        self.last_axis = Some(axis);
        self.seen += 1;
        if self.count <= self.budget
            || (self.seen == 1 && self.budget > 0)
            || (self.budget == 2 && self.seen == self.count)
        {
            self.selected.push(point);
        } else if self.budget > 2 {
            self.buffer.push_back(point);
            self.peak = self.peak.max(self.buffer.len());
            let point_index = self.seen - 1;
            let start = (self.bucket as f64 * self.every).floor() as usize + 1;
            let next = ((self.bucket + 1) as f64 * self.every).floor() as usize + 1;
            let end =
                (((self.bucket + 2) as f64 * self.every).floor() as usize + 1).min(self.count);
            if point_index >= next && point_index < end {
                let origin = self.origin.expect("first point establishes origin");
                let queued = self.buffer.back().expect("point was just buffered");
                self.next_axis_sum += relative_axis(queued.axis(), origin);
                self.next_value_sum += queued.value();
                self.next_count += 1;
            }
            if self.seen == end {
                let length = self.next_count as f64;
                let average_x = self.next_axis_sum / length;
                let average_y = self.next_value_sum / length;
                if !average_x.is_finite() || !average_y.is_finite() {
                    return Err(SelectionError::ArithmeticOverflow);
                }
                let origin = self.origin.expect("first point establishes origin");
                let previous = self.selected.last().expect("first point retained");
                let mut best = 0;
                let mut area = f64::NEG_INFINITY;
                for (index, point) in self.buffer.iter().take(next - start).enumerate() {
                    let candidate = triangle_area(
                        relative_axis(previous.axis(), origin),
                        previous.value(),
                        relative_axis(point.axis(), origin),
                        point.value(),
                        average_x,
                        average_y,
                    );
                    if !candidate.is_finite() {
                        return Err(SelectionError::ArithmeticOverflow);
                    }
                    if candidate > area {
                        area = candidate;
                        best = index;
                    }
                }
                for index in 0..(next - start) {
                    let point = self.buffer.pop_front().expect("current bucket buffered");
                    if index == best {
                        self.selected.push(point);
                    }
                }
                self.bucket += 1;
                self.next_axis_sum = 0.0;
                self.next_value_sum = 0.0;
                self.next_count = 0;
                if self.seen == self.count {
                    self.selected
                        .push(self.buffer.pop_back().expect("last point buffered"));
                    self.buffer.clear();
                }
            }
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<Vec<T>, SelectionError> {
        if self.seen != self.count {
            return Err(SelectionError::CountMismatch {
                expected: self.count,
                actual: self.seen,
            });
        }
        Ok(self.selected)
    }
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

    impl Point for (i64, f64, usize) {
        fn axis(&self) -> i64 {
            self.0
        }
        fn value(&self) -> f64 {
            self.1
        }
    }

    #[test]
    fn ordered_stream_matches_reference_and_bounds_buffers() {
        for count in 0..80 {
            let axes = (0..count)
                .map(|i| 9_007_199_254_740_993 + (i / 3) as i64)
                .collect::<Vec<_>>();
            let values = (0..count)
                .map(|i| ((i * i % 97) as f64).sin())
                .collect::<Vec<_>>();
            for budget in 0..85 {
                let mut stream = OrderedLttb::new(count, budget);
                for index in 0..count {
                    stream.push((axes[index], values[index], index)).unwrap();
                }
                if count > budget && budget > 2 {
                    assert!(
                        stream.peak_buffered()
                            <= 2 * ((count - 2) as f64 / (budget - 2) as f64).ceil() as usize + 1
                    );
                }
                let selected = stream
                    .finish()
                    .unwrap()
                    .into_iter()
                    .map(|p| p.2)
                    .collect::<Vec<_>>();
                assert_eq!(
                    selected,
                    select_indices(&axes, &values, budget).unwrap(),
                    "N={count}, K={budget}"
                );
            }
        }
        assert!(matches!(
            OrderedLttb::<(i64, f64, usize)>::new(1, 1).finish(),
            Err(SelectionError::CountMismatch { .. })
        ));
        let mut stream = OrderedLttb::new(2, 2);
        stream.push((1, 0.0, 0)).unwrap();
        assert_eq!(
            stream.push((0, 1.0, 1)),
            Err(SelectionError::UnsortedAxis { index: 1 })
        );
        assert_eq!(
            stream.push((2, f64::NAN, 1)),
            Err(SelectionError::NonFiniteValue { index: 1 })
        );
        stream.push((2, 1.0, 1)).unwrap();
        assert!(matches!(
            stream.push((3, 1.0, 2)),
            Err(SelectionError::CountMismatch { .. })
        ));
    }

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
