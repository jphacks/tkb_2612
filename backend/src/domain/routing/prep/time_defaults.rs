//! 所要時間のモデルの値（形式書8章、`jp-time-2`）。
//!
//! P1が `graph.bin` の `TIMEMODL` に書く。係数はコードではなくデータなので、値を直したら版を上げ、
//! P1をやり直す（後続のファイルもすべて作り直す）。
//!
//! - `jp-time-1`（初版）：形式書8章の初期値。carの既定速度は motorway 80・trunk 50・primary 40・
//!   secondary 35・tertiary 30・unclassified/residential 25・living_street 10・service 15（km/h）、
//!   `maxspeed` の倍率0.8、信号8秒・一時停止3秒、右折8秒・左折3秒・Uターン20秒。
//!   関東の自動車300組で、自前÷OSRMの所要時間の比の中央値は1.289だった（目安は0.85〜1.15）。
//! - `jp-time-2`（M1.7）：carの種類別の既定速度・`maxspeed` の倍率・信号と一時停止の加算・右折の時間を、
//!   OSRMの所要時間への相対誤差の最小二乗（非負）で合わせ直した。手順は、関東の自動車600組の自前の経路の
//!   特徴量（種類別の走行距離・`maxspeed` つきの辺・信号・右左折の数）をOSRMの所要時間に回帰し、
//!   その速度で経路を探し直して回帰し直すことを3回繰り返した（係数がOSRMのcarプロファイルの値に収束した）。
//!   回帰で安定しない値（左折・Uターン・踏切・`*_link` のうち経路に出にくいもの）はjp-time-1か
//!   OSRMのプロファイルの値のままにした。歩行者・自転車の値は変えていない（形式書8章）。

use crate::domain::routing::graph::HighwayClass;
use crate::domain::routing::graph::time::{PerMode, TimeModel, TurnParams};

/// モデルの版。
pub const TIME_MODEL_VERSION: &str = "jp-time-2";

/// 既定の所要時間モデル（`TIME_MODEL_VERSION`）。
pub fn default_time_model() -> TimeModel {
    use HighwayClass::*;
    let mut walk = [4.8; HighwayClass::COUNT];
    let mut bike = [15.0; HighwayClass::COUNT];
    let mut car = [0.0; HighwayClass::COUNT];

    // motorway・motorway_link は歩行者・自転車が通れない（形式書7.2節）。速度0は「通行不可の種類」
    for class in [Motorway, MotorwayLink] {
        walk[class.code() as usize] = 0.0;
        bike[class.code() as usize] = 0.0;
    }
    // 階段は自転車でも押して歩く速さ
    bike[Steps.code() as usize] = 4.8;

    // carの既定速度（km/h）。jp-time-2：OSRMの所要時間への回帰で合わせた値（上のモジュールの説明）。
    // trunk_link・primary_link・secondary_link・tertiary_link は経路に出にくく回帰が安定しないので、
    // OSRMのcarプロファイルの値（40・30・25・20）のまま。
    // 通常はcarで通れない種類（track・path・cycleway・footway・pedestrian・steps・bridleway）も、
    // `motor_vehicle=yes` などで通れるwayがあるので、低い速度を置く。
    // 速度0の（手段、種類）は `edge_time_ds` が通れない扱いにするので、`classify_way` が `access` を
    // 立て得る組には必ず0より大きい速度を置く（motorway系のbike・walkだけが0で、`access` も立たない）
    for (class, speed) in [
        (Motorway, 89.0),
        (MotorwayLink, 48.0),
        (Trunk, 83.0),
        (TrunkLink, 40.0),
        (Primary, 63.0),
        (PrimaryLink, 30.0),
        (Secondary, 54.0),
        (SecondaryLink, 25.0),
        (Tertiary, 41.0),
        (TertiaryLink, 20.0),
        (Unclassified, 27.0),
        (Residential, 26.5),
        (LivingStreet, 10.0),
        (Service, 19.5),
        (Road, 25.0),
        (Track, 15.0),
        (Path, 10.0),
        (Cycleway, 10.0),
        (Footway, 10.0),
        (Pedestrian, 10.0),
        (Steps, 5.0),
        (Bridleway, 10.0),
    ] {
        car[class.code() as usize] = speed;
    }

    TimeModel {
        version: TIME_MODEL_VERSION.to_owned(),
        speed_kmh: PerMode { walk, bike, car },
        // maxspeedタグの付与率は3.3%。実際の走行は制限速度より遅いので8割を掛け（回帰でも0.80）、
        // 種類の既定速度の1.5倍を上限にする
        car_maxspeed_factor: 0.8,
        car_maxspeed_cap_ratio: 1.5,
        signal_penalty_s: PerMode {
            walk: 15.0,
            bike: 10.0,
            car: 2.0,
        },
        stop_penalty_s: PerMode {
            walk: 0.0,
            bike: 3.0,
            car: 1.0,
        },
        level_crossing_penalty_s: PerMode {
            walk: 10.0,
            bike: 10.0,
            car: 15.0,
        },
        steps_factor_walk: 1.5,
        // 左側通行なので、右折（対向車線を横切る）を左折より重くする
        turn: PerMode {
            walk: TurnParams {
                right_s: 1.0,
                left_s: 1.0,
                u_turn_s: 3.0,
                straight_max_deg: 30.0,
                u_turn_min_deg: 150.0,
            },
            bike: TurnParams {
                right_s: 5.0,
                left_s: 2.0,
                u_turn_s: 8.0,
                straight_max_deg: 30.0,
                u_turn_min_deg: 150.0,
            },
            car: TurnParams {
                right_s: 7.5,
                left_s: 3.0,
                u_turn_s: 20.0,
                straight_max_deg: 30.0,
                u_turn_min_deg: 150.0,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 値が `jp-time-2`（M1.7の回帰の結果）のままであること。歩行者・自転車は形式書8章の初版のまま。
    #[test]
    fn follows_the_initial_values_in_the_format_document() {
        let m = default_time_model();
        assert_eq!(m.version, "jp-time-2");
        let c = |class: HighwayClass| m.speed_kmh.car[class.code() as usize];
        assert_eq!(c(HighwayClass::Motorway), 89.0);
        assert_eq!(c(HighwayClass::MotorwayLink), 48.0);
        assert_eq!(c(HighwayClass::Trunk), 83.0);
        assert_eq!(c(HighwayClass::Primary), 63.0);
        assert_eq!(c(HighwayClass::Secondary), 54.0);
        assert_eq!(c(HighwayClass::Tertiary), 41.0);
        assert_eq!(c(HighwayClass::Unclassified), 27.0);
        assert_eq!(c(HighwayClass::Residential), 26.5);
        assert_eq!(c(HighwayClass::LivingStreet), 10.0);
        assert_eq!(c(HighwayClass::Service), 19.5);
        assert_eq!(m.car_maxspeed_factor, 0.8);
        assert_eq!(m.car_maxspeed_cap_ratio, 1.5);
        assert_eq!(m.signal_penalty_s.car, 2.0);
        assert_eq!(m.stop_penalty_s.car, 1.0);
        assert_eq!(m.turn.car.right_s, 7.5);
        assert_eq!(m.turn.car.left_s, 3.0);
        assert_eq!(m.turn.car.u_turn_s, 20.0);
        assert!(m.speed_kmh.walk.iter().filter(|&&v| v == 4.8).count() >= 20);
        assert_eq!(
            m.speed_kmh.walk[HighwayClass::Motorway.code() as usize],
            0.0
        );
        assert!(m.speed_kmh.bike.iter().filter(|&&v| v == 15.0).count() >= 19);
        for mode in [&m.turn.walk, &m.turn.bike, &m.turn.car] {
            assert!(mode.right_s >= mode.left_s);
        }
        // 歩行者・自転車は変えていない
        assert_eq!(m.signal_penalty_s.walk, 15.0);
        assert_eq!(m.signal_penalty_s.bike, 10.0);
        assert_eq!(m.turn.bike.right_s, 5.0);
        // JSONに書いて読み戻せる
        let json = serde_json::to_value(&m).unwrap();
        assert_eq!(TimeModel::from_json(&json).unwrap(), m);
    }

    /// `classify_way` が `access` を立て得る（手段、種類）の組には、必ず0より大きい速度がある。
    #[test]
    fn every_passable_mode_and_class_pair_has_a_speed() {
        use crate::domain::routing::graph::access;
        use crate::domain::routing::graph::rules::classify_way;
        let m = default_time_model();
        let overrides: [&[(&str, &str)]; 8] = [
            &[],
            &[("motor_vehicle", "yes")],
            &[("motorcar", "yes")],
            &[("bicycle", "yes")],
            &[("foot", "yes")],
            &[("vehicle", "yes"), ("bicycle", "designated")],
            &[
                ("motor_vehicle", "yes"),
                ("bicycle", "yes"),
                ("foot", "yes"),
            ],
            &[("motorroad", "yes")],
        ];
        for class in HighwayClass::ALL {
            for extra in overrides {
                let mut tags = vec![("highway", class.highway_tag())];
                tags.extend_from_slice(extra);
                let Some(c) = classify_way(&tags) else {
                    continue;
                };
                let i = class.code() as usize;
                if c.access & (access::CAR_FWD | access::CAR_BWD) != 0 {
                    assert!(m.speed_kmh.car[i] > 0.0, "car {tags:?}");
                }
                if c.access & (access::BIKE_FWD | access::BIKE_BWD) != 0 {
                    assert!(m.speed_kmh.bike[i] > 0.0, "bike {tags:?}");
                }
                if c.access & (access::WALK_FWD | access::WALK_BWD) != 0 {
                    assert!(m.speed_kmh.walk[i] > 0.0, "walk {tags:?}");
                }
            }
        }
    }
}
