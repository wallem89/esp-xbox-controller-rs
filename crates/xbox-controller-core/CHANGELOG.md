# Changelog

All notable changes to `xbox-controller-core` are documented here.

## [0.1.2] - 2026-08-21

### Changed

- Restored the parser as its own publishable workspace crate.
- Preserved the existing parsing and controller-state API without introducing
  ESP, Embassy, or Bluetooth-stack dependencies.

## [0.1.1] - 2026-08-12

### Changed

- Workspace maintenance release; the parser API was unchanged.

## [0.1.0] - 2026-08-08

- Initial release of the allocation-free, `no_std` Xbox BLE report parser.
