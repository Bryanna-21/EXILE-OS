# Health and storage monitoring services

## Integration contract

The monitor services consume existing kernel sysfs interfaces; this batch does
not add a parallel kernel telemetry API.

- `/sys/kernel/memstat` is the existing `SysFSMemoryStatus` JSON node. Its
  `physical_allocated` and `physical_available` fields are physical-page counts
  from `MemoryManager::get_system_memory_info()`.
- `/sys/devices/storage/<LUN>/last_lba`, `sector_size`, and `command_set` are
  the existing storage-device attributes. `last_lba` is the final zero-based
  logical block, so capacity uses `(last_lba + 1) * sector_size` with overflow
  validation.
- The inspected kernel does not expose SMART health, temperatures, per-device
  read/write error counters, CPU pressure, or a service-health registry. Those
  values must remain `Unknown` until a real provider is implemented.

A listed storage directory proves only enumeration. It is not evidence that a
storage device is healthy. The health service therefore reports the storage
component as `Unknown`, even when one or more devices are enumerated. The
StorageMonitor can report capacity metadata and discovery transitions, but its
`DeviceHealth` and I/O error fields remain `Unknown` with the current kernel
contract.

## Event delivery

Health and storage state changes are sent best-effort as bounded `<5>...`
AF_LOCAL datagrams to `/tmp/portal/syslog`, the existing SystemLog endpoint.
The datagrams are informational log events, not a durable health-state IPC or a
service-health registry. If SystemLog is not running or the socket is
unavailable, the monitor continues and retains its normal debug/warning output.

Both services require read-only access to their respective sysfs paths. They
request only the `unix` pledge capability needed to send the existing local
socket datagrams. No write access to sysfs is granted.

## Host validation

From the repository root, run:

```sh
bash Tests/Services/Monitoring/run-host-tests.sh
```

The host tests exercise threshold boundaries, unknown-state behavior, malformed
memory telemetry, storage capacity overflow, discovery failure/recovery,
device disappearance, and event sanitization. They use temporary fixture files;
they do not claim to test physical hardware or the SerenityOS kernel runtime.
