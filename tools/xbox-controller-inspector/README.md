# Xbox Controller Inspector

`xbox-controller-inspector` is an Ubuntu AMD64 desktop tool for diagnosing an
Xbox Wireless Controller over Bluetooth Low Energy. It displays the controller
address and firmware revision, visualizes buttons, sticks, and triggers, shows
the raw HID input report, and can trigger all four vibration motors.

## Install

```console
cargo install xbox-controller-inspector
xbox-controller-inspector
```

The inspector currently targets Xbox Wireless Controller models 1914 and 1708
with recent BLE-capable firmware.

## Linux requirements

- Ubuntu AMD64 (the supported platform)
- BlueZ installed and its Bluetooth service running
- An enabled Bluetooth adapter
- Permission to scan for and connect to BLE devices through BlueZ; depending on
  the system policy, the user may need polkit authorization
- Read/write access to the controller's `/dev/input/event*` device for live
  controls and force feedback (normally granted through the active desktop
  session's `uaccess` policy)
- Native build packages used by BlueZ and the GUI stack (on Ubuntu:
  `libdbus-1-dev`, `pkg-config`, `libudev-dev`, `libxkbcommon-dev`, and a C
  compiler)

Put the controller in pairing mode before pressing **Scan**. The Scan button
becomes **Stop scanning** while BlueZ discovery is active. Connecting registers
a temporary BlueZ agent that automatically accepts pairing for the selected
device, but only when its advertised name contains `Xbox`; the resulting bond is
marked trusted for later reconnections. BLE privacy means the displayed
advertising address can be random and may change; it is not guaranteed to be the
permanent hardware MAC.
On Linux, BlueZ normally consumes the HID-over-GATT reports and exposes them
through evdev. The inspector therefore uses GATT for identity information and
evdev for live controls and vibration.

## Development

From the repository root:

```console
cargo run -p xbox-controller-inspector
```

The package uses a versioned dependency with a local path override for workspace
development. Cargo removes the path when packaging, so the published package
resolves `xbox-controller-core = "0.1"` from crates.io.

## License

Licensed under either Apache License 2.0 or MIT, at your option.
