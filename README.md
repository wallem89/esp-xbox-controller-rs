# xbox-controller-rs

[![Maintenance: actively-developed](https://img.shields.io/badge/maintenance-actively--developed-brightgreen.svg)](https://github.com/rust-lang/cargo/issues/4121)
[![Crates.io](https://img.shields.io/crates/v/esp-xbox-controller.svg)](https://crates.io/crates/esp-xbox-controller)
[![Docs.rs](https://docs.rs/esp-xbox-controller/badge.svg)](https://docs.rs/esp-xbox-controller/)
[![CI Status](https://github.com/wallem89/esp-xbox-controller-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/wallem89/esp-xbox-controller-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![License: Apache-2.0](https://img.shields.io/badge/License-apache-yellow.svg)](https://opensource.org/license/apache-2.0)

This repository contains the ESP library, its transport-independent core,
embedded examples, and a desktop inspector.

## ESP library

`esp-xbox-controller` connects an ESP32-C3 to an Xbox Series controller over BLE
and turns its HID notifications into useful controller state. The reusable
parser and state types live in the separate `xbox-controller-core` crate and are
re-exported by the ESP library.

## Examples

The examples are tested on the [Seeed Studio XIAO ESP32-C3](https://www.seeedstudio.com/Seeed-XIAO-ESP32C3-p-5431.html).
This board costs around 6 euros that comes including an external antenna.

## Inspector

`xbox-controller-inspector` is a publishable Ubuntu AMD64 GUI for scanning and
connecting to a BLE controller, viewing its address and firmware revision,
diagnosing live inputs, and testing vibration. Install it with
`cargo install xbox-controller-inspector`. See the
[`tools/xbox-controller-inspector` README](tools/xbox-controller-inspector/README.md)
for BlueZ and Linux package requirements.

## Supported controller

The initial target is the standard **Xbox Wireless Controller Model 1914**, also
known as the third-revision controller introduced with Xbox Series X|S in 2020.
You can confirm the model number on the label inside the battery compartment.
Model 1914 has a Share button, USB-C, and BLE HID-over-GATT support; see the
[Xbox Wireless Controller model summary](https://en.wikipedia.org/wiki/Xbox_Wireless_Controller#Summary).

Model 1914 and model 1708 with firmware `5.23.6.0` are the current test targets.

Before testing, connect the controller to Windows over USB and install the latest
firmware offered by the [Xbox Accessories app](https://www.microsoft.com/en-us/p/xbox-accessories/9nblggh30xj3). Do not downgrade
or try to install exactly `5.13.3143.0`.

Models 1537 and 1697 do not support Bluetooth and cannot work with this BLE
transport.

```text
BLE notification bytes
        |
        v
esp_xbox_controller::parse_input_report()
        |
        v
XboxControllerState
```

## Why separate parsing from BLE?

The report format does not depend on the radio or operating system. Keeping the
parser in its own allocation-free, default-`no_std` crate makes it easy to test
on a desktop and reuse with any embedded BLE transport. It intentionally does
not depend on `esp-hal`, Embassy, BLE, or desktop crates such as `gilrs` and
`hidapi`. Those desktop crates sit above an OS HID layer, while an ESP32 receives
raw BLE HID notifications.

## Usage

The examples use `esp-hal 1.2.1` and `esp-println 0.18.0`. Until a compatible
`esp-radio` release is published, they pin the radio and PHY to ESP Git revision
`dbd951f78c1276461ea3050c2ba71f552a3508ef`. The workspace's ESP Git source patches
keep their other ESP dependencies on crates.io, including HAL 1.2.1. Copy these
patches as well as the example dependencies when using a separate workspace.
TrouBLE is pinned to `290ab2f45392998fa80a098f853ec86ebaa7c99c` to match the
radio's `bt-hci 0.9` interface.

On ESP32-C3, enable the `esp32c3` feature and pass the BLE controller plus the
hardware random-number generator to `run`. The callback receives every parsed
notification and can forward it to an Embassy channel or watch:

```rust,ignore
use embassy_time::Duration;
use esp_xbox_controller::{ControllerConfig, ControllerSelector, run};

let config = ControllerConfig::new(
    ControllerSelector::AnyXbox,
    None,
    Duration::from_millis(500),
);
run(ble, &mut random, config, |notification| {
    if let Ok(state) = notification {
        // Print state, or send it through an Embassy channel/watch.
    }
})
.await
```

Select one observed controller with
`ControllerSelector::Address([0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff])`.
Set `ControllerConfig::idle_disconnect_after` to disconnect after a period with
no changed controller state. Passing `None` disables this idle disconnect.

Set the BLE link-loss timeout with the third argument to `ControllerConfig::new`:

```rust,ignore
use embassy_time::Duration;
use esp_xbox_controller::{ControllerConfig, ControllerSelector};

const CONFIG: ControllerConfig = ControllerConfig::new(
    ControllerSelector::AnyXbox,
    Some(Duration::from_secs(5 * 60)),
    Duration::from_millis(500),
);
```

The supervision timeout must be 100–32000 ms in multiples
of 10 ms; `run` and `run_with_events` panic on invalid values. The configured
value applies during connection setup, pairing, and subsequent parameter updates.
Updates whose interval and latency are incompatible with this timeout are rejected.
This detects missing BLE packets, independently of controller input activity;
application scheduling and message delivery add to the final disconnect response time.

For session-end notifications and held-input activity, use `run_with_events`:

```rust,ignore
use embassy_time::Duration;
use esp_xbox_controller::{ControllerConfig, ControllerEvent, ControllerSelector, run_with_events};

let config = ControllerConfig::new(
    ControllerSelector::AnyXbox,
    Some(Duration::from_secs(5 * 60)),
    Duration::from_millis(500),
);
run_with_events(ble, &mut random, config, |state| state.buttons.rb, |event| {
    match event {
        ControllerEvent::Report(Ok(state)) => { /* Apply the fresh input state. */ }
        ControllerEvent::Report(Err(error)) => { /* Handle the malformed report. */ }
        ControllerEvent::Disconnected => { /* Clear retained input; stop outputs. */ }
    }
}).await
```

The activity predicate suspends idle disconnection while relevant input remains
held. Releasing it starts a fresh idle period. Changed inactive reports restart
that period; unchanged inactive reports do not. `run` retains its original
callback and changed-report idle behavior. Both APIs retain keepalives and the
30-second idle reconnect cooldown.

`Disconnected` is emitted when the active BLE/GATT/report session ends, before
reconnect delays. Reconnecting does not replay old input: wait for a fresh report.
Report silence alone does not signal disconnection. BLE supervision detects a
lost link; connection setup and peer parameter updates use the configured
supervision timeout. This cannot detect stalled HID processing on a live BLE connection.

Hardware validation should cover holding RB and LB+RB through report silence,
releasing RB, five-minute idle disconnection, power loss, and fresh input after
reconnection. Applications controlling outputs must clear them on disconnection.

The report parser remains available independently and without ESP dependencies:

```rust
use esp_xbox_controller::parse_input_report;

fn consume(bytes: &[u8]) -> Result<(), esp_xbox_controller::ParseError> {
    let state = parse_input_report(bytes)?;
    // Use state.buttons, state.sticks, and state.triggers.
    Ok(())
}
```

The parser currently recognizes the 16-byte layout used by the
[`asukiaaa/arduino-XboxControllerNotificationParser`](https://github.com/asukiaaa/arduino-XboxControllerNotificationParser)
project. Stick values are translated from unsigned `0..=65535` to signed
`-32768..=32767`; trigger values remain unsigned and are normally `0..=1023`.

## Workspace

- Repository root: publishable `esp-xbox-controller` crate. It re-exports the
  core state and parser API; the `esp32c3` feature adds the ESP BLE connection
  API.
- `crates/xbox-controller-core`: publishable, transport-independent `no_std`
  parser used by the ESP library and desktop inspector.
- `examples/esp32-c3-example`: XIAO ESP32-C3 `no_std` application using
  `esp-hal`, Embassy via `esp-rtos`, and `esp-radio` initialization. See its README for more information.
- `examples/esp32-c3-tasks-example`: a more project-like ESP32-C3 application
  that runs controller input and output printing in separate Embassy tasks,
  connected by a channel.
- `tools/xbox-controller-inspector`: publishable Ubuntu AMD64 desktop GUI using
  BlueZ to inspect controller identity, input reports, and vibration.

## Testing with a XIAO ESP32-C3 and Model 1914 or 1708

### Test procedure

1. Forget/unpair the controller from nearby consoles, PCs, and phones, or turn
   those devices off so they cannot reclaim it.
2. Flash and monitor the example from `examples/esp32-c3-example` with
   `cargo run --release`.
3. Hold the controller's Pair button until the Xbox button flashes rapidly.
4. Watch the serial log progress through `found`, `connected`, `pairing
   complete`, HID discovery, and `ready`.
5. Press buttons and move the sticks. The monitor prints both untouched
   notification bytes and the parsed state.

### Selecting which controller connects

The example is configured to accept the first compatible Xbox controller found.
This is the easiest behavior for a demo: keep other Xbox controllers out of
pairing mode while connecting. Discovery must still verify the advertised Xbox
name and/or HID service UUID; it must not connect to the first arbitrary BLE
device.

An optional `ControllerSelector::Address([u8; 6])` is provided for installations
where multiple Xbox controllers might advertise at once. It filters compatible
advertisements using an address written in normal `AA:BB:CC:DD:EE:FF` order.
The selector and complete BLE connection lifecycle are provided by the
library's `esp32` module. The example only initializes the ESP peripherals and
passes its chosen selector to `esp_xbox_controller::esp32::run`.

An advertising address is not necessarily a permanent device MAC: BLE privacy
allows private addresses to change. After the initial connection, bonding data
and the resolved peer identity should be persisted and used for reconnection.
The address filter is therefore a useful first-scan selector, but not a
replacement for bonding. A future scan-only tool can list compatible controllers
and show their address, address type, advertised name, HID services, and RSSI.

## Status and limitations

This project is experimental. Real notification dumps from Xbox Series
controllers across firmware versions are needed to verify offsets, report
characteristic selection, pairing/security behavior, and report-ID handling.
The parser accepts at least 16 bytes and parses the first 16; it currently
supports one known input layout and no output reports.

If you have trouble re-connecting to the same controller: power cycle the ESP32.

## Building

Run parser checks from the repository root:

```console
cargo test
cargo fmt --all --check
```

The embedded crate is excluded from the default member set so normal parser
development does not require the RISC-V target or ESP toolchain. Build and flash
it from `examples/esp32-c3-example` with `cargo run --release` after installing
`espflash`. The working directory matters: running there loads its local Cargo
configuration and the required ESP linker script.

The example plays a short best-effort vibration pulse after a controller is
fully connected. Controllers without a writable output Report continue working
for input and produce only a warning.

## License

Licensed under either Apache License 2.0 or MIT, at your option.

## Contributing and releases

Notable changes are recorded in [`CHANGELOG.md`](CHANGELOG.md).

If you want to contribute by testing or extending the features of this repository,
for instance to support more ESP32 boards, let me know!

This repository took inspiration from: 
[XboxSeriesXControllerESP32](https://github.com/asukiaaa/arduino-XboxSeriesXControllerESP32).
