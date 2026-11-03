//! `routing_eval engine-batch`・`engine-summary` の結合試験（作業書M5.5・M5.8）。
//!
//! `write_routing_test_release` が作る schema 3の人工releaseを一時ディレクトリに書き、
//! 実際のコマンドを起動して、応答の値と計測値の記録を確かめる。
//!
//! - 全件が `ok`・`invariants_ok=true`。
//! - 候補ごとに `t3` の区間数が応答の `high_risk_sections` と一致し、`t3` のsid数が
//!   `high_risk_total` と一致する。
//! - 同じ候補で `t2`（2件以上かつ深刻度3以上）が `t3`（3件以上）以上になる。
//! - `--out` の `.meta.json` に release_id・読込の秒数・処理の秒数・VmHWM・スレッド数がある。
//! - `engine-summary` がグループごとの件数と状態の内訳を出す。

use std::fs;
use std::path::Path;
use std::process::Command;

use accident_route_backend::testing::write_routing_test_release;
use serde_json::Value;

fn eval(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_routing_eval"))
        .args(args)
        .output()
        .expect("routing_evalを起動できる")
}

fn eval_ok(args: &[&str]) {
    let out = eval(args);
    assert!(
        out.status.success(),
        "routing_eval {args:?} が失敗しました\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn read_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("1行が1つのJSON"))
        .collect()
}

#[test]
fn engine_batch_reports_invariants_and_both_thresholds() {
    let dir = tempfile::tempdir().unwrap();
    let release = write_routing_test_release(dir.path()).unwrap();

    // 同じ組を3件（region は id の接頭辞）。条件はすべて指定なしにする。
    let ods: Vec<String> = (1..=3)
        .map(|i| {
            format!(
                r#"{{"id":"chiba-car-{i}","mode":"{}","o":{{"lat":{},"lon":{}}},"d":{{"lat":{},"lon":{}}}}}"#,
                release.mode.as_str(),
                release.origin.lat,
                release.origin.lon,
                release.dest.lat,
                release.dest.lon,
            )
        })
        .collect();
    let ods_path = dir.path().join("ods.jsonl");
    fs::write(&ods_path, ods.join("\n") + "\n").unwrap();
    let out_path = dir.path().join("e60.jsonl");
    eval_ok(&[
        "engine-batch",
        "--release",
        release.release_dir.to_str().unwrap(),
        "--ods",
        ods_path.to_str().unwrap(),
        "--out",
        out_path.to_str().unwrap(),
        "--conditions",
        "all",
        "--threads",
        "2",
    ]);

    let records = read_lines(&out_path);
    assert_eq!(records.len(), 3, "入力と同じ件数・同じ順序");
    for record in &records {
        assert_eq!(record["status"], "ok", "{record}");
        assert_eq!(record["invariants_ok"], true, "{record}");
        assert_eq!(record["detour_min"], 10);
        assert!(record["total_ms"].as_f64().unwrap() >= 0.0);
        assert!(record["aggregation_ms"].as_f64().unwrap() >= 0.0);
        assert!(record.get("t3_mismatch").is_none(), "{record}");
        let candidates = record["candidates"].as_array().unwrap();
        assert!(!candidates.is_empty());
        for candidate in candidates {
            assert_eq!(
                candidate["t3_sections"].as_u64().unwrap(),
                candidate["high_risk_sections"].as_u64().unwrap(),
                "t3の区間数が応答と一致する: {candidate}"
            );
            assert_eq!(
                candidate["t3_sids"].as_u64().unwrap(),
                candidate["high_risk_total"].as_u64().unwrap(),
                "t3のsid数が応答と一致する: {candidate}"
            );
            assert!(
                candidate["t2_sids"].as_u64().unwrap() >= candidate["t3_sids"].as_u64().unwrap(),
                "t2のsid数がt3以上: {candidate}"
            );
            assert!(
                candidate["t2_sections"].as_u64().unwrap()
                    >= candidate["t3_sections"].as_u64().unwrap(),
                "t2の区間数がt3以上: {candidate}"
            );
            assert!(
                candidate["full_vertex_count"].as_u64().unwrap()
                    >= candidate["vertex_count"].as_u64().unwrap()
            );
            assert!(candidate["accident_point_count"].as_u64().unwrap() <= 250);
        }
    }

    let meta: Value =
        serde_json::from_slice(&fs::read(out_path.with_extension("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["release_id"], release.release_id);
    assert_eq!(meta["threads"], 2);
    assert_eq!(meta["records"], 3);
    assert!(meta["load_seconds"].as_f64().unwrap() >= 0.0);
    assert!(meta["process_seconds"].as_f64().unwrap() >= 0.0);
    assert!(meta["vm_hwm_mb"].as_f64().unwrap() > 0.0);

    let summary_path = dir.path().join("summary.json");
    eval_ok(&[
        "engine-summary",
        "--results",
        out_path.to_str().unwrap(),
        "--group-by",
        "mode,region",
        "--out",
        summary_path.to_str().unwrap(),
    ]);
    let summary: Value = serde_json::from_slice(&fs::read(&summary_path).unwrap()).unwrap();
    assert_eq!(summary["records"], 3);
    let groups = summary["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    let group = &groups[0];
    assert_eq!(group["mode"], release.mode.as_str());
    assert_eq!(group["region"], "chiba");
    assert_eq!(group["count"], 3);
    assert_eq!(group["status_counts"]["ok"], 3);
    assert_eq!(group["invariants_false"], 0);
    assert_eq!(group["t3_mismatch_count"], 0);
    assert_eq!(group["selected_sections"]["count"], 3);
    assert!(group["total_ms"]["median"].as_f64().unwrap() >= 0.0);
    assert!(
        group["candidate_count_distribution"]["1"]
            .as_u64()
            .is_some()
    );
}
