/// Digital button state from one Xbox controller report.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct XboxButtonState {
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub lb: bool,
    pub rb: bool,
    pub view: bool,
    pub menu: bool,
    pub xbox: bool,
    pub left_stick_button: bool,
    pub right_stick_button: bool,
    pub share: bool,
}

/// Stick positions centered around zero.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct XboxSticks {
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
}

/// Analog trigger values, normally in the range `0..=1023`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct XboxTriggers {
    pub left: u16,
    pub right: u16,
}

/// The controller state represented by one BLE HID input notification.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct XboxControllerState {
    pub buttons: XboxButtonState,
    pub sticks: XboxSticks,
    pub triggers: XboxTriggers,
}
