#!/bin/sh
# After install: reload udev rules so keyboard access works without logging out.
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules || true
  udevadm trigger --subsystem-match=input --subsystem-match=misc || true
fi
exit 0
