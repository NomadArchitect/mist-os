// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_MONITOR_MONITOR_H_
#define SRC_DEVELOPER_MEMORY_MONITOR_MONITOR_H_

#include <fidl/fuchsia.memorypressure/cpp/fidl.h>
#include <fuchsia/hardware/ram/metrics/cpp/fidl.h>
#include <fuchsia/memory/inspection/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/trace/observer.h>
#include <lib/zx/socket.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <memory>

#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/metrics/digest.h"
#include "src/developer/memory/monitor/high_water.h"
#include "src/developer/memory/monitor/logger.h"
#include "src/developer/memory/monitor/memory_monitor_config.h"
#include "src/developer/memory/monitor/metrics.h"
#include "src/lib/fxl/command_line.h"

namespace monitor {

namespace test {
class MemoryBandwidthInspectTest;
}  // namespace test

class Monitor : public fuchsia::memory::inspection::Collector,
                public fidl::Server<fuchsia_memorypressure::Watcher> {
 public:
  Monitor(std::unique_ptr<sys::ComponentContext> context, const fxl::CommandLine& command_line,
          async_dispatcher_t* dispatcher, bool send_metrics, bool watch_memory_pressure,
          memory_monitor_config::Config config,
          std::unique_ptr<memory::CaptureMaker> capture_maker);
  ~Monitor() override = default;

  // For memory bandwidth measurement, SetRamDevice should be called once
  void SetRamDevice(fuchsia::hardware::ram::metrics::DevicePtr ptr);

  // Writes a memory capture and the bucket definition to |socket| in JSON,
  // in UTF-8.
  // See the fuchsia.memory.inspection FIDL library for a
  // description of the format of the JSON.
  void CollectJsonStats(zx::socket socket) override;

  void CollectJsonStatsWithOptions(
      fuchsia::memory::inspection::CollectorCollectJsonStatsWithOptionsRequest request) override;

  // fuchsia_memorypressure::Watcher implementation; this callback gets called
  // whenever a memory pressure change is signaled.
  void OnLevelChanged(OnLevelChangedRequest& request,
                      OnLevelChangedCompleter::Sync& completer) override;

  static const char kTraceName[];

 private:
  void CollectJsonStatsWithOptions(zx::socket socket);

  void PublishBucketConfiguration();

  void CreateMetrics(const std::vector<memory::BucketMatch>& bucket_matches);

  void UpdateState();

  void StartTracing();
  void StopTracing();

  void SampleAndPost();
  void MeasureBandwidthAndPost();
  void PeriodicMeasureBandwidth();
  static void PrintHelp();
  inspect::Inspector Inspect(const std::vector<memory::BucketMatch>& bucket_matches);

  void GetDigest(const memory::Capture& capture, memory::Digest* digest);

  void OnLevelChanged(pressure_signaler::Level level);

  std::unique_ptr<memory::CaptureMaker> capture_maker_;
  std::unique_ptr<HighWater> high_water_;
  uint64_t prealloc_size_;
  zx::vmo prealloc_vmo_;
  bool logging_;
  bool tracing_;
  zx::duration delay_;
  zx_handle_t root_;
  async_dispatcher_t* dispatcher_;
  std::unique_ptr<sys::ComponentContext> component_context_;
  fuchsia::metrics::MetricEventLoggerSyncPtr metric_event_logger_;
  fidl::BindingSet<fuchsia::memory::inspection::Collector> bindings_;
  trace::TraceObserver trace_observer_;
  memory_monitor_config::Config config_;
  inspect::ComponentInspector inspector_;
  Logger logger_;
  std::unique_ptr<Metrics> metrics_;
  std::unique_ptr<memory::Digester> digester_;
  std::mutex digester_mutex_;
  fuchsia::hardware::ram::metrics::DevicePtr ram_device_;
  uint64_t pending_bandwidth_measurements_ = 0;
  pressure_signaler::Level level_;
  fidl::Client<fuchsia_memorypressure::Provider> pressure_provider_;

  // Imminent OOM monitoring
  void WaitForImminentOom();
  void WatchForImminentOom();

  zx_handle_t imminent_oom_event_handle_;
  async::TaskClosureMethod<Monitor, &Monitor::WatchForImminentOom> watch_task_{this};
  async::Loop imminent_oom_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};

  friend class test::MemoryBandwidthInspectTest;
  FXL_DISALLOW_COPY_AND_ASSIGN(Monitor);
};

}  // namespace monitor

#endif  // SRC_DEVELOPER_MEMORY_MONITOR_MONITOR_H_
