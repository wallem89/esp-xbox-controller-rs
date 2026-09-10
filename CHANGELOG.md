# Changelog

All notable changes to `esp-xbox-controller` are documented here.

## [Unreleased]

### Changed

- Breaking: `ControllerConfig::new` requires a third `supervision_timeout`
  argument. Pass `Duration::from_millis(500)` to preserve the previous default;
  the `with_supervision_timeout` builder has been removed.

## [0.3.0] - 2026-09-09

### Added

- Support for encrypted
- `run_with_events` with report and session-disconnection events and an activity
  predicate that suspends idle disconnection while relevant input remains held.
- Runtime-independent idle policy tests; existing `run` behavior is preserved.

### Changed

- Support for esp-hal 1.2.1

## [0.2.0] - 2026-08-21

### Added

- Added the ESP32-C3 BLE connection API with `ControllerConfig` and
  `ControllerSelector`.
- Added configurable idle disconnection and controller keepalive handling.
- Added automatic scanning, pairing, HID discovery, notification subscription,
  reconnection, and connection rumble.
- Added minimal and task/channel-based ESP32-C3 examples.

### Changed

- Moved transport-independent parsing into the separate
  `xbox-controller-core` crate and re-exported its public API.
- Improved reconnection behavior and HID-over-GATT initialization.

## [0.1.0] - 2026-08-08

- Initial repository release. The publishable package at this point was
  `xbox-controller-core`.
