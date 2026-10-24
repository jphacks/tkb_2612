//! 共有契約の型・境界値・期限/キャンセルの約束事。

use std::time::{Duration, Instant};

use accident_route_backend::contract::engine::{validate_endpoints, validate_max_detour};
use accident_route_backend::contract::geo::{METERS_PER_DEGREE, straight_distance_m};
use accident_route_backend::contract::{
    Calendar, CancelToken, ConditionFilter, DayClassification, DayType, DayTypeReason, EngineError,
    Interrupt, LatLon, LonLat, Mode, Period, QueryError, RouteQuery, RoutingError, TokyoMinute,
    Weather,
};
use accident_route_backend::testing::FixedCalendar;
use chrono::{NaiveDate, NaiveDateTime};

#[test]
fn period_from_hour_matches_five_bands() {
    let expected = [
        (0, Period::LateNight),
        (3, Period::LateNight),
        (4, Period::EarlyMorning),
        (8, Period::EarlyMorning),
        (9, Period::Daytime),
        (15, Period::Daytime),
        (16, Period::Evening),
        (18, Period::Evening),
        (19, Period::Night),
        (23, Period::Night),
    ];
    for (hour, period) in expected {
        assert_eq!(Period::from_hour(hour), Some(period), "{hour}時");
    }
    for hour in 0..=23 {
        assert!(Period::from_hour(hour).is_some());
    }
    assert_eq!(Period::from_hour(24), None);
}

#[test]
fn codes_and_labels_follow_openapi() {
    let periods: Vec<_> = Period::ALL.iter().map(|p| (p.code(), p.label())).collect();
    assert_eq!(
        periods,
        [(0, "早朝"), (1, "昼"), (2, "夕"), (3, "夜"), (4, "深夜")]
    );
    let weathers: Vec<_> = Weather::ALL.iter().map(|w| (w.code(), w.label())).collect();
    assert_eq!(
        weathers,
        [(1, "晴"), (2, "曇"), (3, "雨"), (4, "霧"), (5, "雪")]
    );
    let day_types: Vec<_> = DayType::ALL.iter().map(|d| (d.code(), d.label())).collect();
    assert_eq!(day_types, [(0, "平日"), (1, "休日")]);
    let modes: Vec<_> = Mode::ALL.iter().map(|m| (m.as_str(), m.mask())).collect();
    assert_eq!(modes, [("walk", 1), ("bike", 2), ("car", 4)]);

    for p in Period::ALL {
        assert_eq!(Period::from_code(p.code()), Some(p));
        assert_eq!(Period::from_label(p.label()), Some(p));
    }
    for w in Weather::ALL {
        assert_eq!(Weather::from_code(w.code()), Some(w));
        assert_eq!(Weather::from_label(w.label()), Some(w));
    }
    for d in DayType::ALL {
        assert_eq!(DayType::from_code(d.code()), Some(d));
        assert_eq!(DayType::from_label(d.label()), Some(d));
    }
    assert_eq!(Period::from_code(5), None);
    assert_eq!(Weather::from_code(0), None);
    assert_eq!(Period::from_label("all"), None);
    assert_eq!(Mode::parse("driving"), None);
}

#[test]
fn coordinate_types_keep_names_across_conversion() {
    let p = LatLon::new(35.0, 139.0);
    let q = LonLat::from(p);
    assert_eq!((q.lon, q.lat), (139.0, 35.0));
    assert_eq!(LatLon::from(q), p);
    assert!(LatLon::new(20.0, 122.0).is_in_japan());
    assert!(LatLon::new(46.0, 154.0).is_in_japan());
    assert!(!LatLon::new(19.999, 139.0).is_in_japan());
    assert!(!LatLon::new(35.0, f64::NAN).is_in_japan());
    // 緯度経度を逆に入れると範囲外になる
    assert!(!LatLon::new(139.0, 35.0).is_in_japan());
}

/// 東西方向にdメートル離れた点。
fn east_of(origin: LatLon, d: f64) -> LatLon {
    let kx = METERS_PER_DEGREE * (origin.lat * std::f64::consts::PI / 180.0).cos();
    LatLon::new(origin.lat, origin.lon + d / kx)
}

#[test]
fn straight_distance_bounds_are_inclusive() {
    let o = LatLon::new(35.0, 139.0);
    assert!((straight_distance_m(o, east_of(o, 1000.0)) - 1000.0).abs() < 1e-6);
    assert_eq!(validate_endpoints(o, o), Err(QueryError::StraightDistance));
    assert_eq!(
        validate_endpoints(o, east_of(o, 19.99)),
        Err(QueryError::StraightDistance)
    );
    assert_eq!(validate_endpoints(o, east_of(o, 20.01)), Ok(()));
    assert_eq!(validate_endpoints(o, east_of(o, 29_999.9)), Ok(()));
    assert_eq!(
        validate_endpoints(o, east_of(o, 30_000.1)),
        Err(QueryError::StraightDistance)
    );
    assert_eq!(
        validate_endpoints(LatLon::new(50.0, 139.0), o),
        Err(QueryError::InvalidOrigin)
    );
    assert_eq!(
        validate_endpoints(o, LatLon::new(35.0, 160.0)),
        Err(QueryError::InvalidDest)
    );
}

#[test]
fn max_detour_bounds() {
    assert_eq!(validate_max_detour(0.0), Ok(()));
    assert_eq!(validate_max_detour(30.0), Ok(()));
    assert_eq!(validate_max_detour(7.5), Ok(()));
    for bad in [-0.1, 30.01, f64::NAN, f64::INFINITY] {
        assert_eq!(
            validate_max_detour(bad),
            Err(QueryError::MaxDetour),
            "{bad}"
        );
    }
    let o = LatLon::new(35.0, 139.0);
    let q = RouteQuery::new(
        o,
        east_of(o, 500.0),
        Mode::Bike,
        10.0,
        ConditionFilter::default(),
    )
    .unwrap();
    assert_eq!(
        (q.origin(), q.mode(), q.max_detour_min()),
        (o, Mode::Bike, 10.0)
    );
    assert_eq!(
        RouteQuery::new(
            o,
            east_of(o, 500.0),
            Mode::Walk,
            31.0,
            ConditionFilter::default()
        ),
        Err(QueryError::MaxDetour)
    );
    for e in [
        QueryError::InvalidOrigin,
        QueryError::InvalidDest,
        QueryError::StraightDistance,
        QueryError::MaxDetour,
    ] {
        assert!(!e.message().is_empty());
    }
}

#[test]
fn cancel_token_reports_cancel_before_deadline() {
    let unbounded = CancelToken::unbounded();
    assert_eq!(unbounded.check(), Ok(()));
    assert_eq!(unbounded.remaining(), None);

    let live = CancelToken::with_timeout(Duration::from_secs(60));
    assert_eq!(live.check(), Ok(()));
    assert!(live.remaining().unwrap() > Duration::from_secs(59));

    let expired = CancelToken::with_deadline(Instant::now() - Duration::from_millis(1));
    assert_eq!(expired.check(), Err(Interrupt::DeadlineExceeded));
    assert_eq!(expired.remaining(), Some(Duration::ZERO));

    // 複製は状態を共有し、取消しは期限切れより優先する
    let clone = expired.clone();
    expired.cancel();
    assert_eq!(clone.check(), Err(Interrupt::Cancelled));
}

#[test]
fn cancel_guard_cancels_on_drop_unless_disarmed() {
    let token = CancelToken::unbounded();
    drop(token.cancel_on_drop());
    assert!(token.is_cancelled());

    let token = CancelToken::unbounded();
    token.cancel_on_drop().disarm();
    assert!(!token.is_cancelled());
    assert_eq!(token.check(), Ok(()));
}

#[test]
fn routing_errors_that_abort_the_whole_request() {
    assert!(!RoutingError::NoRoute.aborts_request());
    assert!(!RoutingError::ExternalApi.aborts_request());
    assert!(RoutingError::Busy.aborts_request());
    assert!(RoutingError::Interrupted(Interrupt::DeadlineExceeded).aborts_request());
    assert!(RoutingError::Interrupted(Interrupt::Cancelled).aborts_request());

    assert_eq!(
        EngineError::from(RoutingError::NoRoute),
        EngineError::NoRoute
    );
    assert_eq!(
        EngineError::from(RoutingError::ExternalApi),
        EngineError::ExternalApi
    );
    assert_eq!(EngineError::from(RoutingError::Busy), EngineError::Busy);
    assert_eq!(
        EngineError::from(RoutingError::Interrupted(Interrupt::DeadlineExceeded)),
        EngineError::Interrupted(Interrupt::DeadlineExceeded)
    );
    assert_eq!(
        EngineError::from(Interrupt::Cancelled),
        EngineError::Interrupted(Interrupt::Cancelled)
    );
}

fn local(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").unwrap()
}

#[test]
fn tokyo_minute_truncates_seconds_and_calendar_uses_shared_period() {
    let at = TokyoMinute::from_local(local("2026-11-03T03:59:59.999"));
    assert_eq!(at.as_local(), local("2026-11-03T03:59:00.0"));
    assert_eq!(at.hour(), 3);

    let calendar = FixedCalendar(DayClassification {
        reason: DayTypeReason::NationalHoliday {
            name: "文化の日".into(),
        },
        holiday_table_covered: true,
    });
    let late = calendar.classify(at);
    assert_eq!(late.period, Period::LateNight);
    assert_eq!(late.day.day_type(), DayType::Holiday);
    assert_eq!(late.day.reason.label(), "祝日：文化の日");

    let early = calendar.classify(TokyoMinute::from_local(local("2026-11-03T04:00:00.0")));
    assert_eq!(early.period, Period::EarlyMorning);
    assert_eq!(
        calendar.classify_date(NaiveDate::from_ymd_opt(2026, 11, 3).unwrap()),
        late.day
    );
}

#[test]
fn day_type_reason_labels() {
    let cases = [
        (DayTypeReason::Weekday, "平日", DayType::Weekday),
        (DayTypeReason::Saturday, "土曜", DayType::Holiday),
        (DayTypeReason::Sunday, "日曜", DayType::Holiday),
        (
            DayTypeReason::SubstituteHoliday,
            "振替休日",
            DayType::Holiday,
        ),
        (
            DayTypeReason::CitizensHoliday,
            "国民の休日",
            DayType::Holiday,
        ),
        (DayTypeReason::YearEnd, "年末年始", DayType::Holiday),
        (
            DayTypeReason::NationalHoliday {
                name: "元日".into(),
            },
            "祝日：元日",
            DayType::Holiday,
        ),
    ];
    for (reason, label, day_type) in cases {
        assert_eq!(reason.label(), label);
        assert_eq!(reason.day_type(), day_type);
    }
}
