// Pure tick-based idle policy, independent of the BLE runtime.
pub(crate) struct IdleTimeout {
    timeout: Option<u64>,
    deadline: Option<u64>,
}

impl IdleTimeout {
    pub(crate) fn new(now: u64, timeout: Option<u64>) -> Self {
        Self {
            timeout,
            deadline: timeout.map(|duration| now + duration),
        }
    }

    pub(crate) fn report(&mut self, now: u64, changed: bool, active: bool) {
        if active {
            self.deadline = None;
        } else if changed || self.deadline.is_none() {
            self.deadline = self.timeout.map(|duration| now + duration);
        }
    }

    pub(crate) fn deadline(&self) -> Option<u64> {
        self.deadline
    }

    pub(crate) fn expired(&self, now: u64) -> bool {
        self.deadline.is_some_and(|deadline| now >= deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::IdleTimeout;

    #[test]
    fn held_input_survives_silence_and_release_starts_idle_period() {
        let mut idle = IdleTimeout::new(0, Some(300));
        idle.report(10, true, true);
        assert_eq!(idle.deadline(), None);
        assert!(!idle.expired(1000));
        idle.report(1000, true, false);
        assert!(!idle.expired(1299));
        assert!(idle.expired(1300));
    }

    #[test]
    fn unchanged_neutral_reports_do_not_keep_controller_awake() {
        let mut idle = IdleTimeout::new(0, Some(300));
        idle.report(10, true, false);
        idle.report(200, false, false);
        assert!(!idle.expired(309));
        assert!(idle.expired(310));
    }

    #[test]
    fn changed_inactive_input_restarts_idle_period() {
        let mut idle = IdleTimeout::new(0, Some(300));
        idle.report(200, true, false);
        assert!(!idle.expired(300));
        assert!(idle.expired(500));
    }

    #[test]
    fn no_input_expires_and_disabled_idle_does_not() {
        assert!(IdleTimeout::new(0, Some(300)).expired(300));
        let mut idle = IdleTimeout::new(0, None);
        idle.report(10, true, true);
        idle.report(20, true, false);
        assert!(!idle.expired(1000));
    }
}
