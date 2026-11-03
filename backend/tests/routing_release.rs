//! schema 3の識別子（形式書4.2節）と定数。

use accident_route_backend::data::release::{
    ACCIDENTS_FILE, ALT_FILE, GRAPH_FILE, ROUTING_FILES, ROUTING_FORMAT_VERSIONS, ROUTING_MAGICS,
    SCHEMA_VERSION, SCHEMA_VERSION_ROUTING, release_id, routing_file_spec, routing_release_id,
    sha256_hex, try_routing_release_id,
};

const DATASET: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const HOLIDAY: &str = "2222222222222222222222222222222222222222222222222222222222222222";

/// ファイルごとに異なるハッシュ（見本）。
fn hashes() -> Vec<(&'static str, String)> {
    ROUTING_FILES
        .iter()
        .map(|&file| (file, sha256_hex(file.as_bytes())))
        .collect()
}

fn pairs<'a>(hashes: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    hashes.iter().map(|(f, h)| (*f, h.as_str())).collect()
}

#[test]
fn constants() {
    assert_eq!(SCHEMA_VERSION, 2);
    assert_eq!(SCHEMA_VERSION_ROUTING, 3);
    assert_eq!(
        ROUTING_FILES,
        [
            "graph.bin",
            "restrictions.bin",
            "topology.bin",
            "odindex.bin",
            "accident_attrs.bin",
            "assignment.bin",
            "risk.bin",
            "alt.bin",
        ]
    );
    let magics: Vec<&[u8]> = ROUTING_MAGICS.iter().map(|m| m.as_slice()).collect();
    assert_eq!(
        magics,
        [
            b"TKRGRAPH".as_slice(),
            b"TKRRESTR",
            b"TKRTOPOL",
            b"TKRODIDX",
            b"TKRACATR",
            b"TKRASSGN",
            b"TKRRISK_",
            b"TKRALT__",
        ]
    );
    assert!(ROUTING_FORMAT_VERSIONS.iter().all(|&v| v == 1));
    assert_eq!(routing_file_spec(GRAPH_FILE), Some((*b"TKRGRAPH", 1)));
    assert_eq!(routing_file_spec(ALT_FILE), Some((*b"TKRALT__", 1)));
    assert_eq!(routing_file_spec(ACCIDENTS_FILE), None);
}

#[test]
fn formula_is_newline_joined_with_single_space_between_name_and_hash() {
    let hashes = hashes();
    let mut text = format!("{DATASET}\n{HOLIDAY}");
    for (file, hash) in &hashes {
        text.push_str(&format!("\n{file} {hash}"));
    }
    assert!(!text.ends_with('\n'));
    assert_eq!(
        routing_release_id(DATASET, HOLIDAY, &pairs(&hashes)),
        sha256_hex(text.as_bytes())
    );
    // 小文字hexのSHA-256
    let id = routing_release_id(DATASET, HOLIDAY, &pairs(&hashes));
    assert_eq!(id.len(), 64);
    assert!(
        id.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
}

#[test]
fn first_two_lines_are_the_schema_2_formula() {
    let hashes = hashes();
    let schema2_input = format!("{DATASET}\n{HOLIDAY}");
    assert_eq!(
        release_id(DATASET, HOLIDAY),
        sha256_hex(schema2_input.as_bytes())
    );
    // schema 3 の入力は schema 2 の入力に続けて、ファイルの行が付く
    let schema3_input = format!(
        "{schema2_input}\ngraph.bin {}\nrestrictions.bin {}\ntopology.bin {}\nodindex.bin {}\naccident_attrs.bin {}\nassignment.bin {}\nrisk.bin {}\nalt.bin {}",
        hashes[0].1,
        hashes[1].1,
        hashes[2].1,
        hashes[3].1,
        hashes[4].1,
        hashes[5].1,
        hashes[6].1,
        hashes[7].1,
    );
    assert_eq!(
        routing_release_id(DATASET, HOLIDAY, &pairs(&hashes)),
        sha256_hex(schema3_input.as_bytes())
    );
    // 両者は別の値になる（schema 2のreleaseと混同しない）
    assert_ne!(
        routing_release_id(DATASET, HOLIDAY, &pairs(&hashes)),
        release_id(DATASET, HOLIDAY)
    );
}

#[test]
fn schema_2_release_id_is_unchanged() {
    // 既知の値：sha256("<dataset>\n<holiday>")
    let expected = sha256_hex(format!("{DATASET}\n{HOLIDAY}").as_bytes());
    assert_eq!(release_id(DATASET, HOLIDAY), expected);
}

#[test]
fn order_of_the_input_does_not_matter_but_the_fixed_order_does() {
    let hashes = hashes();
    let id = routing_release_id(DATASET, HOLIDAY, &pairs(&hashes));
    // 入力の並びを変えても、行の順番は ROUTING_FILES の固定順
    let mut reversed = pairs(&hashes);
    reversed.reverse();
    assert_eq!(routing_release_id(DATASET, HOLIDAY, &reversed), id);

    // 固定順を入れ替えた式は別の値になる（ファイル順が式に効いている）
    let mut swapped = hashes.clone();
    swapped.swap(0, 1);
    let mut text = format!("{DATASET}\n{HOLIDAY}");
    for (file, hash) in &swapped {
        text.push_str(&format!("\n{file} {hash}"));
    }
    assert_ne!(sha256_hex(text.as_bytes()), id);
}

#[test]
fn every_input_changes_the_id() {
    let hashes = hashes();
    let base = routing_release_id(DATASET, HOLIDAY, &pairs(&hashes));
    assert_ne!(
        routing_release_id(&DATASET.replace('1', "3"), HOLIDAY, &pairs(&hashes)),
        base
    );
    assert_ne!(
        routing_release_id(DATASET, &HOLIDAY.replace('2', "3"), &pairs(&hashes)),
        base
    );
    for i in 0..ROUTING_FILES.len() {
        let mut changed = hashes.clone();
        changed[i].1 = sha256_hex(b"changed");
        assert_ne!(
            routing_release_id(DATASET, HOLIDAY, &pairs(&changed)),
            base,
            "{}",
            ROUTING_FILES[i]
        );
    }
}

#[test]
fn missing_duplicate_or_unknown_files_are_rejected() {
    let hashes = hashes();
    let full = pairs(&hashes);
    assert!(try_routing_release_id(DATASET, HOLIDAY, &full).is_some());

    // 欠け
    assert!(try_routing_release_id(DATASET, HOLIDAY, &full[1..]).is_none());
    assert!(try_routing_release_id(DATASET, HOLIDAY, &[]).is_none());
    // 重複（件数は8のまま）
    let mut duplicated = full.clone();
    duplicated[7] = duplicated[0];
    assert!(try_routing_release_id(DATASET, HOLIDAY, &duplicated).is_none());
    // 知らない名前
    let mut unknown = full.clone();
    unknown[7] = ("other.bin", "00");
    assert!(try_routing_release_id(DATASET, HOLIDAY, &unknown).is_none());
    // 9個
    let mut extra = full;
    extra.push((ACCIDENTS_FILE, "00"));
    assert!(try_routing_release_id(DATASET, HOLIDAY, &extra).is_none());

    assert!(
        std::panic::catch_unwind(|| routing_release_id(DATASET, HOLIDAY, &[(GRAPH_FILE, "00")]))
            .is_err()
    );
}

// --- load_engine が schema_version でエンジンを選ぶ分岐（形式書4.3節） ---

use accident_route_backend::data::{self, LoadError};

fn release_dir_with_metadata(metadata: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("metadata.json"), metadata).unwrap();
    dir
}

fn expect_invalid(dir: &std::path::Path) -> String {
    match data::load_engine(dir) {
        Err(LoadError::Invalid(message)) => message,
        Err(other) => panic!("Invalid以外の失敗: {other}"),
        Ok(_) => panic!("読めてはいけないreleaseを受け付けた"),
    }
}

#[test]
fn load_engine_rejects_other_schema_versions() {
    for metadata in [
        r#"{"schema_version": 1}"#,
        r#"{"schema_version": 0}"#,
        r#"{"schema_version": 4}"#,
        r#"{"schema_version": "3"}"#,
        r#"{"schema_version": 3.5}"#,
        r#"{"schema_version": null}"#,
        r#"{}"#,
    ] {
        let dir = release_dir_with_metadata(metadata);
        assert!(
            expect_invalid(dir.path()).contains("schema_version"),
            "{metadata}"
        );
    }
    // JSONでない・metadata.jsonがない
    let dir = release_dir_with_metadata("not json");
    assert!(expect_invalid(dir.path()).contains("JSON"));
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        data::load_engine(dir.path()),
        Err(LoadError::Io(_))
    ));
}

#[tokio::test]
async fn load_engine_routes_schema_3_to_the_routing_loader() {
    // M0.6の人工release：schema 3を実際に読み、候補が空の検索から応答まで通す。
    use accident_route_backend::api::route_response::{check_detour, render_route_response};
    use accident_route_backend::contract::{CancelToken, ConditionFilter, RouteQuery};
    use accident_route_backend::testing::ScriptedRouter;
    use accident_route_backend::testing::TestRelease;
    use accident_route_backend::testing::fixtures::check_outcome_invariants;

    let (release, query) = TestRelease::routing();
    let engine = data::load_engine(release.path()).expect("schema 3の人工releaseを読める");
    assert_eq!(engine.dataset().release_id, release.ids.release_id);
    assert_eq!(engine.dataset().dataset_id, release.ids.dataset_id);
    assert_eq!(engine.dataset().accidents, 3);
    assert_eq!(engine.dataset().data_years, vec![2025]);

    let request = RouteQuery::new(
        query.origin,
        query.dest,
        query.mode,
        10.0,
        ConditionFilter::default(),
    )
    .expect("人工releaseの座標は妥当");
    let router = ScriptedRouter::new(vec![]);
    let candidates = engine
        .collect_candidates(&router, &request, &CancelToken::unbounded())
        .await
        .expect("候補取得");
    assert!(candidates.routes.is_empty());
    assert!(candidates.warnings.is_empty());

    let outcome = engine
        .evaluate(&request, candidates, &CancelToken::unbounded())
        .expect("経路を評価できる");
    check_outcome_invariants(&outcome).expect("相関制約");
    check_detour(&outcome, 10.0).expect("遠回りの上限");
    render_route_response(&outcome).expect("応答JSONにできる");
    assert!(
        outcome
            .candidates
            .iter()
            .any(|candidate| candidate.totals.accident_count > 0),
        "人工releaseの候補に事故が載る"
    );
}

#[test]
fn load_engine_still_loads_schema_2() {
    use accident_route_backend::testing::TestRelease;
    let release = TestRelease::for_example("twoSelected");
    let engine = data::load_engine(release.path()).unwrap();
    assert_eq!(engine.dataset().accidents, 3);
}
