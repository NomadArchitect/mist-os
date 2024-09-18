# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Suspend trace metrics."""

from typing import Iterator, Optional, Sequence

from trace_processing import trace_metrics, trace_model, trace_time, trace_utils

_EVENT_CATEGORY = "power"
# LINT.IfChange
_EVENT_NAME = "system-activity-governor:suspend"
# LINT.ThenChange(//src/power/system-activity-governor/src/system_activity_governor.rs)


class SuspendMetricsProcessor(trace_metrics.MetricsProcessor):
    """Computes suspend/resume metrics."""

    def process_metrics(
        self, model: trace_model.Model
    ) -> Sequence[trace_metrics.TestCaseResult]:
        """Calculate suspend/resume metrics.

        Args:
            model: In-memory representation of a system trace.

        Returns:
            Set of metrics results for this test case.
        """

        def unwrap(e: Optional[trace_time.TimePoint]) -> trace_time.TimePoint:
            if e is None:
                raise ValueError("expected some, but got None")
            return e

        suspend_events = filter_events(model)
        suspend_time = trace_time.TimeDelta(0)
        for se in suspend_events:
            if se.duration is not None:
                suspend_time += se.duration

        # TODO(https://fxbug.dev/366507238): Find a more robust way of
        # determining the start of the test. Doing so would be valuable
        # for tests that do not include scheduling events.
        trace_start_time = min(
            map(
                lambda e: e.start,
                sum(model.scheduling_records.values(), []),
            )
        )
        event_end_times = map(lambda e: e.end_time(), model.all_events())
        trace_end_time = max(
            [unwrap(e) for e in event_end_times if e is not None]
        )
        total_time = trace_end_time - trace_start_time
        running_time = total_time - suspend_time

        return [
            trace_metrics.TestCaseResult(
                label="UnsuspendedTime",
                unit=trace_metrics.Unit.nanoseconds,
                values=[running_time.to_nanoseconds()],
            ),
            trace_metrics.TestCaseResult(
                label="SuspendTime",
                unit=trace_metrics.Unit.nanoseconds,
                values=[suspend_time.to_nanoseconds()],
            ),
            trace_metrics.TestCaseResult(
                label="SuspendPercentage",
                unit=trace_metrics.Unit.percent,
                values=[(suspend_time / total_time) * 100],
            ),
        ]


def filter_events(
    model: trace_model.Model,
) -> Iterator[trace_model.DurationEvent]:
    """Extract suspend duration events from the provided trace model.

    Args:
        model: In-memory representation of a system trace.

    Returns:
        Iterator of suspend duration events emitted by the power subsystem.

    """
    return trace_utils.filter_events(
        model.all_events(),
        category=_EVENT_CATEGORY,
        name=_EVENT_NAME,
        type=trace_model.DurationEvent,
    )


def make_synthetic_event(
    timestamp_usec: int, pid: int, tid: int, duration_usec: int
) -> trace_model.DurationEvent:
    """Build a synthetic suspend DurationEvent.

    Providing this function enables building fake traces for unittests while
    preventing the event category and name from leaking outside this module.
    """
    return trace_model.DurationEvent.from_dict(
        {
            "cat": _EVENT_CATEGORY,
            "name": _EVENT_NAME,
            "ts": timestamp_usec,
            "pid": pid,
            "tid": tid,
            "dur": duration_usec,
        }
    )
