# Timekeeper integration tests

Tests Timekeeper interactions against fake time sources and fake RTC devices.
During an integration test, Timekeeper launches and connects to
`dev_time_source`, a fake time source that forwards connections to the
integration test.

The test component implements a number of services:
 * `fuchsia.time.Maintenance` - provides timekeeper with a handle to a clock
 created by the test component.
 * `test.time.TimeSourceControl` - allows a `dev_time_source` launched
 by Timekeeper to forward the `fuchsia.time.external.*` connections it receives from
 Timekeeper to the test component.

In addition, the test launches a mock Cobalt component, which makes
`fuchsia.metrics.MetricEventLoggerFactory` available to Timekeeper.

## Fake-clock tests
The tests in `tests/faketime` also use `//src/lib/fake-clock`. This allows
the test to control the monotonic time as observed by Timekeeper under test. To
support this, the fake-clock tests additionally launch the fake clock manager
component, which makes `fuchsia.testing.FakeClock` available to Timekeeper.

