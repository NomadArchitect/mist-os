// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ops::Range;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeType {
    Cow(Range<u64>),
    Overwrite(Range<u64>),
}

#[derive(Debug)]
pub struct AllocatedRanges {
    ranges: Mutex<Vec<Range<u64>>>,
}

pub struct RangeOverlapIter<'a> {
    query_range: Range<u64>,
    index: usize,
    ranges: std::sync::MutexGuard<'a, Vec<Range<u64>>>,
}

impl<'a> Iterator for RangeOverlapIter<'a> {
    type Item = RangeType;

    fn next(&mut self) -> Option<Self::Item> {
        if self.query_range.start == self.query_range.end {
            return None;
        }

        if self.index == self.ranges.len() || self.query_range.start < self.ranges[self.index].start
        {
            let range = self.query_range.start
                ..std::cmp::min(
                    self.query_range.end,
                    self.ranges.get(self.index).map(|r| r.start).unwrap_or(self.query_range.end),
                );
            self.query_range.start = range.end;
            return Some(RangeType::Cow(range));
        }

        let range = self.query_range.start
            ..std::cmp::min(self.query_range.end, self.ranges[self.index].end);
        self.query_range.start = range.end;
        self.index += 1;

        return Some(RangeType::Overwrite(range));
    }
}

impl AllocatedRanges {
    pub fn new(ranges_to_apply: Vec<Range<u64>>) -> Self {
        let mut ranges = Vec::new();
        for range_to_apply in ranges_to_apply {
            Self::apply_range_to(&mut ranges, range_to_apply);
        }
        Self { ranges: Mutex::new(ranges) }
    }

    pub fn overlap<'a>(&'a self, query_range: Range<u64>) -> RangeOverlapIter<'a> {
        let ranges = self.ranges.lock().unwrap();
        let index = match ranges.binary_search_by_key(&query_range.start, |r| r.end) {
            // If the start of the query range is exactly at the end of a range, there is zero
            // overlap with that range, so start with the next one.
            Ok(pos) => pos + 1,
            Err(pos) => pos,
        };
        RangeOverlapIter { query_range, index, ranges }
    }

    // Apply range takes a single, valid file range and inserts it into the list of ranges it's
    // storing. This list of ranges, so it's easy to insert and search, is kept sorted and merged,
    // so that the list has no overlapping ranges.
    pub fn apply_range(&self, new_range: Range<u64>) {
        Self::apply_range_to(self.ranges.lock().unwrap().as_mut(), new_range)
    }

    pub fn apply_range_to(ranges: &mut Vec<Range<u64>>, new_range: Range<u64>) {
        let merge_start = match ranges.binary_search_by_key(&new_range.start, |r| r.end) {
            // Ok means the returned index has a range that ends where this new one starts, which
            // is handled fine by the logic below.
            Ok(pos) => pos,
            Err(pos) => pos,
        };
        if merge_start == ranges.len() {
            // The new ranges starts beyond the end of all the current ranges.
            ranges.push(new_range);
            return;
        }

        if ranges[merge_start].start <= new_range.start {
            // If the new range start is past (or at) the start but before the end, this is the
            // first range that needs to get merged.
            ranges[merge_start].end = std::cmp::max(ranges[merge_start].end, new_range.end);
        } else {
            // The new range starts before this one. Insert it at this spot, and merge from here.
            ranges.insert(merge_start, new_range);
        }

        let mut merge_index = merge_start + 1;
        while merge_index < ranges.len() && ranges[merge_index].start <= ranges[merge_start].end {
            ranges[merge_start].end =
                std::cmp::max(ranges[merge_start].end, ranges[merge_index].end);
            merge_index += 1;
        }
        ranges.drain(merge_start + 1..merge_index);
    }
}

#[cfg(test)]
mod tests {
    use super::{AllocatedRanges, RangeType};
    use std::ops::Range;

    #[fuchsia::test]
    fn test_allocated_ranges() {
        struct Case {
            applied_ranges: Vec<Range<u64>>,
            expected_ranges: Vec<Range<u64>>,
        }
        let cases = [
            Case { applied_ranges: vec![0..1], expected_ranges: vec![0..1] },
            Case { applied_ranges: vec![0..1, 2..3], expected_ranges: vec![0..1, 2..3] },
            Case {
                applied_ranges: vec![0..1, 2..3, 4..5],
                expected_ranges: vec![0..1, 2..3, 4..5],
            },
            Case {
                applied_ranges: vec![4..5, 2..3, 0..1],
                expected_ranges: vec![0..1, 2..3, 4..5],
            },
            Case {
                applied_ranges: vec![0..1, 4..5, 2..3],
                expected_ranges: vec![0..1, 2..3, 4..5],
            },
            Case { applied_ranges: vec![0..10, 20..30], expected_ranges: vec![0..10, 20..30] },
            Case { applied_ranges: vec![0..5, 0..5], expected_ranges: vec![0..5] },
            Case { applied_ranges: vec![0..5, 0..1], expected_ranges: vec![0..5] },
            Case { applied_ranges: vec![0..5, 0..10], expected_ranges: vec![0..10] },
            Case { applied_ranges: vec![3..4, 2..3], expected_ranges: vec![2..4] },
            Case { applied_ranges: vec![2..3, 3..4], expected_ranges: vec![2..4] },
            Case { applied_ranges: vec![2..3, 3..4, 4..5, 1..2], expected_ranges: vec![1..5] },
            Case { applied_ranges: vec![1..10, 2..4, 8..9, 2..9], expected_ranges: vec![1..10] },
            Case { applied_ranges: vec![2..3, 3..4, 1..2, 0..10], expected_ranges: vec![0..10] },
            Case {
                applied_ranges: vec![1..2, 3..4, 5..6, 7..8],
                expected_ranges: vec![1..2, 3..4, 5..6, 7..8],
            },
            Case {
                applied_ranges: vec![1..2, 3..4, 5..6, 7..8, 0..10],
                expected_ranges: vec![0..10],
            },
            Case { applied_ranges: vec![4..8, 6..10], expected_ranges: vec![4..10] },
            Case { applied_ranges: vec![4..8, 2..6], expected_ranges: vec![2..8] },
            Case {
                applied_ranges: vec![2..5, 7..11, 13..18, 20..30, 40..45, 10..25],
                expected_ranges: vec![2..5, 7..30, 40..45],
            },
        ];

        for case in cases {
            let ranges = AllocatedRanges::new(case.applied_ranges);
            assert_eq!(*ranges.ranges.lock().unwrap(), case.expected_ranges);
        }
    }

    #[fuchsia::test]
    fn test_allocated_ranges_overlap() {
        let ranges = AllocatedRanges::new(Vec::new());
        // With no overwrite ranges recorded, all overlap calls should return the same range
        // wrapped with Cow.
        assert_eq!(ranges.overlap(0..1).collect::<Vec<_>>(), vec![RangeType::Cow(0..1)]);
        assert_eq!(ranges.overlap(10..20).collect::<Vec<_>>(), vec![RangeType::Cow(10..20)]);

        ranges.apply_range(10..20);
        assert_eq!(ranges.overlap(30..35).collect::<Vec<_>>(), vec![RangeType::Cow(30..35)]);
        assert_eq!(ranges.overlap(20..30).collect::<Vec<_>>(), vec![RangeType::Cow(20..30)]);
        assert_eq!(ranges.overlap(0..5).collect::<Vec<_>>(), vec![RangeType::Cow(0..5)]);
        assert_eq!(ranges.overlap(0..10).collect::<Vec<_>>(), vec![RangeType::Cow(0..10)]);

        assert_eq!(ranges.overlap(12..13).collect::<Vec<_>>(), vec![RangeType::Overwrite(12..13)]);
        assert_eq!(ranges.overlap(10..20).collect::<Vec<_>>(), vec![RangeType::Overwrite(10..20)]);

        assert_eq!(
            ranges.overlap(5..15).collect::<Vec<_>>(),
            vec![RangeType::Cow(5..10), RangeType::Overwrite(10..15)]
        );
        assert_eq!(
            ranges.overlap(5..20).collect::<Vec<_>>(),
            vec![RangeType::Cow(5..10), RangeType::Overwrite(10..20)]
        );
        assert_eq!(
            ranges.overlap(5..25).collect::<Vec<_>>(),
            vec![RangeType::Cow(5..10), RangeType::Overwrite(10..20), RangeType::Cow(20..25)]
        );

        assert_eq!(ranges.overlap(10..15).collect::<Vec<_>>(), vec![RangeType::Overwrite(10..15)]);
        assert_eq!(ranges.overlap(10..20).collect::<Vec<_>>(), vec![RangeType::Overwrite(10..20)]);
        assert_eq!(
            ranges.overlap(10..25).collect::<Vec<_>>(),
            vec![RangeType::Overwrite(10..20), RangeType::Cow(20..25)]
        );

        assert_eq!(ranges.overlap(15..20).collect::<Vec<_>>(), vec![RangeType::Overwrite(15..20)]);
        assert_eq!(
            ranges.overlap(15..25).collect::<Vec<_>>(),
            vec![RangeType::Overwrite(15..20), RangeType::Cow(20..25)]
        );

        assert_eq!(ranges.overlap(20..25).collect::<Vec<_>>(), vec![RangeType::Cow(20..25)]);

        ranges.apply_range(30..40);
        ranges.apply_range(50..60);

        assert_eq!(
            ranges.overlap(15..35).collect::<Vec<_>>(),
            vec![
                RangeType::Overwrite(15..20),
                RangeType::Cow(20..30),
                RangeType::Overwrite(30..35)
            ]
        );
        assert_eq!(
            ranges.overlap(25..45).collect::<Vec<_>>(),
            vec![RangeType::Cow(25..30), RangeType::Overwrite(30..40), RangeType::Cow(40..45)]
        );
        assert_eq!(
            ranges.overlap(0..70).collect::<Vec<_>>(),
            vec![
                RangeType::Cow(0..10),
                RangeType::Overwrite(10..20),
                RangeType::Cow(20..30),
                RangeType::Overwrite(30..40),
                RangeType::Cow(40..50),
                RangeType::Overwrite(50..60),
                RangeType::Cow(60..70)
            ]
        );

        ranges.apply_range(0..100);
        assert_eq!(ranges.overlap(0..100).collect::<Vec<_>>(), vec![RangeType::Overwrite(0..100)]);
    }
}
