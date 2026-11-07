# SystemLog service

`SystemLog` receives bounded local datagram records at `/tmp/portal/syslog`, validates severity, sanitizes control characters, timestamps accepted records, and stores each as `timestamp severity source message` in `/var/log/system.log`. The active file is capped at 1 MiB and rotates to two 1 MiB archives. Writes are append-only and fsynced; errors are reported to the daemon's stderr/debug output. The socket is mode 0622: unprivileged services may submit records but cannot read or replace the log. The daemon creates `/tmp/portal` and `/var/log` before locking down its unveil rules.

LibC `syslog()` submits a capped message with source identification and optional PID to the datagram endpoint. It falls back to console/stderr only when the caller requested `LOG_CONS` or `LOG_PERROR`; POSIX's void API cannot report delivery status to callers. Delivery can be lost when the service is unavailable, the datagram socket is full, or storage fails.

## Limitations

The initial service has no query/retrieval protocol, severity/source/time filtering API, kernel-log ingestion, audit integrity, or durable acknowledgment. The persistent log is world-readable (0644) to support diagnostics; deployments requiring stronger confidentiality need an explicit group/IPC authorization policy. Rotation errors prevent accepting that record and are reported; there is no remote or network logging.

The SystemLog service is registered in `Base/etc/SystemServer.ini` and built as a required system service. Boot order among unrelated services is not guaranteed by the INI group order. SystemServer's dependency graph can be used to order a service after SystemLog if it needs log availability, but logging callers must still tolerate service startup/restart gaps.
