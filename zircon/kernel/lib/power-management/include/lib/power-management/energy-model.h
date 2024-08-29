// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_ENERGY_MODEL_H_
#define ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_ENERGY_MODEL_H_

#include <lib/fit/function.h>
#include <lib/stdcompat/array.h>
#include <lib/stdcompat/span.h>
#include <lib/stdcompat/utility.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <stdint.h>
#include <stdlib.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/syscalls-next.h>
#include <zircon/types.h>

#include <cstdint>
#include <limits>
#include <string_view>

#include <fbl/intrusive_single_list.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <fbl/vector.h>

namespace power_management {

// forward declaration.
class PowerModel;

// Enum representing supported control interfaces.
enum class ControlInterface : uint64_t {
  kArmWfi = ZX_PROCESSOR_POWER_CONTROL_ARM_WFI,
  kArmPsci = ZX_PROCESSOR_POWER_CONTROL_ARM_PSCI,
  kRiscvSbi = ZX_PROCESSOR_POWER_CONTROL_RISCV_SBI,
  kRiscvWfi = ZX_PROCESSOR_POWER_CONTROL_RISCV_WFI,
  kCpuDriver = ZX_PROCESSOR_POWER_CONTROL_CPU_DRIVER,
};

// List of support control interfaces.
static constexpr auto kSupportedControlInterfaces = cpp20::to_array(
    {ControlInterface::kArmPsci, ControlInterface::kArmWfi, ControlInterface::kRiscvSbi,
     ControlInterface::kRiscvWfi, ControlInterface::kCpuDriver});

// Returns whether the interface is a supported or not.
constexpr bool IsSupportedControlInterface(zx_processor_power_control_t interface) {
  return std::find(kSupportedControlInterfaces.begin(), kSupportedControlInterfaces.end(),
                   static_cast<ControlInterface>(interface)) != kSupportedControlInterfaces.end();
}

// Kernel representation of `zx_processor_power_level_t` with useful accessors and option support.
class PowerLevel {
 public:
  enum Type {
    // Entity is not eligible for active work.
    kIdle,

    // Entity is eligible for work, but the rate at which work is completed is determined by the
    // active power level.
    kActive,
  };

  constexpr PowerLevel() = default;
  explicit constexpr PowerLevel(uint8_t level_index, const zx_processor_power_level_t& level)
      : options_(level.options),
        control_(static_cast<ControlInterface>(level.control_interface)),
        control_argument_(level.control_argument),
        processing_rate_(level.processing_rate),
        power_coefficient_nw_(level.power_coefficient_nw),
        level_(level_index) {
    memcpy(name_.data(), level.diagnostic_name, name_.size());
    size_t end = std::string_view(name_.data(), name_.size()).find('\0');
    name_len_ = end == std::string_view::npos ? ZX_MAX_NAME_LEN : end;
  }

  // Power level type. Idle and Active power levels are orthogonal, that is, an entity may be idle
  // while keepings its active power level unchanged. This means that the actual power level of
  // an entity should be determined by the tuple <Idle Power Level*, Active Power Level>, where if
  // `Idle Power Level`is absent then that means that the entity is active and the active power
  // level should be used.
  //
  // This situation happens for example, when a CPU transitions from an active power level A (which
  // may be interpreted as a known OPP or P-State) into an idle state, such as suspension, idle
  // thread or even powering it off.
  constexpr Type type() const { return processing_rate_ == 0 ? Type::kIdle : kActive; }

  // Processing rate when this power level is active. This is key to determining the available
  // bandwidth of the entity.
  constexpr uint64_t processing_rate() const { return processing_rate_; }

  // Relative to the system power consumption, determines how much power is being consumed at this
  // level. This allows determining if this power level should be a candidate when operating under a
  // given energy budget.
  constexpr uint64_t power_coefficient_nw() const { return power_coefficient_nw_; }

  // ID of the interface handling transitions for TO this power level.
  constexpr ControlInterface control() const { return control_; }

  // Argument to be interpreted by the control interface in order to transition to this level.
  //
  // The control interface is only aware of this arguments, and power levels are identified by this
  // argument.
  constexpr uint64_t control_argument() const { return control_argument_; }

  // This level may be transitioned in a per cpu basis, without affecting other entities in the same
  // power domain.
  constexpr bool TargetsCpus() const {
    return (options_ & ZX_PROCESSOR_POWER_LEVEL_OPTIONS_DOMAIN_INDEPENDENT) != 0;
  }

  // This level may be transitioned in a per power domain basis, that is, all other entities in the
  // power domain will be transitioned together.
  //
  // This means that underlying hardware elements are share and it is not possible to transition a
  // single member of the power domain.
  constexpr bool TargetsPowerDomain() const {
    return (options_ & ZX_PROCESSOR_POWER_LEVEL_OPTIONS_DOMAIN_INDEPENDENT) == 0;
  }

  // Name used to identify this power level, for diagnostic purposes.
  constexpr std::string_view name() const { return {name_.data(), name_len_}; }

  // Power Level as understood from the original model perspective.
  constexpr uint8_t level() const { return level_; }

 private:
  // Options.
  zx_processor_power_level_options_t options_ = {};

  // Control interface used to transition to this level.
  ControlInterface control_ = {};

  // Argument to be provided to the control interface.
  uint64_t control_argument_ = 0;

  // Processing rate.
  uint64_t processing_rate_ = 0;

  // Power coefficient in nanowatts.
  uint64_t power_coefficient_nw_ = 0;

  std::array<char, ZX_MAX_NAME_LEN> name_ = {};
  size_t name_len_ = 0;

  // Level as described in the model shared with user.
  uint8_t level_ = 0;
  [[maybe_unused]] std::array<uint8_t, 7> reserved_ = {};
};

// Represents an entry in a transition matrix, where the position in the matrix denotes
// the source and target power level. This constructs just denotes the properties of that
// cell.
class PowerLevelTransition {
 public:
  // Returns an invalid transition.
  static constexpr PowerLevelTransition Invalid() { return {}; }

  constexpr PowerLevelTransition() = default;
  explicit constexpr PowerLevelTransition(const zx_processor_power_level_transition_t& transition)
      : latency_(transition.latency), energy_cost_(transition.energy_nj) {}

  // Latency for transitioning from a given level to another.
  constexpr zx::duration latency() const { return latency_; }

  // Energy cost in nano joules(nj) for transition from a given level to another.
  constexpr uint64_t energy_cost() const { return energy_cost_; }

  // Whether the transition is valid or not.
  explicit constexpr operator bool() {
    return latency_ != Invalid().latency() && energy_cost_ != Invalid().energy_cost_;
  }

 private:
  // Time required for the transition to take effect. In some cases it may mean for the actual
  // voltage to stabilize.
  zx::duration latency_ = zx::duration::infinite();

  // Amount of energy consumed to perform the transition.
  uint64_t energy_cost_ = std::numeric_limits<uint64_t>::max();
};

// Represents a view of the `zx_processor_power_level_transition_t` array as
// a matrix. As per a view's concept, this view is only valid so long the original
// object remains valid and it's tied to its lifecycle.
//
// Additionally transition matrix are required to be squared matrixes, since
// they describe transition from every existent level to every other level.
struct TransitionMatrix {
 public:
  constexpr TransitionMatrix(const TransitionMatrix& other) = default;

  constexpr cpp20::span<const PowerLevelTransition> operator[](size_t index) const {
    return transitions_.subspan(index * num_rows_, num_rows_);
  }

 private:
  friend PowerModel;
  constexpr TransitionMatrix(cpp20::span<const PowerLevelTransition> transitions, size_t num_rows)
      : transitions_(transitions), num_rows_(num_rows) {
    ZX_DEBUG_ASSERT(transitions_.size() != 0);
    ZX_DEBUG_ASSERT(num_rows_ != 0);
    ZX_DEBUG_ASSERT(transitions_.size() % num_rows_ == 0);
    ZX_DEBUG_ASSERT(transitions.size() / num_rows_ == num_rows_);
  }

  const cpp20::span<const PowerLevelTransition> transitions_;
  const size_t num_rows_;
};

// A `Power Model` describes information related to, how many levels are there,
// what interface should be used for transitioning to any particular level, etc.
//
// A `PowerModel` is constant and will never change once initialized, the update
// model for a `PowerModel` consists on creating a new one, and increasing
// a generation number.
class PowerModel {
 public:
  static zx::result<PowerModel> Create(
      cpp20::span<const zx_processor_power_level_t> levels,
      cpp20::span<const zx_processor_power_level_transition_t> transitions);

  PowerModel() = default;
  PowerModel(const PowerModel&) = delete;
  PowerModel(PowerModel&&) = default;

  // All power levels described in the model, sorted by processing power and energy consumption.
  //
  // (1) The processing rate of power level i is less or equal than the processing rate of power
  // level j, where i <= j.
  //
  // (2) The energy cost of power level i is less or equal than the processing rate of power level
  // j, where i <= j.
  constexpr cpp20::span<const PowerLevel> levels() const { return power_levels_; }

  // Following the same rules as `levels()` but returns only the set of power levels whose type is
  // `PowerLevel::Type::kIdle`. This set may be empty.
  constexpr cpp20::span<const PowerLevel> idle_levels() const {
    return levels().subspan(0, idle_power_levels_);
  }

  // Following the same rules as `levels()` but returns only the set of power levels whose type is
  // `PowerLevel::Type::kActive`. This set may be empty.
  constexpr cpp20::span<const PowerLevel> active_levels() const {
    return levels().subspan(idle_power_levels_);
  }

  // Returns a transition matrix, where the entry <i,j> represents the transition costs for
  // transitioning from i to j.
  constexpr TransitionMatrix transitions() const {
    return TransitionMatrix(transitions_, power_levels_.size());
  }

  std::optional<size_t> FindPowerLevel(ControlInterface interface_id,
                                       uint64_t control_argument) const;

 private:
  PowerModel(fbl::Vector<PowerLevel> levels, fbl::Vector<PowerLevelTransition> transitions,
             fbl::Vector<size_t> control_lookup, size_t idle_levels)
      : power_levels_(std::move(levels)),
        transitions_(std::move(transitions)),
        control_lookup_(std::move(control_lookup)),
        idle_power_levels_(idle_levels) {}

  fbl::Vector<PowerLevel> power_levels_;
  fbl::Vector<PowerLevelTransition> transitions_;
  fbl::Vector<size_t> control_lookup_;
  size_t idle_power_levels_ = 0;
};

// A `PowerDomain` establishes the relationship between a CPU and a `PowerModel`.
// A `PowerDomain` may not be modified after its initialization, except for the associated
// `cpus_`.
// A `PowerDomain` ID represents a link between a set of cpus and a power model.
// A `PowerDomain` is considered active if at least once CPU is part of the domain.
class PowerDomain : public fbl::RefCounted<PowerDomain>,
                    public fbl::SinglyLinkedListable<fbl::RefPtr<PowerDomain>> {
 public:
  PowerDomain(uint32_t id, zx_cpu_set_t cpus, PowerModel model)
      : cpus_(cpus), id_(id), power_model_(std::move(model)) {}

  // ID representing the relationship between a set of CPUs and a power model.
  constexpr uint32_t id() const { return id_; }

  // Set of CPUs associated with `model()`.
  constexpr const zx_cpu_set_t& cpus() const { return cpus_; }

  // Model describing the behavior of the power domain.
  constexpr const PowerModel& model() const { return power_model_; }

 private:
  const zx_cpu_set_t cpus_;
  const uint32_t id_;
  const PowerModel power_model_;
};

// `PowerDomainRegistry` provides a starting point for looking at any
// of the previously registered power domains.
//
// This class also provides the mechanism for updating existing `PowerDomain` entries, by means of
// replacing. For atomic updates external synchronization is required.
//
// In practice, there will be a single instance of this object in the kernel.
class PowerDomainRegistry {
 public:
  // Register `power_domain` with this registry.
  //
  // A `CpuPowerDomainAccessor` must provide the following contract:
  //
  //  // `cpu_num` is a valid cpu number that falls within `cpu_set_t` bits.
  //  // `new_domain` new `PowerDomain` for `cpu_num`. If `nullptr` then
  //  //  current domain should be cleared.
  //  void operator()(size_t cpu_num, fbl::RefPtr<PowerDomain>& new_domain)
  //
  template <typename CpuPowerDomainAccessor>
  zx::result<> Register(fbl::RefPtr<PowerDomain> power_domain,
                        CpuPowerDomainAccessor&& update_domain) {
    return UpdateRegistry(std::move(power_domain), update_domain);
  }

  // Visit each registered `PowerDomain`.
  template <typename Visitor>
  void Visit(Visitor&& visitor) {
    for (const auto& domain : domains_) {
      visitor(domain);
    }
  }

 private:
  static constexpr size_t kBitsPerBucket = ZX_CPU_SET_BITS_PER_WORD;
  static constexpr size_t kBuckets = ZX_CPU_SET_MAX_CPUS / ZX_CPU_SET_BITS_PER_WORD;

  // Updates the registry list, by possibly removing a domain registered with the same id.
  // If a domain is replaced.
  zx::result<> UpdateRegistry(
      fbl::RefPtr<PowerDomain> power_domain,
      fit::inline_function<void(size_t, fbl::RefPtr<PowerDomain>)> update_cpu_power_domain);

  fbl::SinglyLinkedList<fbl::RefPtr<PowerDomain>> domains_;
};

}  // namespace power_management

#endif  // ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_ENERGY_MODEL_H_
