# ESP32-C3 tasks example

This example connects to Xbox controller `C0:D6:D5:EA:BE:85` and separates
controller input from application work using Embassy tasks:

- `controller_task` owns the BLE connection and publishes changed controller
  states to a bounded Embassy channel.
- `print_task` waits on that channel and prints each state.

This structure leaves the executor free to run additional application tasks.
The controller callback uses non-blocking channel sends because the library's
notification callback is synchronous. If the consumer cannot keep up, a full
channel causes that update to be dropped and a warning to be printed.

From this directory, flash and monitor the example with:

```console
cargo run --release
```

Put the selected controller into pairing mode before connecting. The address is
written in the same order as the usual `C0:D6:D5:EA:BE:85` notation.
