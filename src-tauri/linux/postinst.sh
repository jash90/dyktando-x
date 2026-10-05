#!/bin/sh
# Po instalacji: przeładuj reguły udev, żeby dostęp do klawiatury działał bez wylogowania.
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules || true
  udevadm trigger --subsystem-match=input --subsystem-match=misc || true
fi
exit 0
