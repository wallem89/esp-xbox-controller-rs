use crate::{ParseError, XboxButtonState, XboxControllerState, XboxSticks, XboxTriggers};

/// Length of the currently supported Xbox Series input report.
pub const INPUT_REPORT_LEN: usize = 16;

/// Parse an Xbox Series controller BLE input notification.
///
/// This initially supports the 16-byte layout documented by the
/// `asukiaaa/arduino-XboxControllerNotificationParser` project. The report ID
/// belongs to the GATT Report characteristic/descriptor and is therefore not
/// present in these notification bytes.
pub fn parse_input_report(bytes: &[u8]) -> Result<XboxControllerState, ParseError> {
    if bytes.len() < INPUT_REPORT_LEN {
        return Err(ParseError::TooShort {
            expected: INPUT_REPORT_LEN,
            actual: bytes.len(),
        });
    }

    let main = bytes[13];
    let center = bytes[14];
    let dpad = bytes[12];

    Ok(XboxControllerState {
        buttons: XboxButtonState {
            a: main & 0x01 != 0,
            b: main & 0x02 != 0,
            x: main & 0x08 != 0,
            y: main & 0x10 != 0,
            lb: main & 0x40 != 0,
            rb: main & 0x80 != 0,
            view: center & 0x04 != 0,
            menu: center & 0x08 != 0,
            xbox: center & 0x10 != 0,
            left_stick_button: center & 0x20 != 0,
            right_stick_button: center & 0x40 != 0,
            share: bytes[15] & 0x01 != 0,
            dpad_up: matches!(dpad, 1 | 2 | 8),
            dpad_right: matches!(dpad, 2..=4),
            dpad_down: matches!(dpad, 4..=6),
            dpad_left: matches!(dpad, 6..=8),
        },
        sticks: XboxSticks {
            left_x: centered_axis(bytes, 0),
            left_y: centered_axis(bytes, 2),
            right_x: centered_axis(bytes, 4),
            right_y: centered_axis(bytes, 6),
        },
        triggers: XboxTriggers {
            left: u16_at(bytes, 8),
            right: u16_at(bytes, 10),
        },
    })
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn centered_axis(bytes: &[u8], offset: usize) -> i16 {
    (i32::from(u16_at(bytes, offset)) - 0x8000) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    // These samples encode the currently known 16-byte layout. Offsets may
    // need adjustment after comparison with real controller firmware dumps.
    fn neutral_report() -> [u8; INPUT_REPORT_LEN] {
        [
            0x00, 0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0x80, 0, 0, 0, 0, 0, 0, 0, 0,
        ]
    }

    #[test]
    fn rejects_too_short_input() {
        assert_eq!(
            parse_input_report(&[0; 15]),
            Err(ParseError::TooShort {
                expected: 16,
                actual: 15
            })
        );
    }

    #[test]
    fn parses_neutral_state() {
        assert_eq!(
            parse_input_report(&neutral_report()).unwrap(),
            XboxControllerState::default()
        );
    }

    #[test]
    fn parses_face_buttons() {
        let mut report = neutral_report();
        report[13] = 0x01 | 0x02 | 0x08 | 0x10;
        let buttons = parse_input_report(&report).unwrap().buttons;
        assert!(buttons.a && buttons.b && buttons.x && buttons.y);
    }

    #[test]
    fn parses_dpad_cardinals_and_diagonals() {
        for (hat, up, right, down, left) in [
            (0, false, false, false, false),
            (1, true, false, false, false),
            (2, true, true, false, false),
            (3, false, true, false, false),
            (5, false, false, true, false),
            (7, false, false, false, true),
            (8, true, false, false, true),
        ] {
            let mut report = neutral_report();
            report[12] = hat;
            let buttons = parse_input_report(&report).unwrap().buttons;
            assert_eq!(
                (
                    buttons.dpad_up,
                    buttons.dpad_right,
                    buttons.dpad_down,
                    buttons.dpad_left
                ),
                (up, right, down, left)
            );
        }
    }

    #[test]
    fn parses_triggers() {
        let mut report = neutral_report();
        report[8..12].copy_from_slice(&[0x23, 0x01, 0xff, 0x03]);
        assert_eq!(
            parse_input_report(&report).unwrap().triggers,
            XboxTriggers {
                left: 0x123,
                right: 0x3ff
            }
        );
    }

    #[test]
    fn parses_and_centers_sticks() {
        let mut report = neutral_report();
        report[..8].copy_from_slice(&[0, 0, 0xff, 0xff, 0x34, 0x92, 0xcc, 0x6d]);
        assert_eq!(
            parse_input_report(&report).unwrap().sticks,
            XboxSticks {
                left_x: i16::MIN,
                left_y: i16::MAX,
                right_x: 0x1234,
                right_y: -0x1234
            }
        );
    }

    #[test]
    fn accepts_transport_metadata_after_report() {
        let mut report = [0u8; 17];
        report[0..16].copy_from_slice(&neutral_report());
        assert!(parse_input_report(&report).is_ok());
    }
}
