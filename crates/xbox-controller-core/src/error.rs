/// An error encountered while parsing an Xbox input notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The notification does not contain the complete 16-byte input report.
    TooShort { expected: usize, actual: usize },
}
