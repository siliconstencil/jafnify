use jafnify_core::budget::{
    clamp_reset_day, days_in_month, display_for, format_gb, month_key, warning_for, BudgetRecord,
};

const GB: u64 = 1_000_000_000;

fn record(allow: u64, used: u64, reset: u8, month: u32, warned: u32) -> BudgetRecord {
    BudgetRecord {
        allow_bytes: allow,
        used_bytes: used,
        reset_day: reset,
        month_key: month,
        warned_month: warned,
    }
}

#[test]
fn the_month_key_matches_the_m0_field() {
    assert_eq!(month_key(2026, 11), 24323);
    assert_eq!(month_key(2026, 1), 24313);
}

#[test]
fn the_reset_day_is_clamped_to_the_month_end() {
    assert_eq!(days_in_month(2026, 2), 28);
    assert_eq!(days_in_month(2024, 2), 29);
    assert_eq!(days_in_month(2026, 4), 30);
    assert_eq!(days_in_month(2026, 1), 31);
    assert_eq!(clamp_reset_day(31, 2026, 2), 28);
    assert_eq!(clamp_reset_day(31, 2024, 2), 29);
    assert_eq!(clamp_reset_day(0, 2026, 11), 1);
    assert_eq!(clamp_reset_day(12, 2026, 11), 12);
}

#[test]
fn a_new_month_resets_the_used_and_warned_values() {
    let mut budget = record(
        10 * GB,
        5 * GB,
        12,
        month_key(2026, 10),
        month_key(2026, 10),
    );
    assert!(budget.prepare(2026, 11, 12));
    assert_eq!(budget.used_bytes, 0);
    assert_eq!(budget.warned_month, 0);
    assert_eq!(budget.month_key, month_key(2026, 11));
    assert_eq!(budget.reset_day, 12);

    budget.used_bytes = 3 * GB;
    assert!(!budget.prepare(2026, 11, 12));
    assert_eq!(budget.used_bytes, 3 * GB);
}

#[test]
fn the_warning_is_at_eighty_percent() {
    let below = record(10 * GB, 8 * GB - 1, 1, 24323, 0);
    assert_eq!(warning_for(below.used_bytes, below.allow_bytes), None);
    let at = record(10 * GB, 8 * GB, 1, 24323, 0);
    assert_eq!(
        warning_for(at.used_bytes, at.allow_bytes),
        Some("80% of your mobile data allowance used.")
    );
    assert!(at.warning_due());
    let no_allowance = record(0, 9 * GB, 1, 24323, 0);
    assert!(!no_allowance.warning_due());
}

#[test]
fn the_warning_is_once_per_month() {
    let warned = record(10 * GB, 9 * GB, 1, 24323, 24323);
    assert!(!warned.warning_due());
    let next = record(10 * GB, 9 * GB, 1, 24324, 24323);
    assert!(next.warning_due());
}

#[test]
fn the_display_line_uses_decimal_gigabytes() {
    assert_eq!(
        display_for(3_200_000_000, 10 * GB),
        "Mobile data: 3.2 GB of 10 GB this month."
    );
    assert_eq!(display_for(0, 0), "Set your monthly allowance.");
}

#[test]
fn gigabytes_are_whole_or_one_decimal() {
    assert_eq!(format_gb(10 * GB), "10 GB");
    assert_eq!(format_gb(3_200_000_000), "3.2 GB");
    assert_eq!(format_gb(0), "0 GB");
    assert_eq!(format_gb(1_050_000_000), "1 GB");
    assert_eq!(format_gb(999_999_999), "0.9 GB");
}

#[test]
fn the_budget_line_round_trips() {
    let budget = record(10 * GB, 3 * GB, 12, 24323, 24320);
    let line = budget.line();
    assert_eq!(
        line,
        "JFB1 1 allow=10000000000 used=3000000000 reset=12 month=24323 warned=24320"
    );
    assert_eq!(BudgetRecord::parse_line(&line).unwrap(), budget);
}

#[test]
fn an_old_line_without_warned_loads_zero() {
    let parsed = BudgetRecord::parse_line("JFB1 1 allow=100 used=50 reset=1 month=24323").unwrap();
    assert_eq!(parsed.warned_month, 0);
    assert_eq!(parsed.allow_bytes, 100);
    assert_eq!(parsed.used_bytes, 50);
}

#[test]
fn unknown_keys_are_ignored_and_bad_values_are_rejected() {
    let parsed =
        BudgetRecord::parse_line("JFB1 1 allow=100 used=50 reset=1 month=24323 extra=x").unwrap();
    assert_eq!(parsed.allow_bytes, 100);
    assert!(BudgetRecord::parse_line("JFB2 1 allow=1").is_err());
    assert!(BudgetRecord::parse_line("JFB1 2 allow=1").is_err());
    assert!(BudgetRecord::parse_line("JFB1 1 allow=-1 used=0 reset=1 month=1").is_err());
    assert!(BudgetRecord::parse_line("JFB1 1 allow=1 allow=2").is_err());
    assert!(BudgetRecord::parse_line("JFB1 1 allow=1 used=0 reset=99 month=1").is_err());
}

#[test]
fn the_budget_arithmetic_does_not_overflow() {
    let budget = record(u64::MAX, u64::MAX, 31, 24323, 0);
    assert!(budget.warning_due());
    let _ = budget.display_line();
    assert_eq!(format_gb(u64::MAX), "18446744073.7 GB");
}
