# Service definition reference

SystemServer reads service definitions from INI configuration files. Each INI group names one service. Existing fields include `Executable`, `Arguments`, `StdIO`, `Priority`, `KeepAlive`, `Lazy`, `User`, `WorkingDirectory`, `Environment`, `SystemModes`, `MultiInstance`, `AcceptSocketConnections`, `Socket`, and `SocketPermissions`.

## Dependencies

`Dependencies` is an optional comma-separated list of service group names. Names are literal identifiers; they are not shell commands. For example:

```ini
[LookupServer]
Executable=/bin/LookupServer
Dependencies=NetworkServer
```

Within one SystemServer configuration activation, dependencies are started before dependents. Unknown, disabled-in-mode, or failed dependencies block the dependent service. Circular dependency graphs are rejected before sockets are set up or child processes are launched. Avoid whitespace around identifiers; use the exact INI group name.

The current behavior reports process spawn success, not application readiness. Dependencies across separate configuration batches, readiness checks, dependency-aware restart, lifecycle status APIs, and stop/restart commands are not implemented by this contract. The existing `KeepAlive` restart policy remains in effect and has not been replaced by this change.

Environment entries continue to use the existing SystemServer format and must not be interpreted as shell commands. Resource limits, health checks, and structured service logs are not yet supported by this definition format.
