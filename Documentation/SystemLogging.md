# SystemLog service

`SystemLog` receives bounded local datagram records at `/tmp/portal/syslog`, validates the severity prefix, sanitizes control characters, timestamps accepted records, and appends to `/var/log/system.log`. It retains up to two rotated 1 MiB archives in addition to the active file. Messages that cannot reach the local service can still be emitted through the legacy console/stderr flags.

The LibC `syslog()` implementation submits a capped message via a local datagram and includes the process identifier when `LOG_PID` is requested. The sender does not block waiting for a daemon response; logging loss is possible if the service is not running or the local socket send fails. The service currently provides no retrieval IPC, severity/source query API, privileged filtering model, or kernel-log ingestion. File ownership and mode are controlled by the daemon (`root`, 0640); applications cannot directly write the file unless OS permissions otherwise allow it.

This component uses the existing local socket and filesystem interfaces. It is not a kernel logging pipeline and does not claim cryptographic integrity or tamper resistance. Rotation is size bounded; a full or read-only filesystem produces a diagnostic to the service's stderr/debug output and drops the record rather than filling other storage.
