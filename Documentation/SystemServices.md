# System service management: current behavior

SystemServer reads service definitions from INI configuration files. Each INI group names one service. Existing fields include `Executable`, `Arguments`, `StdIO`, `Priority`, `KeepAlive`, `Lazy`, `User`, `WorkingDirectory`, `Environment`, `SystemModes`, `MultiInstance`, `AcceptSocketConnections`, `Socket`, and `SocketPermissions`.

## Dependency ordering

`Dependencies` is an optional comma-separated list of service group names. Names are literal identifiers, not shell expressions. Within one configuration activation, dependencies start before dependents. Unknown or mode-disabled dependencies block the service and its dependents. Duplicate service names and cycles (including self-dependency) are rejected before creating sockets or starting processes. Duplicate dependency entries do not launch a service more than once. Independent entries preserve configuration order as far as the stable topological scan permits.

If socket setup or `fork()` activation fails, the service is marked as failed for the remainder of this activation pass. Its dependents are skipped, and that blocked result is propagated transitively. One known limitation remains: a successful `fork()` is currently treated as activation success before the child has proved that `exec()` succeeded. A process that exits immediately after fork may therefore fail after dependents have already been launched.

## Process lifecycle and restart limitations

The existing `Service` tracks a PID for single-instance children and handles process exit through SystemServer's `SIGCHLD`/`waitpid` path. Its legacy `KeepAlive=true` behavior can relaunch a service, with a small rapid-failure attempt cap. It does not provide lifecycle state queries, configurable restart policy/backoff, readiness checks, shutdown ordering, or graceful stop/timeout coordination. A dependent already running is not stopped or restarted when its dependency exits.

## Logging limitations

`LibC` exposes the POSIX `syslog()` API, but the inspected `syslog.cpp` implementation explicitly does not connect to a logging daemon and writes output only for `LOG_CONS` or `LOG_PERROR`. SystemServer's `dbgln`/`warnln` messages are not retained in a bounded structured log store. No centralized log collection, retrieval API, rotation, or retention policy is implemented by this batch.

## Verification

The service graph tests cover empty configurations, stable ordering, absent dependency propagation, cycles, self-dependencies, duplicate service identifiers, duplicate dependency entries, long chains, and multiple consumers of one provider. Tests were added but not executed in this environment. No claim is made for OS build, CTest, QEMU, or physical hardware validation.

Current exact limitation: service supervision enhancements and centralized logging remain unimplemented; this document reports the existing implementation rather than describing those future features as complete.
