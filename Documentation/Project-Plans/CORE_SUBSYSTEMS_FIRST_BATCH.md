# First implementation batch: service startup dependencies

This work extends the inherited SystemServer rather than adding a second service manager. Service definitions remain INI groups in `/etc/SystemServer.ini`; existing service process, socket activation, mode selection, and account handling remain in `Service`.

## Dependencies

A service may declare a comma-separated list of other service group names:

```ini
[LookupServer]
Dependencies=NetworkServer
```

Dependencies are literal names, never shell commands. SystemServer calculates a stable dependency-first order before opening service sockets. Missing or mode-disabled dependencies block dependents. Duplicate active service names and circular graphs (including self-dependencies) are rejected. Duplicate dependency entries do not cause duplicate process starts. Independent services retain the stable input scan order. If socket setup or activation returns an error, dependent services are blocked for this activation pass, including transitive dependents.

There is an important process boundary limitation: `Service::activate()` currently treats a successful `fork()` as activation success. The parent does not receive an `exec()` handshake. A child may fail setup/exec after fork, so dependent processes can be started before that child's later SIGCHLD failure is observed. Runtime readiness and dependency-aware recovery are not implemented.

## Existing functionality and current gaps

- Implemented in existing SystemServer: service group registration, process fork/exec, single-instance PID tracking, SIGCHLD/waitpid exit collection, legacy KeepAlive restart cap, socket activation, system-mode selection, and dependency-first activation.
- Graph tests added for empty graphs, independent entries/order, dependency chains, shared dependencies, duplicate dependencies, missing references, duplicate service names, self-dependency and cycles. They have not been run in this environment.
- Not implemented: explicit lifecycle-state API, configurable restart policies/backoff, process readiness handshake, dependency-aware runtime restart/shutdown, graceful service stop/timeouts, central logging daemon, structured persistent log store, filtering API, rotation, and retention limits.
- Logging audit: LibC `syslog()` is present but `syslog.cpp` says it does not connect to a syslog server; output is conditional on console/stderr flags. `dbgln` is not a retained system-log service.
- Storage and network work is intentionally outside this batch.

## Verification status

No OS build, CTest, QEMU or physical-hardware run is recorded here. The documented validation commands in a configured environment are:

```sh
cmake -S Meta/CMake/Superbuild -B Build/superbuild -GNinja
cmake --build Build/superbuild
ctest --test-dir Build/superbuild --output-on-failure
Meta/exile.sh test
```

The repository's build instructions name `Meta/exile.sh` as the supported wrapper; `Meta/serenity.sh` was not present at the inspected path.
