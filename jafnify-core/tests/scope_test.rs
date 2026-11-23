//! The scope of a tile: what a tile sums over.
//!
//! A tile that sums over time can be tapped to change the window it sums
//! over. The core owns the enum, the cycle, the window and the words.

use jafnify_core::scope::{
    effective, in_scope, resumes, totals, window_start, ScopeSession, ScopeTotals, TileScope,
    RESUME_GRACE_MS,
};
use jafnify_core::tiles::{ID_DROPS, ID_SPEED, ID_TODAY};

/// 2026-01-01T00:00:00Z.
const BASE: i64 = 1_767_225_600_000;
const HOUR: i64 = 3_600_000;

/// The sharing session of the tests.
fn session() -> ScopeSession {
    ScopeSession {
        started_at_ms: BASE,
        up_bytes: 208_000_000,
        down_bytes: 695_000_000,
        peak_down_bps: 49_900_000,
    }
}

#[test]
fn the_scope_words_are_the_words_the_tiles_show() {
    assert_eq!(TileScope::Session.word(), "SESSION");
    assert_eq!(TileScope::Today.word(), "TODAY");
    assert_eq!(TileScope::Last24Hours.word(), "24 H");
}

#[test]
fn a_scope_word_is_read_leniently() {
    assert_eq!(TileScope::parse("SESSION"), Some(TileScope::Session));
    assert_eq!(TileScope::parse("session"), Some(TileScope::Session));
    assert_eq!(TileScope::parse(" Today "), Some(TileScope::Today));
    assert_eq!(TileScope::parse("24 H"), Some(TileScope::Last24Hours));
    assert_eq!(TileScope::parse("24h"), Some(TileScope::Last24Hours));
    assert_eq!(TileScope::parse("nonsense"), None);
}

#[test]
fn the_cycle_runs_session_today_and_the_last_day() {
    assert_eq!(TileScope::Session.next(ID_DROPS), TileScope::Today);
    assert_eq!(TileScope::Today.next(ID_DROPS), TileScope::Last24Hours);
    assert_eq!(TileScope::Last24Hours.next(ID_DROPS), TileScope::Session);
}

#[test]
fn the_cycle_skips_the_scopes_the_tile_does_not_have() {
    // The bytes tile has no 24 hour window: the cycle goes straight back.
    assert_eq!(TileScope::Session.next(ID_TODAY), TileScope::Today);
    assert_eq!(TileScope::Today.next(ID_TODAY), TileScope::Session);
    assert_eq!(TileScope::Session.next(ID_SPEED), TileScope::Today);
    assert_eq!(TileScope::Today.next(ID_SPEED), TileScope::Session);
    assert!(!TileScope::Last24Hours.supports(ID_TODAY));
    assert!(TileScope::Last24Hours.supports(ID_DROPS));
}

#[test]
fn a_tile_starts_with_the_scope_it_had_before() {
    assert_eq!(TileScope::default_for(ID_DROPS), TileScope::Last24Hours);
    assert_eq!(TileScope::default_for(ID_TODAY), TileScope::Today);
    assert_eq!(TileScope::default_for(ID_SPEED), TileScope::Today);
}

#[test]
fn an_unknown_word_falls_back_to_the_default() {
    assert_eq!(
        TileScope::for_tile(ID_DROPS, Some("nonsense")),
        TileScope::Last24Hours
    );
    assert_eq!(TileScope::for_tile(ID_DROPS, None), TileScope::Last24Hours);
    assert_eq!(
        TileScope::for_tile(ID_TODAY, Some("24 H")),
        TileScope::Today,
        "a scope the tile does not have is not the scope of the tile"
    );
    assert_eq!(
        TileScope::for_tile(ID_DROPS, Some("SESSION")),
        TileScope::Session
    );
}

#[test]
fn the_today_window_starts_at_the_local_midnight() {
    let now = BASE + 13 * HOUR;
    assert_eq!(window_start(TileScope::Today, now, 0, None), Some(BASE));
    // Three hours ahead of UTC: the local midnight is three hours earlier.
    assert_eq!(
        window_start(TileScope::Today, now, 180, None),
        Some(BASE - 3 * HOUR)
    );
}

#[test]
fn the_session_window_starts_at_the_sharing() {
    let now = BASE + 13 * HOUR;
    assert_eq!(
        window_start(TileScope::Session, now, 0, Some(&session())),
        Some(BASE)
    );
}

#[test]
fn a_session_without_a_sharing_has_no_window() {
    let now = BASE + 13 * HOUR;
    assert_eq!(window_start(TileScope::Session, now, 0, None), None);
}

#[test]
fn the_last_day_window_is_the_window_of_the_hour_strip() {
    let now = BASE + 13 * HOUR + 1_000;
    // The strip covers 24 hours ending at the current hour: it starts at
    // 14:00 of the day before.
    assert_eq!(
        window_start(TileScope::Last24Hours, now, 0, None),
        Some(BASE - 10 * HOUR)
    );
}

#[test]
fn a_reconnect_inside_the_sharing_stays_in_the_session() {
    let now = BASE + 13 * HOUR;
    let reconnect = BASE + 4 * HOUR;
    assert!(in_scope(
        reconnect,
        TileScope::Session,
        now,
        0,
        Some(&session())
    ));
}

#[test]
fn an_absence_inside_the_sharing_stays_in_the_session() {
    let now = BASE + 13 * HOUR;
    // The phone was away for ten minutes in the middle of the sharing.
    let away = BASE + 7 * HOUR;
    assert!(in_scope(away, TileScope::Session, now, 0, Some(&session())));
}

#[test]
fn a_moment_before_the_sharing_is_out_of_the_session() {
    let now = BASE + 13 * HOUR;
    assert!(!in_scope(
        BASE - HOUR,
        TileScope::Session,
        now,
        0,
        Some(&session())
    ));
}

#[test]
fn the_fact_words_name_the_scope() {
    assert_eq!(TileScope::Today.fact_word(), "today");
    assert_eq!(TileScope::Session.fact_word(), "in session");
    assert_eq!(TileScope::Last24Hours.fact_word(), "in 24 h");
}

#[test]
fn a_session_round_trips_as_one_line() {
    let line = session().line();
    assert_eq!(
        line,
        "up=208000000 down=695000000 peak=49900000 start=1767225600000"
    );
    assert_eq!(ScopeSession::parse(&line), Some(session()));
}

#[test]
fn an_empty_session_line_is_no_session() {
    assert_eq!(ScopeSession::parse(""), None);
}

#[test]
fn a_session_line_takes_an_unknown_key() {
    let parsed =
        ScopeSession::parse("up=1 down=2 peak=3 start=4 future=9").expect("the line parses");
    assert_eq!(parsed.started_at_ms, 4);
    assert_eq!(parsed.down_bytes, 2);
}

#[test]
fn the_session_scope_without_a_session_falls_back_to_the_default() {
    // A tile must not lose its scope because there is no session: it would
    // lose the tap that brings the scope back.
    assert_eq!(
        effective(TileScope::Session, ID_TODAY, None),
        TileScope::Today
    );
    assert_eq!(
        effective(TileScope::Session, ID_DROPS, None),
        TileScope::Last24Hours
    );
    assert_eq!(
        effective(TileScope::Today, ID_TODAY, None),
        TileScope::Today
    );
    assert_eq!(
        effective(TileScope::Session, ID_TODAY, Some(&session())),
        TileScope::Session
    );
}

#[test]
fn the_numbers_follow_the_scope() {
    let today = ScopeTotals {
        up_bytes: 1,
        down_bytes: 2,
        peak_down_bps: 3,
    };
    let (scope, numbers) = totals(TileScope::Session, ID_TODAY, today, Some(&session()));
    assert_eq!(scope, TileScope::Session);
    assert_eq!(numbers.down_bytes, session().down_bytes);
    let (scope, numbers) = totals(TileScope::Session, ID_TODAY, today, None);
    assert_eq!(scope, TileScope::Today);
    assert_eq!(numbers.down_bytes, 2);
    let (scope, numbers) = totals(TileScope::Today, ID_TODAY, today, Some(&session()));
    assert_eq!(scope, TileScope::Today);
    assert_eq!(numbers.down_bytes, 2);
}

#[test]
fn a_session_continues_inside_the_grace_with_the_same_peer() {
    // A quick stop and reconnect in a few seconds after an update
    // of the phone app. The session of that moment should not restart.
    assert!(resumes(1_000, 1_000, true));
    assert!(resumes(1_000, 1_000 + RESUME_GRACE_MS, true));
    assert!(
        !resumes(1_000, 1_001 + RESUME_GRACE_MS, true),
        "the grace is over"
    );
    assert!(
        !resumes(1_000, 1_500, false),
        "another peer is a new session"
    );
    assert!(!resumes(0, 1_500, true), "no session was stopped");
    assert!(!resumes(2_000, 1_500, true), "the stop is in the future");
}

#[test]
fn the_grace_is_two_minutes() {
    assert_eq!(RESUME_GRACE_MS, 120_000);
}
