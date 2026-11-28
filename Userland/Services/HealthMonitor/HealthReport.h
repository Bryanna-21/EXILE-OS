/*
 * Copyright (c) 2026, Exile OS contributors
 * SPDX-License-Identifier: BSD-2-Clause
 */
#pragma once

#include <cstdint>
#include <string>

namespace Exile::HealthMonitor {

enum class HealthState : uint8_t { Unknown, Healthy, Warning, Critical };
enum class Severity : uint8_t { Informational, Warning, Critical };
enum class Component : uint8_t { Memory, Storage, SystemService, ResourcePressure, CriticalComponent };

struct HealthReport {
    Component component { Component::CriticalComponent };
    HealthState state { HealthState::Unknown };
    Severity severity { Severity::Informational };
    // CLOCK_MONOTONIC milliseconds; zero means timestamp unavailable.
    uint64_t observed_at_ms { 0 };
    std::string reason;
};

struct HealthObservation {
    Component component { Component::CriticalComponent };
    HealthState state { HealthState::Unknown };
    std::string reason;
    uint8_t measured_percent { 0 };
    bool has_measurement { false };
};

}
