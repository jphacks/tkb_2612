//! 手段別の通行規則（形式書7章、`jp-rules-1`）。
//!
//! P1（`routing_prep osm-graph`）が、wayのタグから `class`・`access`・`flags`・`maxspeed_kmh` を決める
//! 純粋関数。規則を変えたら版を上げ、`graph.bin` の `PARAMS.rules_version` と `metadata.json` に記録する。
//! タグがないことを「禁止なし」や「安全」と同じに扱わない（計画書3.2節）。
//!
//! 形式書7.2節の「上から順に適用し、後のものが優先」を、次の順番で実装している。
//!
//! 1. 種類ごとの既定（7.1節）
//! 2. `access`
//! 3. `vehicle`
//! 4. `motor_vehicle`、`motorcar`
//! 5. `bicycle`
//! 6. `foot`
//! 7. `motorroad=yes`、motorway・motorway_link（bike・walkを不可）
//!
//! 一方通行（7.2節の7〜9）は、通行できる手段ごとに向きを決めて `access` のビットにする。

use super::HighwayClass;
use super::access;
use super::edge_flags;
use super::node_flags;

/// 規則の版（形式書7章）。
pub const RULES_VERSION: &str = "jp-rules-1";

/// wayの分類の結果（形式書6.2節の `EdgeRecord` のうちタグから決まる部分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WayClassification {
    /// 道路の種類（6.5節）。
    pub class: HighwayClass,
    /// `access` のビット（7.1節・7.2節）。向きは「wayのノード順」を始点→終点とする。
    pub access: u8,
    /// `edge_flags` のビット。
    pub flags: u8,
    /// `maxspeed`（7.4節）。0は不明。
    pub maxspeed_kmh: u8,
}

/// グラフに入れないwayの理由（監査用。`graph.bin` の `PARAMS` に件数を記録する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WayExclusion {
    /// `highway` タグがない。道路ではないので、件数にも数えない。
    NotHighway,
    /// `highway` の値がグラフの種類（6.5節）にない（`construction`・`proposed`・`platform` など）。
    UnsupportedHighway,
    /// `area=yes`（6.5節）。
    Area,
    /// 7.1節・7.2節の結果、どの手段のどの向きにも通れない。
    NoAccess,
}

impl WayExclusion {
    /// 監査JSONのキー。
    pub const fn as_str(self) -> &'static str {
        match self {
            WayExclusion::NotHighway => "not_highway",
            WayExclusion::UnsupportedHighway => "unsupported_highway",
            WayExclusion::Area => "area_yes",
            WayExclusion::NoAccess => "no_access",
        }
    }
}

fn tag<'a>(tags: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    tags.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// 手段ごとの可否。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Allowed {
    car: bool,
    bike: bool,
    walk: bool,
}

impl Allowed {
    const NONE: Allowed = Allowed {
        car: false,
        bike: false,
        walk: false,
    };
}

/// 一方通行の向き（wayのノード順を基準にする）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Both,
    Forward,
    Backward,
}

impl Direction {
    fn bits(self, forward: u8, backward: u8) -> u8 {
        match self {
            Direction::Both => forward | backward,
            Direction::Forward => forward,
            Direction::Backward => backward,
        }
    }
}

/// `oneway` 系のタグの値。`yes/true/1` は始点→終点、`-1/reverse` は終点→始点、`no/false/0` は両方向。
fn parse_oneway(value: &str) -> Option<Direction> {
    match value {
        "yes" | "true" | "1" => Some(Direction::Forward),
        "-1" | "reverse" => Some(Direction::Backward),
        "no" | "false" | "0" => Some(Direction::Both),
        _ => None,
    }
}

/// 7.1節：種類ごとの既定。
fn default_allowed(class: HighwayClass, tags: &[(&str, &str)]) -> Allowed {
    use HighwayClass::*;
    match class {
        Motorway | MotorwayLink => Allowed {
            car: true,
            bike: false,
            walk: false,
        },
        Trunk | TrunkLink | Primary | PrimaryLink | Secondary | SecondaryLink | Tertiary
        | TertiaryLink | Unclassified | Residential | LivingStreet | Service | Road => Allowed {
            car: true,
            bike: true,
            walk: true,
        },
        Track | Path | Cycleway => Allowed {
            car: false,
            bike: true,
            walk: true,
        },
        Footway | Pedestrian => Allowed {
            car: false,
            bike: matches!(
                tag(tags, "bicycle"),
                Some("yes" | "designated" | "permissive")
            ),
            walk: true,
        },
        Steps => Allowed {
            car: false,
            bike: false,
            walk: true,
        },
        Bridleway => Allowed::NONE,
    }
}

/// 7.2節の1〜6（`access`・`vehicle`・`motor_vehicle`/`motorcar`・`bicycle`・`foot`）を順に適用する。
/// `default` は `vehicle=yes` などで「既定に戻す」ときの値（既定が不可のものは可にならない）。
fn apply_access_tags(allowed: &mut Allowed, default: Allowed, tags: &[(&str, &str)]) {
    // 1. access
    if let Some("no" | "private" | "agricultural" | "forestry" | "delivery") = tag(tags, "access") {
        *allowed = Allowed::NONE;
    }
    // 2. vehicle（car・bike）
    match tag(tags, "vehicle") {
        Some("no" | "private") => {
            allowed.car = false;
            allowed.bike = false;
        }
        Some("yes" | "permissive" | "destination") => {
            allowed.car = default.car;
            allowed.bike = default.bike;
        }
        _ => {}
    }
    // 3. motor_vehicle、motorcar（より限定的な motorcar を後に適用する）
    for key in ["motor_vehicle", "motorcar"] {
        match tag(tags, key) {
            Some("no" | "private") => allowed.car = false,
            Some("yes" | "destination") => allowed.car = true,
            _ => {}
        }
    }
    // 4. bicycle
    match tag(tags, "bicycle") {
        Some("no" | "use_sidepath" | "dismount") => allowed.bike = false,
        Some("yes" | "designated" | "permissive" | "destination") => allowed.bike = true,
        _ => {}
    }
    // 5. foot
    match tag(tags, "foot") {
        Some("no" | "use_sidepath") => allowed.walk = false,
        Some("yes" | "designated" | "permissive" | "destination") => allowed.walk = true,
        _ => {}
    }
}

/// `tunnel`・`bridge` のように、値が `no` 以外ならその構造物とみなすタグ。
fn is_structure(value: Option<&str>) -> bool {
    matches!(value, Some(v) if !v.is_empty() && v != "no")
}

/// wayのタグから分類し、グラフに入れない理由も返す。`classify_way` はこの結果の `ok()`。
pub fn classify_way_detailed(tags: &[(&str, &str)]) -> Result<WayClassification, WayExclusion> {
    let highway = tag(tags, "highway").ok_or(WayExclusion::NotHighway)?;
    let class = HighwayClass::from_highway_tag(highway).ok_or(WayExclusion::UnsupportedHighway)?;
    if tag(tags, "area") == Some("yes") {
        return Err(WayExclusion::Area);
    }

    let default = default_allowed(class, tags);
    let mut allowed = default;
    apply_access_tags(&mut allowed, default, tags);

    // 6. motorroad=yes、motorway・motorway_link は bike・walk を不可（4・5のタグより優先）
    let motorroad = tag(tags, "motorroad") == Some("yes");
    if motorroad || matches!(class, HighwayClass::Motorway | HighwayClass::MotorwayLink) {
        allowed.bike = false;
        allowed.walk = false;
    }

    // 7. 一方通行（carとbike）
    let roundabout = matches!(tag(tags, "junction"), Some("roundabout" | "circular"));
    let reversible = matches!(tag(tags, "oneway"), Some("reversible" | "alternating"));
    let car_direction = match tag(tags, "oneway").and_then(parse_oneway) {
        Some(direction) => direction,
        None if matches!(class, HighwayClass::Motorway | HighwayClass::MotorwayLink)
            || roundabout =>
        {
            Direction::Forward
        }
        None => Direction::Both,
    };
    if reversible {
        // 向きが時間で変わる道路は、carでは使わない
        allowed.car = false;
    }

    // 8. bikeの一方通行：carの一方通行に従う。ただし逆走を許すタグがあれば両方向、
    //    `oneway:bicycle=yes/-1` があればそれに従う。reversible/alternating はbikeを両方向にする
    let mut bike_direction = if reversible {
        Direction::Both
    } else {
        car_direction
    };
    let opposite_cycleway = [
        "cycleway",
        "cycleway:left",
        "cycleway:right",
        "cycleway:both",
    ]
    .iter()
    .any(|key| matches!(tag(tags, key), Some(v) if v.starts_with("opposite")));
    if opposite_cycleway {
        bike_direction = Direction::Both;
    }
    if let Some(direction) = tag(tags, "oneway:bicycle").and_then(parse_oneway) {
        bike_direction = direction;
    }

    // 9. walkは一方通行に従わない。`oneway:foot=yes/-1` があるときだけ従う
    let walk_direction = match tag(tags, "oneway:foot").and_then(parse_oneway) {
        Some(direction @ (Direction::Forward | Direction::Backward)) => direction,
        _ => Direction::Both,
    };

    let mut access_bits = 0u8;
    if allowed.car {
        access_bits |= car_direction.bits(access::CAR_FWD, access::CAR_BWD);
    }
    if allowed.bike {
        access_bits |= bike_direction.bits(access::BIKE_FWD, access::BIKE_BWD);
    }
    if allowed.walk {
        access_bits |= walk_direction.bits(access::WALK_FWD, access::WALK_BWD);
    }
    if access_bits == 0 {
        return Err(WayExclusion::NoAccess);
    }

    let mut flags = 0u8;
    if motorroad {
        flags |= edge_flags::MOTORROAD;
    }
    if tag(tags, "footway") == Some("sidewalk") {
        flags |= edge_flags::SIDEWALK;
    }
    if tag(tags, "footway") == Some("crossing")
        || (class == HighwayClass::Footway && matches!(tag(tags, "crossing"), Some(v) if v != "no"))
    {
        flags |= edge_flags::CROSSING;
    }
    if is_structure(tag(tags, "tunnel")) {
        flags |= edge_flags::TUNNEL;
    }
    if is_structure(tag(tags, "bridge")) {
        flags |= edge_flags::BRIDGE;
    }
    if roundabout {
        flags |= edge_flags::ROUNDABOUT;
    }
    if tag(tags, "toll") == Some("yes") {
        flags |= edge_flags::TOLL;
    }

    Ok(WayClassification {
        class,
        access: access_bits,
        flags,
        maxspeed_kmh: tag(tags, "maxspeed").map(parse_maxspeed).unwrap_or(0),
    })
}

/// wayのタグ（キー、値）の組から分類する。グラフに入れないwayや、`access` が0になるwayは `None`
/// （形式書6.5節・6.2節）。
pub fn classify_way(tags: &[(&str, &str)]) -> Option<WayClassification> {
    classify_way_detailed(tags).ok()
}

/// ノードのタグから、`NODEFLG` のビット4〜6（car・bike・walkの通過不可、形式書7.3節）を返す。
/// 障害物でなければ0。
pub fn classify_barrier(tags: &[(&str, &str)]) -> u8 {
    let Some(barrier) = tag(tags, "barrier") else {
        return 0;
    };
    // 通過できるかどうか（表の「可」が真）
    let mut allowed = match barrier {
        "bollard" | "block" | "jersey_barrier" | "bus_trap" | "cycle_barrier" | "chain"
        | "lift_gate" | "gate" | "swing_gate" => Allowed {
            car: false,
            bike: true,
            walk: true,
        },
        "kissing_gate" | "stile" | "turnstile" => Allowed {
            car: false,
            bike: false,
            walk: true,
        },
        "fence" | "wall" => Allowed::NONE,
        _ => return 0,
    };
    // ゲート類は、`access=yes/permissive` 等のタグがあればcarも通れる
    if matches!(barrier, "chain" | "lift_gate" | "gate" | "swing_gate")
        && matches!(
            tag(tags, "access"),
            Some("yes" | "permissive" | "destination" | "customers")
        )
    {
        allowed.car = true;
    }
    let default = allowed;
    apply_access_tags(&mut allowed, default, tags);

    let mut bits = 0u8;
    if !allowed.car {
        bits |= node_flags::BLOCK_CAR;
    }
    if !allowed.bike {
        bits |= node_flags::BLOCK_BIKE;
    }
    if !allowed.walk {
        bits |= node_flags::BLOCK_WALK;
    }
    bits
}

/// ノードのタグから `NODEFLG` のビットを決める（形式書6.3節）。信号・横断歩道・一時停止・踏切と、
/// 障害物（`classify_barrier`）。該当しなければ0。
pub fn classify_node(tags: &[(&str, &str)]) -> u8 {
    let mut bits = classify_barrier(tags);
    let highway = tag(tags, "highway");
    if highway == Some("traffic_signals") || tag(tags, "crossing") == Some("traffic_signals") {
        bits |= node_flags::SIGNAL;
    }
    if highway == Some("crossing") {
        bits |= node_flags::CROSSING;
    }
    if highway == Some("stop") {
        bits |= node_flags::STOP;
    }
    if matches!(tag(tags, "railway"), Some("level_crossing" | "crossing")) {
        bits |= node_flags::LEVEL_CROSSING;
    }
    bits
}

/// `maxspeed` タグの値をkm/hにする（形式書7.4節）。解釈できない値は0、255以上は255。
///
/// 整数（`40`、`40 km/h`）はその値、`mph` 付きは1.609倍して四捨五入する。`JP:urban` などの記号、
/// `40;50` のような複数の値、単位が不明な値は0（不明）。
pub fn parse_maxspeed(value: &str) -> u8 {
    let value = value.trim();
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(end);
    let Ok(number) = number.parse::<f64>() else {
        return 0;
    };
    let kmh = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "km/h" | "kmh" | "kph" => number,
        "mph" => number * 1.609,
        _ => return 0,
    };
    let rounded = kmh.round();
    if !rounded.is_finite() || rounded < 1.0 {
        0
    } else if rounded >= 255.0 {
        255
    } else {
        rounded as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAR: u8 = access::CAR_FWD | access::CAR_BWD;
    const BIKE: u8 = access::BIKE_FWD | access::BIKE_BWD;
    const WALK: u8 = access::WALK_FWD | access::WALK_BWD;

    fn classify(tags: &[(&str, &str)]) -> Option<WayClassification> {
        classify_way(tags)
    }

    fn access_of(tags: &[(&str, &str)]) -> u8 {
        classify(tags).map_or(0, |c| c.access)
    }

    #[test]
    fn version_is_recorded() {
        assert_eq!(RULES_VERSION, "jp-rules-1");
    }

    // --- 7.1節：種類ごとの既定 ---

    #[test]
    fn defaults_by_class() {
        // motorway・motorway_link：carのみ（一方通行）
        for h in ["motorway", "motorway_link"] {
            let c = classify(&[("highway", h)]).unwrap();
            assert_eq!(c.access, access::CAR_FWD, "{h}");
        }
        // trunk〜tertiary（link含む）・unclassified・residential・living_street・service・road：全手段
        for h in [
            "trunk",
            "trunk_link",
            "primary",
            "primary_link",
            "secondary",
            "secondary_link",
            "tertiary",
            "tertiary_link",
            "unclassified",
            "residential",
            "living_street",
            "service",
            "road",
        ] {
            let c = classify(&[("highway", h)]).unwrap();
            assert_eq!(c.access, CAR | BIKE | WALK, "{h}");
            assert_eq!(c.class.highway_tag(), h);
        }
        // track・path・cycleway：bikeとwalk
        for h in ["track", "path", "cycleway"] {
            assert_eq!(access_of(&[("highway", h)]), BIKE | WALK, "{h}");
        }
        // footway・pedestrian：walkのみ。bicycle=yes/designated/permissive のときだけbike
        for h in ["footway", "pedestrian"] {
            assert_eq!(access_of(&[("highway", h)]), WALK, "{h}");
            for v in ["yes", "designated", "permissive"] {
                assert_eq!(
                    access_of(&[("highway", h), ("bicycle", v)]),
                    BIKE | WALK,
                    "{h} bicycle={v}"
                );
            }
            assert_eq!(
                access_of(&[("highway", h), ("bicycle", "dismount")]),
                WALK,
                "{h}"
            );
        }
        // steps：walkのみ
        assert_eq!(access_of(&[("highway", "steps")]), WALK);
        // bridleway：どれも不可なのでグラフに入らない。foot=yes で walk だけ可
        assert_eq!(classify(&[("highway", "bridleway")]), None);
        assert_eq!(
            access_of(&[("highway", "bridleway"), ("foot", "yes")]),
            WALK
        );
    }

    #[test]
    fn highway_classes_outside_the_graph_are_excluded() {
        for h in [
            "construction",
            "proposed",
            "abandoned",
            "platform",
            "corridor",
            "elevator",
            "bus_guideway",
            "raceway",
            "services",
            "rest_area",
        ] {
            assert_eq!(
                classify_way_detailed(&[("highway", h)]),
                Err(WayExclusion::UnsupportedHighway),
                "{h}"
            );
        }
        assert_eq!(
            classify_way_detailed(&[("building", "yes")]),
            Err(WayExclusion::NotHighway)
        );
        assert_eq!(classify(&[]), None);
        // area=yes のwayは入れない
        assert_eq!(
            classify_way_detailed(&[("highway", "pedestrian"), ("area", "yes")]),
            Err(WayExclusion::Area)
        );
        assert!(classify(&[("highway", "pedestrian"), ("area", "no")]).is_some());
    }

    // --- 7.2節：タグによる上書き ---

    #[test]
    fn access_tag_row1() {
        for v in ["no", "private", "agricultural", "forestry", "delivery"] {
            assert_eq!(
                classify_way_detailed(&[("highway", "residential"), ("access", v)]),
                Err(WayExclusion::NoAccess),
                "access={v}"
            );
        }
        for v in ["yes", "permissive", "destination", "customers"] {
            assert_eq!(
                access_of(&[("highway", "residential"), ("access", v)]),
                CAR | BIKE | WALK,
                "access={v}"
            );
        }
        // access=no でも、後のタグ（foot）が優先
        assert_eq!(
            access_of(&[
                ("highway", "residential"),
                ("access", "no"),
                ("foot", "yes")
            ]),
            WALK
        );
        assert_eq!(
            access_of(&[
                ("highway", "service"),
                ("access", "private"),
                ("bicycle", "permissive")
            ]),
            BIKE
        );
    }

    #[test]
    fn vehicle_tag_row2() {
        for v in ["no", "private"] {
            assert_eq!(
                access_of(&[("highway", "residential"), ("vehicle", v)]),
                WALK,
                "vehicle={v}"
            );
        }
        // vehicle=yes は、既定が不可のものを可にしない
        for v in ["yes", "permissive", "destination"] {
            assert_eq!(
                access_of(&[("highway", "track"), ("vehicle", v)]),
                BIKE | WALK,
                "track vehicle={v}"
            );
            assert_eq!(
                access_of(&[("highway", "footway"), ("vehicle", v)]),
                WALK,
                "footway vehicle={v}"
            );
            assert_eq!(
                access_of(&[("highway", "motorway"), ("vehicle", v)]),
                access::CAR_FWD,
                "motorway vehicle={v}"
            );
        }
        // access=no の後の vehicle=yes は、既定が可のcar・bikeを戻す
        assert_eq!(
            access_of(&[
                ("highway", "residential"),
                ("access", "no"),
                ("vehicle", "yes")
            ]),
            CAR | BIKE
        );
    }

    #[test]
    fn motor_vehicle_tags_row3() {
        for key in ["motor_vehicle", "motorcar"] {
            for v in ["no", "private"] {
                assert_eq!(
                    access_of(&[("highway", "residential"), (key, v)]),
                    BIKE | WALK,
                    "{key}={v}"
                );
            }
            // yes/destination は既定が不可のtrackでもcarを可にする
            for v in ["yes", "destination"] {
                assert_eq!(
                    access_of(&[("highway", "track"), (key, v)]),
                    CAR | BIKE | WALK,
                    "{key}={v}"
                );
            }
        }
        // より限定的な motorcar が motor_vehicle に優先する
        assert_eq!(
            access_of(&[
                ("highway", "track"),
                ("motor_vehicle", "no"),
                ("motorcar", "yes")
            ]),
            CAR | BIKE | WALK
        );
        assert_eq!(
            access_of(&[
                ("highway", "residential"),
                ("motor_vehicle", "yes"),
                ("motorcar", "no")
            ]),
            BIKE | WALK
        );
        // vehicle=no の後の motor_vehicle=yes が優先
        assert_eq!(
            access_of(&[
                ("highway", "residential"),
                ("vehicle", "no"),
                ("motor_vehicle", "yes")
            ]),
            CAR | WALK
        );
    }

    #[test]
    fn bicycle_and_foot_tags_rows4_and_5() {
        for v in ["no", "use_sidepath", "dismount"] {
            assert_eq!(
                access_of(&[("highway", "residential"), ("bicycle", v)]),
                CAR | WALK,
                "bicycle={v}"
            );
            assert_eq!(
                access_of(&[("highway", "cycleway"), ("bicycle", v)]),
                WALK,
                "cycleway bicycle={v}"
            );
        }
        for v in ["yes", "designated", "permissive", "destination"] {
            assert_eq!(
                access_of(&[("highway", "steps"), ("bicycle", v)]),
                BIKE | WALK,
                "steps bicycle={v}"
            );
        }
        for v in ["no", "use_sidepath"] {
            assert_eq!(
                access_of(&[("highway", "residential"), ("foot", v)]),
                CAR | BIKE,
                "foot={v}"
            );
        }
        for v in ["yes", "designated", "permissive", "destination"] {
            assert_eq!(
                access_of(&[("highway", "bridleway"), ("foot", v)]),
                WALK,
                "foot={v}"
            );
        }
        // 後のタグが優先：vehicle=no の後の bicycle=yes
        assert_eq!(
            access_of(&[
                ("highway", "service"),
                ("vehicle", "no"),
                ("bicycle", "yes")
            ]),
            BIKE | WALK
        );
    }

    #[test]
    fn motorroad_and_motorway_exclude_bike_and_walk_row6() {
        let c = classify(&[("highway", "trunk"), ("motorroad", "yes")]).unwrap();
        assert_eq!(c.access, CAR);
        assert_eq!(c.flags & edge_flags::MOTORROAD, edge_flags::MOTORROAD);
        // 4・5の可より優先
        let c = classify(&[
            ("highway", "primary"),
            ("motorroad", "yes"),
            ("bicycle", "yes"),
            ("foot", "yes"),
        ])
        .unwrap();
        assert_eq!(c.access, CAR);
        for h in ["motorway", "motorway_link"] {
            let c =
                classify(&[("highway", h), ("bicycle", "yes"), ("foot", "designated")]).unwrap();
            assert_eq!(c.access, access::CAR_FWD, "{h}");
        }
        // motorroad=no は何も変えない
        assert_eq!(
            access_of(&[("highway", "trunk"), ("motorroad", "no")]),
            CAR | BIKE | WALK
        );
        // motorroad=yes でcarも不可なら、グラフに入らない
        assert_eq!(
            classify(&[
                ("highway", "trunk"),
                ("motorroad", "yes"),
                ("motor_vehicle", "no")
            ]),
            None
        );
    }

    // --- 7.2節の7〜9：一方通行 ---

    #[test]
    fn oneway_for_car_row7() {
        let both = CAR;
        for v in ["yes", "true", "1"] {
            assert_eq!(
                access_of(&[("highway", "residential"), ("oneway", v)]) & CAR,
                access::CAR_FWD,
                "oneway={v}"
            );
        }
        for v in ["-1", "reverse"] {
            assert_eq!(
                access_of(&[("highway", "residential"), ("oneway", v)]) & CAR,
                access::CAR_BWD,
                "oneway={v}"
            );
        }
        for v in ["no", "false", "0"] {
            assert_eq!(
                access_of(&[("highway", "residential"), ("oneway", v)]) & CAR,
                both,
                "oneway={v}"
            );
        }
        // タグがなければ、motorway・motorway_link・環状交差点は始点→終点のみ
        assert_eq!(
            access_of(&[("highway", "primary"), ("junction", "roundabout")]) & CAR,
            access::CAR_FWD
        );
        assert_eq!(
            access_of(&[("highway", "primary"), ("junction", "circular")]) & CAR,
            access::CAR_FWD
        );
        assert_eq!(access_of(&[("highway", "primary")]) & CAR, both);
        // 明示された oneway=no は暗黙の一方通行に優先
        assert_eq!(
            access_of(&[("highway", "motorway"), ("oneway", "no")]) & CAR,
            both
        );
        // reversible/alternating はcarを不可にする（bike・walkは残る）
        for v in ["reversible", "alternating"] {
            assert_eq!(
                access_of(&[("highway", "primary"), ("oneway", v)]),
                BIKE | WALK,
                "oneway={v}"
            );
        }
        // 解釈できない値は、暗黙の規則に従う
        assert_eq!(
            access_of(&[("highway", "primary"), ("oneway", "maybe")]) & CAR,
            both
        );
    }

    #[test]
    fn oneway_for_bike_row8() {
        // carの一方通行に従う
        let c = classify(&[("highway", "residential"), ("oneway", "yes")]).unwrap();
        assert_eq!(c.access & BIKE, access::BIKE_FWD);
        let c = classify(&[("highway", "residential"), ("oneway", "-1")]).unwrap();
        assert_eq!(c.access & BIKE, access::BIKE_BWD);
        // 一方通行のcyclewayもbikeは従う
        let c = classify(&[("highway", "cycleway"), ("oneway", "yes")]).unwrap();
        assert_eq!(c.access & BIKE, access::BIKE_FWD);
        // oneway:bicycle=no、cycleway=opposite*、cycleway:left/right=opposite* があれば両方向
        for tags in [
            vec![("oneway:bicycle", "no")],
            vec![("cycleway", "opposite")],
            vec![("cycleway", "opposite_lane")],
            vec![("cycleway", "opposite_track")],
            vec![("cycleway:left", "opposite_lane")],
            vec![("cycleway:right", "opposite")],
        ] {
            let mut all = vec![("highway", "residential"), ("oneway", "yes")];
            all.extend(tags.iter().copied());
            let c = classify(&all).unwrap();
            assert_eq!(c.access & BIKE, BIKE, "{tags:?}");
            assert_eq!(c.access & CAR, access::CAR_FWD, "{tags:?}");
        }
        // oneway:bicycle=yes/-1 はそれに従う（carが両方向でも）
        let c = classify(&[("highway", "residential"), ("oneway:bicycle", "yes")]).unwrap();
        assert_eq!((c.access & BIKE, c.access & CAR), (access::BIKE_FWD, CAR));
        let c = classify(&[("highway", "residential"), ("oneway:bicycle", "-1")]).unwrap();
        assert_eq!(c.access & BIKE, access::BIKE_BWD);
        // oneway:bicycle=yes は cycleway=opposite より後に適用される
        let c = classify(&[
            ("highway", "residential"),
            ("oneway", "yes"),
            ("cycleway", "opposite"),
            ("oneway:bicycle", "yes"),
        ])
        .unwrap();
        assert_eq!(c.access & BIKE, access::BIKE_FWD);
        // 環状交差点のbikeもcarに従う
        let c = classify(&[("highway", "tertiary"), ("junction", "roundabout")]).unwrap();
        assert_eq!(c.access & BIKE, access::BIKE_FWD);
    }

    #[test]
    fn walk_ignores_oneway_row9() {
        let c = classify(&[("highway", "residential"), ("oneway", "yes")]).unwrap();
        assert_eq!(c.access & WALK, WALK);
        let c = classify(&[("highway", "primary"), ("junction", "roundabout")]).unwrap();
        assert_eq!(c.access & WALK, WALK);
        // oneway:foot=yes/-1 のときだけ従う
        let c = classify(&[("highway", "footway"), ("oneway:foot", "yes")]).unwrap();
        assert_eq!(c.access, access::WALK_FWD);
        let c = classify(&[("highway", "footway"), ("oneway:foot", "-1")]).unwrap();
        assert_eq!(c.access, access::WALK_BWD);
        let c = classify(&[("highway", "footway"), ("oneway:foot", "no")]).unwrap();
        assert_eq!(c.access, WALK);
        // 一方通行のmotorwayは歩行者不可
        assert_eq!(
            access_of(&[("highway", "motorway"), ("oneway:foot", "no")]),
            access::CAR_FWD
        );
    }

    // --- flags・maxspeed ---

    #[test]
    fn flags_follow_the_table() {
        let c = classify(&[("highway", "footway"), ("footway", "sidewalk")]).unwrap();
        assert_eq!(c.flags, edge_flags::SIDEWALK);
        let c = classify(&[("highway", "footway"), ("footway", "crossing")]).unwrap();
        assert_eq!(c.flags, edge_flags::CROSSING);
        let c = classify(&[("highway", "footway"), ("crossing", "zebra")]).unwrap();
        assert_eq!(c.flags, edge_flags::CROSSING);
        // crossing=* は highway=footway のときだけ（道路のwayのcrossingは対象外）
        let c = classify(&[("highway", "residential"), ("crossing", "zebra")]).unwrap();
        assert_eq!(c.flags, 0);
        let c = classify(&[("highway", "footway"), ("crossing", "no")]).unwrap();
        assert_eq!(c.flags, 0);
        for v in ["yes", "building_passage"] {
            let c = classify(&[("highway", "residential"), ("tunnel", v)]).unwrap();
            assert_eq!(c.flags, edge_flags::TUNNEL, "tunnel={v}");
        }
        let c = classify(&[("highway", "residential"), ("tunnel", "no")]).unwrap();
        assert_eq!(c.flags, 0);
        for v in ["yes", "viaduct"] {
            let c = classify(&[("highway", "residential"), ("bridge", v)]).unwrap();
            assert_eq!(c.flags, edge_flags::BRIDGE, "bridge={v}");
        }
        let c = classify(&[("highway", "primary"), ("junction", "roundabout")]).unwrap();
        assert_eq!(c.flags, edge_flags::ROUNDABOUT);
        let c = classify(&[("highway", "motorway"), ("toll", "yes")]).unwrap();
        assert_eq!(c.flags, edge_flags::TOLL);
        let c = classify(&[
            ("highway", "primary"),
            ("bridge", "yes"),
            ("tunnel", "yes"),
            ("toll", "yes"),
            ("motorroad", "yes"),
        ])
        .unwrap();
        assert_eq!(
            c.flags,
            edge_flags::BRIDGE | edge_flags::TUNNEL | edge_flags::TOLL | edge_flags::MOTORROAD
        );
    }

    #[test]
    fn maxspeed_is_attached_to_the_classification() {
        let c = classify(&[("highway", "residential"), ("maxspeed", "30")]).unwrap();
        assert_eq!(c.maxspeed_kmh, 30);
        let c = classify(&[("highway", "residential"), ("maxspeed", "JP:urban")]).unwrap();
        assert_eq!(c.maxspeed_kmh, 0);
        let c = classify(&[("highway", "residential")]).unwrap();
        assert_eq!(c.maxspeed_kmh, 0);
    }

    // --- 7.4節：maxspeed ---

    #[test]
    fn parse_maxspeed_table() {
        assert_eq!(parse_maxspeed("40"), 40);
        assert_eq!(parse_maxspeed(" 40 "), 40);
        assert_eq!(parse_maxspeed("40 km/h"), 40);
        assert_eq!(parse_maxspeed("40km/h"), 40);
        assert_eq!(parse_maxspeed("40 kmh"), 40);
        assert_eq!(parse_maxspeed("40 kph"), 40);
        // mph は1.609倍して四捨五入
        assert_eq!(parse_maxspeed("25 mph"), 40);
        assert_eq!(parse_maxspeed("5 mph"), 8);
        assert_eq!(parse_maxspeed("60mph"), 97);
        assert_eq!(parse_maxspeed("30 MPH"), 48);
        // 記号・解釈できない値は0
        for v in [
            "JP:urban",
            "JP:national",
            "none",
            "signals",
            "walk",
            "",
            "abc",
            "40;50",
            "40 knots",
            "-5",
            "0",
            "0 mph",
        ] {
            assert_eq!(parse_maxspeed(v), 0, "{v:?}");
        }
        // 255以上は255
        assert_eq!(parse_maxspeed("255"), 255);
        assert_eq!(parse_maxspeed("300"), 255);
        assert_eq!(parse_maxspeed("200 mph"), 255);
        assert_eq!(parse_maxspeed("254"), 254);
    }

    // --- 7.3節：障害物 ---

    fn barrier(tags: &[(&str, &str)]) -> u8 {
        classify_barrier(tags)
    }

    const BLOCK_ALL: u8 = node_flags::BLOCK_CAR | node_flags::BLOCK_BIKE | node_flags::BLOCK_WALK;

    #[test]
    fn barrier_table() {
        for b in [
            "bollard",
            "block",
            "jersey_barrier",
            "bus_trap",
            "cycle_barrier",
        ] {
            assert_eq!(barrier(&[("barrier", b)]), node_flags::BLOCK_CAR, "{b}");
        }
        for b in ["chain", "lift_gate", "gate", "swing_gate"] {
            assert_eq!(barrier(&[("barrier", b)]), node_flags::BLOCK_CAR, "{b}");
        }
        for b in ["kissing_gate", "stile", "turnstile"] {
            assert_eq!(
                barrier(&[("barrier", b)]),
                node_flags::BLOCK_CAR | node_flags::BLOCK_BIKE,
                "{b}"
            );
        }
        for b in ["fence", "wall"] {
            assert_eq!(barrier(&[("barrier", b)]), BLOCK_ALL, "{b}");
        }
        // 障害物でないものは0
        assert_eq!(barrier(&[("barrier", "kerb")]), 0);
        assert_eq!(barrier(&[("barrier", "toll_booth")]), 0);
        assert_eq!(barrier(&[("highway", "traffic_signals")]), 0);
        assert_eq!(barrier(&[]), 0);
    }

    #[test]
    fn barrier_access_tags_override() {
        // ゲートは access=yes/permissive 等でcarも通れる
        for v in ["yes", "permissive", "destination", "customers"] {
            assert_eq!(
                barrier(&[("barrier", "gate"), ("access", v)]),
                0,
                "gate access={v}"
            );
            assert_eq!(
                barrier(&[("barrier", "lift_gate"), ("access", v)]),
                0,
                "lift_gate access={v}"
            );
        }
        // bollardは access=yes では変わらない（7.2節の1：既定のまま）
        assert_eq!(
            barrier(&[("barrier", "bollard"), ("access", "yes")]),
            node_flags::BLOCK_CAR
        );
        // access=no は全手段を不可にする。後のタグ（bicycle・foot）で戻せる
        assert_eq!(barrier(&[("barrier", "gate"), ("access", "no")]), BLOCK_ALL);
        assert_eq!(
            barrier(&[
                ("barrier", "gate"),
                ("access", "no"),
                ("bicycle", "yes"),
                ("foot", "yes")
            ]),
            node_flags::BLOCK_CAR
        );
        // motor_vehicle=yes/destination・motorcar=yes はcarを通す
        assert_eq!(
            barrier(&[("barrier", "bollard"), ("motor_vehicle", "yes")]),
            0
        );
        assert_eq!(
            barrier(&[("barrier", "block"), ("motorcar", "destination")]),
            0
        );
        // bicycle=no/foot=no はそれぞれを不可にする
        assert_eq!(
            barrier(&[("barrier", "bollard"), ("bicycle", "no")]),
            node_flags::BLOCK_CAR | node_flags::BLOCK_BIKE
        );
        assert_eq!(
            barrier(&[("barrier", "bollard"), ("foot", "no")]),
            node_flags::BLOCK_CAR | node_flags::BLOCK_WALK
        );
        // kissing_gate でも bicycle=yes ならbikeは通れる
        assert_eq!(
            barrier(&[("barrier", "kissing_gate"), ("bicycle", "yes")]),
            node_flags::BLOCK_CAR
        );
        // fence でも foot=yes ならwalkは通れる
        assert_eq!(
            barrier(&[("barrier", "fence"), ("foot", "yes")]),
            node_flags::BLOCK_CAR | node_flags::BLOCK_BIKE
        );
        // vehicle=no はcar・bikeを不可、vehicle=yes は既定の可を戻す
        assert_eq!(
            barrier(&[("barrier", "cycle_barrier"), ("vehicle", "no")]),
            node_flags::BLOCK_CAR | node_flags::BLOCK_BIKE
        );
    }

    #[test]
    fn node_flags_for_signals_crossings_and_stops() {
        assert_eq!(
            classify_node(&[("highway", "traffic_signals")]),
            node_flags::SIGNAL
        );
        assert_eq!(
            classify_node(&[("highway", "crossing"), ("crossing", "traffic_signals")]),
            node_flags::SIGNAL | node_flags::CROSSING
        );
        assert_eq!(
            classify_node(&[("highway", "crossing")]),
            node_flags::CROSSING
        );
        assert_eq!(classify_node(&[("highway", "stop")]), node_flags::STOP);
        for v in ["level_crossing", "crossing"] {
            assert_eq!(
                classify_node(&[("railway", v)]),
                node_flags::LEVEL_CROSSING,
                "{v}"
            );
        }
        assert_eq!(
            classify_node(&[("barrier", "bollard"), ("highway", "crossing")]),
            node_flags::BLOCK_CAR | node_flags::CROSSING
        );
        assert_eq!(classify_node(&[("name", "x")]), 0);
        assert_eq!(classify_node(&[]), 0);
    }
}
