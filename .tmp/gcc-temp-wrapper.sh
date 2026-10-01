#!/usr/bin/env bash

export TMP="C:/Users/USER/Downloads/EXILE-OS/.tmp/win-temp"
export TEMP="C:/Users/USER/Downloads/EXILE-OS/.tmp/win-temp"
export TMPDIR="C:/Users/USER/Downloads/EXILE-OS/.tmp/win-temp"

exec /c/msys64/mingw64/bin/g++ "$@"
