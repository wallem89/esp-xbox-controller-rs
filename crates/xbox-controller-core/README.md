# xbox-controller-core

An allocation-free, `no_std` parser for Xbox Wireless Controller BLE input
reports. It contains no ESP, Embassy, operating-system, or Bluetooth-stack
dependencies.

```rust
use xbox_controller_core::parse_input_report;

fn consume(bytes: &[u8]) -> Result<(), xbox_controller_core::ParseError> {
    let state = parse_input_report(bytes)?;
    // Use state.buttons, state.sticks, and state.triggers.
    Ok(())
}
```

Use the separate `esp-xbox-controller` crate when you also want ESP32 BLE
scanning, pairing, HID discovery, keepalives, and reconnection management.
