# esp32-c3-example

Experimental `no_std` demo for a Seeed Studio XIAO ESP32-C3. It uses `esp-hal`,
Embassy through `esp-rtos`, `esp-radio`, and TrouBLE to scan for an Xbox
controller, connect and pair, discover HID, subscribe to input notifications,
and parse them with `xbox-controller-core`.

Despite its name, the `esp-bootloader-esp-idf` dependency does not turn this into
an ESP-IDF application. It only emits application metadata in the image format
expected by the ESP-IDF-compatible second-stage bootloader used by `espflash`.
The firmware remains `no_std`, bare-metal, and based on `esp-hal`.

The primary target is Xbox Wireless Controller **Model 1914**. Use firmware
`5.13.3143.0` or newer as the oldest known protocol baseline, and preferably
update to the latest firmware offered by the Xbox Accessories app before testing.
That baseline comes from the reference Arduino implementation; this Rust example
has not yet established its own minimum through hardware testing.

Xbox Wireless Controller **Model 1708** is also supported experimentally. It
must have firmware that exposes Bluetooth Low Energy HID; update it with Xbox
Accessories first. Unlike Model 1914, its older BLE hardware can require LE
Legacy Pairing, which this example enables explicitly. Firmware `5.13.3143.0`
has successfully connected, paired, completed HID discovery, and subscribed to
the input Report characteristic on a XIAO ESP32-C3.

The example does **not** generate fake controller notifications. Every byte
shown by `BLE notification` came from the subscribed GATT characteristic.

## Prerequisites

- Rust stable with `rust-src`
- target `riscv32imc-unknown-none-elf` installed (`rustup target add riscv32imc-unknown-none-elf`)
- `espflash` (`cargo install espflash`)
- A XIAO ESP32-C3 connected over USB

From this directory:

```console
cargo run --release
```

Run this command from `examples/esp32-c3-example`, not from the workspace root.
Cargo then loads the example's `.cargo/config.toml`, including the ESP32-C3
target, `espflash` runner, and `-Tlinkall.x` linker script. Without that linker
argument, `rust-lld` reports missing ESP runtime symbols such as
`_dram_data_start`.

Before running, forget the controller on nearby consoles/PCs or turn them off.
Start the firmware, then hold the controller Pair button until the Xbox button
flashes rapidly. Expected progress is:

```text
esp-radio BLE controller initialized
scanning for Xbox Wireless Controller (HID 0x1812)
found compatible Xbox at ...
connecting to ...
connected; starting pairing/encryption
pairing complete: ...
discovering HID service 0x1812
reading HID Information 0x2A4A
reading HID Report Map 0x2A4B
selected Report handle ..., CCCD ...
initial Report value (... bytes): ...
subscribing to input notifications
ready; move a stick or press a button
```

Pairing is deliberately non-bondable because keys are not stored in ESP32 flash
yet. This prevents the controller remembering a key that the board loses on
reset. After resetting the board, put the controller into pairing mode again.
Controllers previously tested with a bondable firmware build may retain its old
stale bond; explicitly enter pairing mode for the first non-bondable connection.

Transient BLE failures do not panic or reboot the board. A scan, connection,
pairing, synchronization-timeout, GATT, or subscription failure is logged; the
application then waits one second and resumes scanning. If the controller exits
pairing mode, hold its Pair button again while the board is rescanning.

The connection sequence includes explicit 100 ms settling periods after scan
cancellation and link establishment. These are required for reliable Model 1708
operation; debug logging originally exposed the timing dependency by adding the
same delay accidentally. Pairing also has a 20-second application deadline so a
silent peer is disconnected and retried instead of waiting until the controller
powers off.

## Selecting a controller

The example defaults to:

```rust
const CONTROLLER_SELECTOR: ControllerSelector = ControllerSelector::AnyXbox;
```

This selects the first compatible advertisement. For a simple setup, put only
the controller you want into pairing mode.

To restrict initial discovery to one observed BLE address, change it to:

```rust
const CONTROLLER_SELECTOR: ControllerSelector =
    ControllerSelector::Address([0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
```

The bytes follow the displayed `AA:BB:CC:DD:EE:FF` order. Address filtering will
still require the advertisement to match the Xbox name or HID service; an
address match alone is not considered sufficient.

BLE devices can use private addresses that rotate, so a scanned address is not
guaranteed to be a permanent MAC address. The long-term reconnection mechanism
should store bonding data and the controller's resolved peer identity. The
address selector is primarily useful for choosing the right controller during
the first scan. A future scan-only utility can print compatible advertisements,
their address type, name, service UUIDs, and signal strength.

Radio crates are unstable and version-sensitive. If their APIs move, regenerate
a minimal ESP32-C3 BLE project with the current `esp-generate` and reconcile the
initialization code before testing on hardware.
