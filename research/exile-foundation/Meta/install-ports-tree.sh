#!/usr/bin/env bash

DESTDIR="${EXILE_SOURCE_DIR}/Build/${EXILE_ARCH}/Root/usr"

git ls-files --full-name "${EXILE_SOURCE_DIR}/Ports" | \
  rsync -raHL \
    --chown=0:0 --inplace --update \
    --files-from=- \
    --exclude="Ports/.hosted_defs.sh" \
    "${EXILE_SOURCE_DIR}" "${DESTDIR}"
