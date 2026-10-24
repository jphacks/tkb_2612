//! 結合テストの共通補助（人工の事故スナップショットと経路）。
#![allow(dead_code)]

pub mod source;

use std::sync::Arc;

use accident_route_backend::contract::geo::METERS_PER_DEGREE;
use accident_route_backend::contract::{
    ConditionFilter, DatasetInfo, DayType, LatLon, LonLat, Mode, Period, RouteQuery, RoutedPath,
    Weather,
};
use accident_route_backend::domain::{AccidentColumns, AccidentEngine, AccidentSnapshot};

/// 人工事故1件。
#[derive(Debug, Clone, Copy)]
pub struct Acc {
    pub lon: f64,
    pub lat: f64,
    pub modes: u8,
    pub period: Period,
    pub weather: Weather,
    pub day_type: DayType,
    pub deaths: u16,
    pub injuries: u16,
}

impl Acc {
    /// 全手段・昼・晴・平日の負傷事故（負傷1人）。
    pub fn at(lon: f64, lat: f64) -> Self {
        Self {
            lon,
            lat,
            modes: 7,
            period: Period::Daytime,
            weather: Weather::Clear,
            day_type: DayType::Weekday,
            deaths: 0,
            injuries: 1,
        }
    }

    pub fn casualties(mut self, deaths: u16, injuries: u16) -> Self {
        self.deaths = deaths;
        self.injuries = injuries;
        self
    }

    pub fn modes(mut self, modes: u8) -> Self {
        self.modes = modes;
        self
    }

    pub fn period(mut self, period: Period) -> Self {
        self.period = period;
        self
    }

    pub fn weather(mut self, weather: Weather) -> Self {
        self.weather = weather;
        self
    }

    pub fn day_type(mut self, day_type: DayType) -> Self {
        self.day_type = day_type;
        self
    }
}

pub fn snapshot(accidents: &[Acc]) -> AccidentSnapshot {
    let mut c = AccidentColumns::default();
    for a in accidents {
        c.lat.push(a.lat);
        c.lon.push(a.lon);
        c.modes.push(a.modes);
        c.period.push(a.period.code());
        c.weather.push(a.weather.code());
        c.day_type.push(a.day_type.code());
        c.deaths.push(a.deaths);
        c.injuries.push(a.injuries);
        c.source_year.push(2025);
        c.date.push(20250106);
    }
    AccidentSnapshot::new(c)
}

pub fn dataset(accidents: u64) -> DatasetInfo {
    DatasetInfo {
        release_id: "1".repeat(64),
        dataset_id: "2".repeat(64),
        accidents,
        data_years: vec![2025],
        load_seconds: 0.0,
        grid_cells: None,
    }
}

pub fn engine(accidents: &[Acc]) -> Arc<AccidentEngine> {
    Arc::new(AccidentEngine::new(
        Arc::new(snapshot(accidents)),
        dataset(accidents.len() as u64),
    ))
}

pub const ORIGIN: LatLon = LatLon::new(35.0, 139.0);
pub const DEST: LatLon = LatLon::new(35.0, 139.01);

pub fn query(mode: Mode, max_detour_min: f64, conditions: ConditionFilter) -> RouteQuery {
    RouteQuery::new(ORIGIN, DEST, mode, max_detour_min, conditions).unwrap()
}

pub fn path(duration_s: f64, distance_m: f64, coordinates: &[(f64, f64)]) -> RoutedPath {
    RoutedPath {
        duration_s,
        distance_m,
        coordinates: coordinates
            .iter()
            .map(|&(lon, lat)| LonLat::new(lon, lat))
            .collect(),
    }
}

/// 緯度35度の東西の直線（139.0から `length_m` だけ東へ）。
pub fn straight_east(duration_s: f64, distance_m: f64, length_m: f64) -> RoutedPath {
    let kx = METERS_PER_DEGREE * (35f64 * std::f64::consts::PI / 180.0).cos();
    path(
        duration_s,
        distance_m,
        &[(139.0, 35.0), (139.0 + length_m / kx, 35.0)],
    )
}

/// 緯度35度で、経路から北へ `meters` 離れた点の緯度。
pub fn lat_north_of_35(meters: f64) -> f64 {
    35.0 + meters / METERS_PER_DEGREE
}
