//! Time-based conditional expressions for task scheduling.
//!
//! Supports expressions like:
//! - "weekday" — Monday through Friday
//! - "weekend" — Saturday and Sunday
//! - "after 9am" / "after 09:00" — after a specific time
//! - "before 5pm" / "before 17:00" — before a specific time
//! - "between 9am and 5pm" — within a time range
//! - Compound expressions with "&&" (AND) and "||" (OR)
//!
//! Examples:
//! - "weekday && after 9am && before 5pm"
//! - "weekend || after 6pm"
//! - "weekday && between 9am and 5pm"

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};

use common::errors::TaskError;

/// A parsed and validated time condition expression.
#[derive(Debug, Clone)]
pub struct TimeCondition {
    expression: String,
    root: ConditionNode,
}

/// Internal representation of a condition tree.
#[derive(Debug, Clone)]
enum ConditionNode {
    /// A single atomic condition.
    Atom(AtomicCondition),
    /// Logical AND of two conditions.
    And(Box<ConditionNode>, Box<ConditionNode>),
    /// Logical OR of two conditions.
    Or(Box<ConditionNode>, Box<ConditionNode>),
}

/// An atomic (non-compound) time condition.
#[derive(Debug, Clone)]
enum AtomicCondition {
    /// Monday through Friday.
    Weekday,
    /// Saturday and Sunday.
    Weekend,
    /// After a specific hour:minute.
    After { hour: u32, minute: u32 },
    /// Before a specific hour:minute.
    Before { hour: u32, minute: u32 },
    /// Between two times (inclusive of start, exclusive of end).
    Between {
        start_hour: u32,
        start_minute: u32,
        end_hour: u32,
        end_minute: u32,
    },
    /// A specific day of the week.
    DayOfWeek(Weekday),
}

impl TimeCondition {
    /// Parse a time condition expression.
    ///
    /// Supports:
    /// - `weekday`, `weekend`
    /// - `after HH:MM`, `after Ham`, `after Hpm`
    /// - `before HH:MM`, `before Ham`, `before Hpm`
    /// - `between Ham and Hpm`, `between HH:MM and HH:MM`
    /// - `monday`, `tuesday`, ..., `sunday`
    /// - Compound: `expr && expr`, `expr || expr`
    pub fn parse(expression: &str) -> Result<Self, TaskError> {
        let trimmed = expression.trim();
        if trimmed.is_empty() {
            return Err(TaskError::InvalidCronExpression {
                expression: "empty time condition expression".to_string(),
            });
        }

        let root = Self::parse_or(trimmed)?;
        Ok(Self {
            expression: expression.to_string(),
            root,
        })
    }

    /// Evaluate the condition at the given time.
    pub fn evaluate(&self, at: DateTime<Utc>) -> bool {
        Self::eval_node(&self.root, at)
    }

    /// Get the raw expression string.
    pub fn expression(&self) -> &str {
        &self.expression
    }

    // =========================================================================
    // Parsing
    // =========================================================================

    /// Parse OR-level expressions (lowest precedence).
    fn parse_or(input: &str) -> Result<ConditionNode, TaskError> {
        // Split on "||" but be careful not to split inside tokens
        if let Some(pos) = input.find("||") {
            let left = input[..pos].trim();
            let right = input[pos + 2..].trim();
            let left_node = Self::parse_and(left)?;
            let right_node = Self::parse_or(right)?;
            Ok(ConditionNode::Or(Box::new(left_node), Box::new(right_node)))
        } else {
            Self::parse_and(input)
        }
    }

    /// Parse AND-level expressions.
    fn parse_and(input: &str) -> Result<ConditionNode, TaskError> {
        if let Some(pos) = input.find("&&") {
            let left = input[..pos].trim();
            let right = input[pos + 2..].trim();
            let left_node = Self::parse_atom(left)?;
            let right_node = Self::parse_and(right)?;
            Ok(ConditionNode::And(
                Box::new(left_node),
                Box::new(right_node),
            ))
        } else {
            Self::parse_atom(input)
        }
    }

    /// Parse an atomic condition.
    fn parse_atom(input: &str) -> Result<ConditionNode, TaskError> {
        let input = input.trim().to_lowercase();

        match input.as_str() {
            "weekday" => Ok(ConditionNode::Atom(AtomicCondition::Weekday)),
            "weekend" => Ok(ConditionNode::Atom(AtomicCondition::Weekend)),
            "monday" | "mon" => Ok(ConditionNode::Atom(AtomicCondition::DayOfWeek(
                Weekday::Mon,
            ))),
            "tuesday" | "tue" => Ok(ConditionNode::Atom(AtomicCondition::DayOfWeek(
                Weekday::Tue,
            ))),
            "wednesday" | "wed" => Ok(ConditionNode::Atom(AtomicCondition::DayOfWeek(
                Weekday::Wed,
            ))),
            "thursday" | "thu" => Ok(ConditionNode::Atom(AtomicCondition::DayOfWeek(
                Weekday::Thu,
            ))),
            "friday" | "fri" => Ok(ConditionNode::Atom(AtomicCondition::DayOfWeek(
                Weekday::Fri,
            ))),
            "saturday" | "sat" => Ok(ConditionNode::Atom(AtomicCondition::DayOfWeek(
                Weekday::Sat,
            ))),
            "sunday" | "sun" => Ok(ConditionNode::Atom(AtomicCondition::DayOfWeek(
                Weekday::Sun,
            ))),
            _ => {
                // Try "after X"
                if let Some(time_str) = input.strip_prefix("after ") {
                    let (hour, minute) = Self::parse_time(time_str.trim())?;
                    return Ok(ConditionNode::Atom(AtomicCondition::After { hour, minute }));
                }
                // Try "before X"
                if let Some(time_str) = input.strip_prefix("before ") {
                    let (hour, minute) = Self::parse_time(time_str.trim())?;
                    return Ok(ConditionNode::Atom(AtomicCondition::Before {
                        hour,
                        minute,
                    }));
                }
                // Try "between X and Y"
                if let Some(rest) = input.strip_prefix("between ") {
                    if let Some(and_pos) = rest.find(" and ") {
                        let start_str = rest[..and_pos].trim();
                        let end_str = rest[and_pos + 5..].trim();
                        let (start_hour, start_minute) = Self::parse_time(start_str)?;
                        let (end_hour, end_minute) = Self::parse_time(end_str)?;
                        return Ok(ConditionNode::Atom(AtomicCondition::Between {
                            start_hour,
                            start_minute,
                            end_hour,
                            end_minute,
                        }));
                    }
                }

                Err(TaskError::InvalidCronExpression {
                    expression: format!("unrecognized time condition: '{}'", input),
                })
            }
        }
    }

    /// Parse a time string like "9am", "5pm", "09:00", "17:30".
    fn parse_time(input: &str) -> Result<(u32, u32), TaskError> {
        let input = input.trim().to_lowercase();

        // Try HH:MM format
        if input.contains(':') {
            let parts: Vec<&str> = input.split(':').collect();
            if parts.len() == 2 {
                let hour: u32 = parts[0]
                    .parse()
                    .map_err(|_| TaskError::InvalidCronExpression {
                        expression: format!("invalid hour in time: '{}'", input),
                    })?;
                let minute: u32 =
                    parts[1]
                        .parse()
                        .map_err(|_| TaskError::InvalidCronExpression {
                            expression: format!("invalid minute in time: '{}'", input),
                        })?;
                if hour > 23 || minute > 59 {
                    return Err(TaskError::InvalidCronExpression {
                        expression: format!("time out of range: '{}'", input),
                    });
                }
                return Ok((hour, minute));
            }
        }

        // Try 12-hour format: "9am", "5pm", "12pm", "12am"
        if input.ends_with("am") || input.ends_with("pm") {
            let is_pm = input.ends_with("pm");
            let num_str = &input[..input.len() - 2];
            let hour: u32 = num_str
                .parse()
                .map_err(|_| TaskError::InvalidCronExpression {
                    expression: format!("invalid hour in time: '{}'", input),
                })?;

            if hour == 0 || hour > 12 {
                return Err(TaskError::InvalidCronExpression {
                    expression: format!("invalid 12-hour time: '{}'", input),
                });
            }

            let hour_24 = if is_pm {
                if hour == 12 {
                    12
                } else {
                    hour + 12
                }
            } else {
                if hour == 12 {
                    0
                } else {
                    hour
                }
            };

            return Ok((hour_24, 0));
        }

        Err(TaskError::InvalidCronExpression {
            expression: format!("cannot parse time: '{}'", input),
        })
    }

    // =========================================================================
    // Evaluation
    // =========================================================================

    fn eval_node(node: &ConditionNode, at: DateTime<Utc>) -> bool {
        match node {
            ConditionNode::Atom(atom) => Self::eval_atom(atom, at),
            ConditionNode::And(left, right) => {
                Self::eval_node(left, at) && Self::eval_node(right, at)
            }
            ConditionNode::Or(left, right) => {
                Self::eval_node(left, at) || Self::eval_node(right, at)
            }
        }
    }

    fn eval_atom(atom: &AtomicCondition, at: DateTime<Utc>) -> bool {
        match atom {
            AtomicCondition::Weekday => {
                let day = at.weekday();
                !matches!(day, Weekday::Sat | Weekday::Sun)
            }
            AtomicCondition::Weekend => {
                let day = at.weekday();
                matches!(day, Weekday::Sat | Weekday::Sun)
            }
            AtomicCondition::After { hour, minute } => {
                let current_minutes = at.hour() * 60 + at.minute();
                let target_minutes = hour * 60 + minute;
                current_minutes >= target_minutes
            }
            AtomicCondition::Before { hour, minute } => {
                let current_minutes = at.hour() * 60 + at.minute();
                let target_minutes = hour * 60 + minute;
                current_minutes < target_minutes
            }
            AtomicCondition::Between {
                start_hour,
                start_minute,
                end_hour,
                end_minute,
            } => {
                let current_minutes = at.hour() * 60 + at.minute();
                let start_minutes = start_hour * 60 + start_minute;
                let end_minutes = end_hour * 60 + end_minute;
                current_minutes >= start_minutes && current_minutes < end_minutes
            }
            AtomicCondition::DayOfWeek(day) => at.weekday() == *day,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_time(year: i32, month: u32, day: u32, hour: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, min, 0)
            .unwrap()
    }

    #[test]
    fn test_parse_weekday() {
        let cond = TimeCondition::parse("weekday").unwrap();
        // 2024-01-15 is a Monday
        assert!(cond.evaluate(make_time(2024, 1, 15, 10, 0)));
        // 2024-01-13 is a Saturday
        assert!(!cond.evaluate(make_time(2024, 1, 13, 10, 0)));
    }

    #[test]
    fn test_parse_weekend() {
        let cond = TimeCondition::parse("weekend").unwrap();
        // 2024-01-13 is a Saturday
        assert!(cond.evaluate(make_time(2024, 1, 13, 10, 0)));
        // 2024-01-15 is a Monday
        assert!(!cond.evaluate(make_time(2024, 1, 15, 10, 0)));
    }

    #[test]
    fn test_parse_after_am_pm() {
        let cond = TimeCondition::parse("after 9am").unwrap();
        assert!(cond.evaluate(make_time(2024, 1, 15, 10, 0)));
        assert!(!cond.evaluate(make_time(2024, 1, 15, 8, 0)));
        // Exactly 9am should match (>=)
        assert!(cond.evaluate(make_time(2024, 1, 15, 9, 0)));
    }

    #[test]
    fn test_parse_before_pm() {
        let cond = TimeCondition::parse("before 5pm").unwrap();
        assert!(cond.evaluate(make_time(2024, 1, 15, 10, 0)));
        assert!(!cond.evaluate(make_time(2024, 1, 15, 17, 0)));
        assert!(!cond.evaluate(make_time(2024, 1, 15, 18, 0)));
    }

    #[test]
    fn test_parse_after_24h() {
        let cond = TimeCondition::parse("after 09:00").unwrap();
        assert!(cond.evaluate(make_time(2024, 1, 15, 10, 0)));
        assert!(!cond.evaluate(make_time(2024, 1, 15, 8, 59)));
    }

    #[test]
    fn test_parse_between() {
        let cond = TimeCondition::parse("between 9am and 5pm").unwrap();
        assert!(cond.evaluate(make_time(2024, 1, 15, 12, 0)));
        assert!(cond.evaluate(make_time(2024, 1, 15, 9, 0)));
        assert!(!cond.evaluate(make_time(2024, 1, 15, 17, 0))); // end is exclusive
        assert!(!cond.evaluate(make_time(2024, 1, 15, 8, 0)));
    }

    #[test]
    fn test_parse_between_24h() {
        let cond = TimeCondition::parse("between 09:00 and 17:30").unwrap();
        assert!(cond.evaluate(make_time(2024, 1, 15, 12, 0)));
        assert!(cond.evaluate(make_time(2024, 1, 15, 17, 29)));
        assert!(!cond.evaluate(make_time(2024, 1, 15, 17, 30)));
    }

    #[test]
    fn test_compound_and() {
        let cond = TimeCondition::parse("weekday && after 9am").unwrap();
        // Monday at 10am
        assert!(cond.evaluate(make_time(2024, 1, 15, 10, 0)));
        // Monday at 8am
        assert!(!cond.evaluate(make_time(2024, 1, 15, 8, 0)));
        // Saturday at 10am
        assert!(!cond.evaluate(make_time(2024, 1, 13, 10, 0)));
    }

    #[test]
    fn test_compound_or() {
        let cond = TimeCondition::parse("weekend || after 6pm").unwrap();
        // Saturday at 10am
        assert!(cond.evaluate(make_time(2024, 1, 13, 10, 0)));
        // Monday at 7pm
        assert!(cond.evaluate(make_time(2024, 1, 15, 19, 0)));
        // Monday at 10am
        assert!(!cond.evaluate(make_time(2024, 1, 15, 10, 0)));
    }

    #[test]
    fn test_complex_compound() {
        let cond = TimeCondition::parse("weekday && after 9am && before 5pm").unwrap();
        // Monday at noon
        assert!(cond.evaluate(make_time(2024, 1, 15, 12, 0)));
        // Monday at 6pm
        assert!(!cond.evaluate(make_time(2024, 1, 15, 18, 0)));
        // Saturday at noon
        assert!(!cond.evaluate(make_time(2024, 1, 13, 12, 0)));
    }

    #[test]
    fn test_day_of_week() {
        let cond = TimeCondition::parse("monday").unwrap();
        assert!(cond.evaluate(make_time(2024, 1, 15, 10, 0))); // Monday
        assert!(!cond.evaluate(make_time(2024, 1, 16, 10, 0))); // Tuesday
    }

    #[test]
    fn test_invalid_expression() {
        assert!(TimeCondition::parse("").is_err());
        assert!(TimeCondition::parse("invalid stuff").is_err());
        assert!(TimeCondition::parse("after 25:00").is_err());
        assert!(TimeCondition::parse("after 13am").is_err());
    }

    #[test]
    fn test_12pm_and_12am() {
        let after_noon = TimeCondition::parse("after 12pm").unwrap();
        assert!(after_noon.evaluate(make_time(2024, 1, 15, 12, 0)));
        assert!(after_noon.evaluate(make_time(2024, 1, 15, 13, 0)));
        assert!(!after_noon.evaluate(make_time(2024, 1, 15, 11, 0)));

        let after_midnight = TimeCondition::parse("after 12am").unwrap();
        // 12am = 0:00, so everything is after midnight
        assert!(after_midnight.evaluate(make_time(2024, 1, 15, 0, 0)));
        assert!(after_midnight.evaluate(make_time(2024, 1, 15, 1, 0)));
    }
}
