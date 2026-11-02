//! 所要時間のモデルの初版の値（形式書8章、`jp-time-1`）。
//!
//! P1が `graph.bin` の `TIMEMODL` に書く。係数はコードではなくデータなので、M1.7でOSRMとの照合から
//! 決め直したら、ここの値を直してP1をやり直す（後続のファイルもすべて作り直す）。
//! 形式書8章に値がない項目（`*_link`、信号・一時停止・踏切の加算、右左折コスト、階段の倍率）は、
//! 照合までの仮置きである。

use crate::domain::routing::graph::HighwayClass;
use crate::domain::routing::graph::time::{PerMode, TimeModel, TurnParams};

/// モデルの版。
pub const TIME_MODEL_VERSION: &str = "jp-time-1";

/// 初版の所要時間モデル。
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

    // carの既定速度（km/h）。形式書8章の初版の値。*_linkは本線と同じ値（仮置き）。
    // 通常はcarで通れない種類（track・path・cycleway・footway・pedestrian・steps・bridleway）も、
    // `motor_vehicle=yes` などで通れるwayがあるので、低い速度を置く。
    // 速度0の（手段、種類）は `edge_time_ds` が通れない扱いにするので、`classify_way` が `access` を
    // 立て得る組には必ず0より大きい速度を置く（motorway系のbike・walkだけが0で、`access` も立たない）
    for (class, speed) in [
        (Motorway, 80.0),
        (MotorwayLink, 80.0),
        (Trunk, 50.0),
        (TrunkLink, 50.0),
        (Primary, 40.0),
        (PrimaryLink, 40.0),
        (Secondary, 35.0),
        (SecondaryLink, 35.0),
        (Tertiary, 30.0),
        (TertiaryLink, 30.0),
        (Unclassified, 25.0),
        (Residential, 25.0),
        (LivingStreet, 10.0),
        (Service, 15.0),
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
        // maxspeedタグの付与率は3.3%。実際の走行は制限速度より遅いので8割を掛け、
        // 種類の既定速度の1.5倍を上限にする
        car_maxspeed_factor: 0.8,
        car_maxspeed_cap_ratio: 1.5,
        signal_penalty_s: PerMode {
            walk: 15.0,
            bike: 10.0,
            car: 8.0,
        },
        stop_penalty_s: PerMode {
            walk: 0.0,
            bike: 3.0,
            car: 3.0,
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
                right_s: 8.0,
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

    #[test]
    fn follows_the_initial_values_in_the_format_document() {
        let m = default_time_model();
        assert_eq!(m.version, "jp-time-1");
        let c = |class: HighwayClass| m.speed_kmh.car[class.code() as usize];
        assert_eq!(c(HighwayClass::Motorway), 80.0);
        assert_eq!(c(HighwayClass::Trunk), 50.0);
        assert_eq!(c(HighwayClass::Primary), 40.0);
        assert_eq!(c(HighwayClass::Secondary), 35.0);
        assert_eq!(c(HighwayClass::Tertiary), 30.0);
        assert_eq!(c(HighwayClass::Unclassified), 25.0);
        assert_eq!(c(HighwayClass::Residential), 25.0);
        assert_eq!(c(HighwayClass::LivingStreet), 10.0);
        assert_eq!(c(HighwayClass::Service), 15.0);
        assert!(m.speed_kmh.walk.iter().filter(|&&v| v == 4.8).count() >= 20);
        assert_eq!(
            m.speed_kmh.walk[HighwayClass::Motorway.code() as usize],
            0.0
        );
        assert!(m.speed_kmh.bike.iter().filter(|&&v| v == 15.0).count() >= 19);
        for mode in [&m.turn.walk, &m.turn.bike, &m.turn.car] {
            assert!(mode.right_s >= mode.left_s);
        }
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
