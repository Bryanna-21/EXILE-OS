# Exile OS Application Audit

The application audit report is in `Documentation/Project-Audits/APPLICATION_AUDIT.md`.

## Status and limitations

This is a proposed, source-based audit. A successful patch-application check does not establish that the optional patch compiles or works at runtime. The report lists build, boot, GUI, hardware, and other validation that remains outstanding. Review its findings and limitations before treating any application as verified.

## Optional Exile Store patch

The optional patch is `patches/0001-exilestore-native-lifetime-status-installpath.patch`.

It passes `git apply --check --whitespace=error` against the reviewed checkout. It has not been natively compiled or runtime-tested. Do not apply it without reviewing the diff and running the appropriate build and tests.

To inspect it:

```sh
git apply --stat patches/0001-exilestore-native-lifetime-status-installpath.patch
git apply --check --whitespace=error patches/0001-exilestore-native-lifetime-status-installpath.patch
```

The patch is provided separately and is not applied by this audit submission.
