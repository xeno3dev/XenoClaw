//! Cron expression parsing and evaluation with minimum 1-minute interval validation.

use chrono::{DateTime, Utc};
use cron::Schedule;
use std::str::FromStr;

use common::errors::TaskError;

/// A validated cron schedule that enforces a minimum 1-minute interval.
#[derive(Debug, Clone)]
pub struct CronSchedule {
    expression: String,
    schedule: Schedule,
}

impl CronSchedule {
    /// Parse and validate a cron expression.
    ///
    /// The expression must be a valid 7-field cron expression (seconds, minutes, hours,
    /// day-of-month, month, day-of-week, year) or a 6-field expression (without year).
    ///
    /// Returns an error if:
    /// - The expression is not valid cron syntax
    /// - The effective interval between executions is less than 1 minute
    pub fn parse(expression: &str) -> Result<Self, TaskError> {
        let schedule =
            Schedule::from_str(expression).map_err(|e| TaskError::InvalidCronExpression {
                expression: format!("{}: {}", expression, e),
            })?;

        let cron_schedule = Self {
            expression: expression.to_string(),
            schedule,
        };

        // Validate minimum 1-minute interval
        cron_schedule.validate_minimum_interval()?;

        Ok(cron_schedule)
    }

    /// Get the next occurrence after the given time.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.schedule.after(&after).next()
    }

    /// Get the next N occurrences after the given time.
    pub fn next_n_after(&self, after: DateTime<Utc>, n: usize) -> Vec<DateTime<Utc>> {
        self.schedule.after(&after).take(n).collect()
    }

    /// Check if a given time matches this cron schedule (within a tolerance window).
    pub fn matches_at(&self, time: DateTime<Utc>, tolerance_secs: i64) -> bool {
        // Check if there's a scheduled time within the tolerance window before the given time
        let window_start = time - chrono::Duration::seconds(tolerance_secs);
        if let Some(next) = self.schedule.after(&window_start).next() {
            next <= time
        } else {
            false
        }
    }

    /// Get the raw cron expression string.
    pub fn expression(&self) -> &str {
        &self.expression
    }

    /// Validate that the cron schedule doesn't fire more frequently than once per minute.
    fn validate_minimum_interval(&self) -> Result<(), TaskError> {
        let now = Utc::now();
        let upcoming: Vec<DateTime<Utc>> = self.schedule.after(&now).take(5).collect();

        if upcoming.len() < 2 {
            // Can't determine interval with fewer than 2 occurrences, allow it
            return Ok(());
        }

        for window in upcoming.windows(2) {
            let interval = window[1] - window[0];
            if interval.num_seconds() < 60 {
                return Err(TaskError::InvalidCronExpression {
                    expression: format!(
                        "{}: interval {}s is less than minimum 60s",
                        self.expression,
                        interval.num_seconds()
                    ),
                });
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_every_minute() {
        // Every minute (minimum allowed)
        let result = CronSchedule::parse("0 * * * * *");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_valid_every_hour() {
        let result = CronSchedule::parse("0 0 * * * *");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_valid_daily() {
        let result = CronSchedule::parse("0 0 9 * * *");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_invalid_sub_minute() {
        // Every second — should be rejected (less than 1-minute interval)
        let result = CronSchedule::parse("* * * * * *");
        assert!(result.is_err());
        if let Err(TaskError::InvalidCronExpression { expression }) = result {
            assert!(expression.contains("less than minimum 60s"));
        }
    }

    #[test]
    fn test_parse_invalid_every_30_seconds() {
        // Every 30 seconds — should be rejected
        let result = CronSchedule::parse("0,30 * * * * *");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invalid_syntax() {
        let result = CronSchedule::parse("not a cron expression");
        assert!(result.is_err());
    }

    #[test]
    fn test_next_after() {
        let schedule = CronSchedule::parse("0 * * * * *").unwrap();
        let now = Utc::now();
        let next = schedule.next_after(now);
        assert!(next.is_some());
        let next = next.unwrap();
        assert!(next > now);
    }

    #[test]
    fn test_next_n_after() {
        let schedule = CronSchedule::parse("0 * * * * *").unwrap();
        let now = Utc::now();
        let next_5 = schedule.next_n_after(now, 5);
        assert_eq!(next_5.len(), 5);
        // All should be in ascending order
        for window in next_5.windows(2) {
            assert!(window[1] > window[0]);
        }
    }

    #[test]
    fn test_matches_at_with_tolerance() {
        let schedule = CronSchedule::parse("0 * * * * *").unwrap();
        let now = Utc::now();
        if let Some(next) = schedule.next_after(now) {
            // At the exact scheduled time, should match with 60s tolerance
            assert!(schedule.matches_at(next, 60));
            // 30 seconds after the scheduled time should still match with 60s tolerance
            let after = next + chrono::Duration::seconds(30);
            assert!(schedule.matches_at(after, 60));
        }
    }

    #[test]
    fn test_expression_getter() {
        let schedule = CronSchedule::parse("0 0 9 * * *").unwrap();
        assert_eq!(schedule.expression(), "0 0 9 * * *");
    }
}
