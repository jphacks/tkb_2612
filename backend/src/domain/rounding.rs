//! 小数桁の丸め（behavior.md 8章）。

/// Pythonの `round(x, digits)` と同じ丸め。
///
/// 二進浮動小数の値そのものを10進の `digits` 桁に丸め、ちょうど半分のときだけ偶数側にする。
/// Rustの `f64::round`（0から遠い側）とは異なる。fingerprint 5桁、CSV 7桁、
/// `weighted_risk` 2桁、`risk_per_km`・追加分3桁、削減率1桁、`aggregation_ms` 2桁のすべてで使う。
pub fn py_round(x: f64, digits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    // 固定桁の書式化は二進値を正確に10進へ丸める（同点は偶数側）。Pythonも同じ規則で丸める。
    format!("{x:.digits$}")
        .parse()
        .expect("固定桁で書式化した有限値は再び読める")
}

/// CSV等へ書く固定桁の文字列（`py_round` と同じ丸め）。
pub fn fixed(x: f64, digits: usize) -> String {
    format!("{x:.digits$}")
}
