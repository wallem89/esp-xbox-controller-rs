# Hardware-validation checklist

- [ ] Record the Model 1914 firmware version reported by Xbox Accessories.
- [x] Select and integrate a `no_std` BLE host with central/GATT client support over `esp-radio`.
- [x] Scan by advertised name and HID service UUID (`0x1812`).
- [x] Apply `ControllerSelector` and log the selected candidate's address type and RSSI.
- [ ] Add a scan-only address-discovery utility or example.
- [ ] Hardware-test the implemented pairing and encryption; then add persistent bonding, reconnects, and key storage.
- [ ] Hardware-verify that the first Report characteristic `0x2A4D` is the input report; use Report Reference descriptor `0x2908` if needed.
- [x] Read the input Report before subscribing, then log untouched notifications.
- [ ] Confirm whether notifications ever include a report ID byte.
- [ ] Capture neutral, each button, D-pad, trigger, and stick reports from Model 1914.
- [ ] Reconcile parser offsets and ranges against those captures.
- [ ] Add real notification captures as regression fixtures with their firmware version.
- [ ] Repeat captures after a controller firmware update to identify protocol drift.
- [ ] Test reconnect and power-cycle behavior on a Seeed Studio XIAO ESP32-C3.
- [x] Recover from transient scan, connection, pairing, GATT, and disconnect failures without panicking.
- [ ] Add output reports (rumble) only after input parsing and transport are stable.
- [ ] Publish `xbox-controller-core` after CI passes and the repository is public.
