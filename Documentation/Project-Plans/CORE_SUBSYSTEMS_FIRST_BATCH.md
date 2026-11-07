# First implementation batch: service startup dependencies

This change extends the existing `SystemServer` rather than adding a second service manager. The current service definitions remain INI groups in `/etc/SystemServer.ini`, and existing `KeepAlive`, socket activation, system-mode selection, user account, and process-launch behavior remain in `Service`.

## `Dependencies` field

A service may declare a comma-separated list of other service group names:

```ini
[LookupServer]
Dependencies=NetworkServer
```

Dependencies are names, not shell expressions or commands. SystemServer starts configured services in a stable dependency-first order. A reference to a service that is absent from the current configuration or disabled in the active system mode blocks that service. A blocked dependency propagates to dependents. A cycle is rejected before service sockets are created or processes are started. If socket setup or process activation fails, dependent services are not launched in that activation batch.

Dependency declarations are currently resolved within one `SystemServer` configuration batch. They do not express readiness probes: “started” means the dependency process was successfully spawned, not that it has completed application-level initialization or is healthy. Restarting a dependency does not yet restart its dependents.

## Verification status and scope

- Dependency-first startup, missing/disabled dependency blocking, failure propagation, and cycle rejection: implemented in `Userland/Services/SystemServer/main.cpp`; not yet verified by an OS build or runtime test in this batch.
- `NetworkServer` and `LookupServer`: existing implementations reused; LookupServer is configured to start after NetworkServer. This ordering does not imply network connectivity.
- Storage: existing kernel block devices, partition parsing, and ext2/FAT/ISO9660 filesystem implementations remain the storage foundation. No new storage daemon, automatic mounting policy, SMART, or encryption support is added here.
- Networking: existing kernel TCP/IP stack, wired adapters, NetworkServer, DHCPClient, and LookupServer remain in place. Wi-Fi and enforced firewall functionality are not provided.
- QEMU and physical hardware verification: not performed.

Build and test commands, when the supported cross-toolchain is available, are the repository commands:

```sh
cmake -S Meta/CMake/Superbuild -B Build/superbuild -GNinja
cmake --build Build/superbuild
./Meta/serenity.sh test
```
