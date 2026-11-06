//! M3b.3：P8 `routing_prep alt` が合成グラフから `alt.bin` と監査JSONを作ることを確かめる。
//!
//! 小さな格子を `graph.bin` に書き、`routing_prep alt` を実行して、出力をデコードする。
//! (a) 手段ごとのランドマークの数、(b) すべてのノード対で距離表の下界が「辺の時間だけ」の
//! 最短時間以下であること、(c) 監査JSONがあること、を検査する。

use std::fs;
use std::process::Command;

use accident_route_backend::contract::Mode;
use accident_route_backend::data::release::{GRAPH_FILE, sha256_hex};
use accident_route_backend::data::routing_files::{decode_alt, decode_graph, encode_graph};
use accident_route_backend::domain::routing::graph::synthetic::grid;
use accident_route_backend::domain::routing::search::alt::edge_only_distances;
use serde_json::Value;

fn run(args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_routing_prep"))
        .args(args)
        .output()
        .expect("routing_prepを起動できる");
    assert!(
        output.status.success(),
        "routing_prep {args:?} が失敗しました\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn prep_alt_writes_tables_and_audit_for_a_synthetic_graph() {
    let tmp = tempfile::tempdir().unwrap();
    let (builder, _shape) = grid(3, 4, 150.0);
    let graph = builder.build();
    let graph_bytes = encode_graph(&graph);
    let graph_path = tmp.path().join(GRAPH_FILE);
    fs::write(&graph_path, &graph_bytes).unwrap();
    let output = tmp.path().join("alt.bin");
    let (graph_s, output_s) = (
        graph_path.to_str().unwrap(),
        output.to_str().unwrap().to_owned(),
    );

    run(&[
        "alt",
        "--graph",
        graph_s,
        "--output",
        &output_s,
        "--modes",
        "car,bike,walk",
        "--landmarks",
        "4",
        "--unit-ds",
        "20",
        "--with-to",
        "true",
    ]);

    let bytes = fs::read(&output).unwrap();
    let decoded = decode_alt(&bytes).unwrap();
    assert_eq!(decoded.deps[GRAPH_FILE], sha256_hex(&graph_bytes));
    assert_eq!(decoded.value.params.unit_ds, 20);

    // (a) ランドマークの数。12ノードの格子なので、要求した4個が選ばれる。
    for mode in Mode::ALL {
        let table = decoded
            .value
            .table(mode)
            .unwrap_or_else(|| panic!("{mode:?}の表がない"));
        assert_eq!(table.count(), 4, "{mode:?}");
        assert_eq!(table.from.len(), graph.node_count() * 4, "{mode:?}");
        assert_eq!(
            table.to.as_ref().map(Vec::len),
            Some(graph.node_count() * 4),
            "{mode:?}"
        );
    }

    // (b) すべてのノード対で、下界が辺の時間だけの最短時間以下。
    for mode in Mode::ALL {
        for t in 0..graph.node_count() as u32 {
            let truth = edge_only_distances(&graph, mode, t, true);
            for v in 0..graph.node_count() as u32 {
                let lower = decoded.value.lower_bound_ds(&graph, mode, v, t);
                assert!(
                    lower <= truth[v as usize],
                    "{mode:?} v={v} t={t}: 下界{lower} > 真の値{}",
                    truth[v as usize]
                );
            }
        }
    }

    // (c) 監査JSON。手段ごとのランドマークのノード番号・座標・到達できたノードの数がある。
    let audit_path = output.with_extension("audit.json");
    assert!(audit_path.exists(), "{} がない", audit_path.display());
    let audit: Value = serde_json::from_slice(&fs::read(&audit_path).unwrap()).unwrap();
    assert_eq!(audit["command"], "alt");
    assert_eq!(audit["sha256"], sha256_hex(&bytes));
    for mode in Mode::ALL {
        let key = mode.as_str();
        let mode_audit = &audit["modes"][key];
        assert_eq!(mode_audit["count"], 4, "{key}");
        assert_eq!(mode_audit["start_node"], 0, "{key}");
        let landmarks = mode_audit["landmarks"].as_array().unwrap();
        assert_eq!(landmarks.len(), 4, "{key}");
        for landmark in landmarks {
            let node = landmark["node"].as_u64().unwrap();
            let coord = graph.node_coord(node as u32);
            assert_eq!(landmark["lat_e7"], coord.lat_e7, "{key}");
            assert_eq!(landmark["lon_e7"], coord.lon_e7, "{key}");
            assert!(landmark["reached_nodes"].as_u64().unwrap() > 0, "{key}");
        }
    }

    // 出力を `graph.bin` として読み直しても、元のグラフと同じ（入力の版の確認）。
    assert_eq!(
        decode_graph(&graph_bytes).unwrap().value.parts(),
        graph.parts()
    );
}

#[test]
fn prep_alt_defaults_to_car_without_to() {
    let tmp = tempfile::tempdir().unwrap();
    let (builder, _shape) = grid(1, 4, 200.0);
    let graph = builder.build();
    let graph_bytes = encode_graph(&graph);
    let graph_path = tmp.path().join(GRAPH_FILE);
    fs::write(&graph_path, &graph_bytes).unwrap();
    let output = tmp.path().join("alt.bin");

    run(&[
        "alt",
        "--graph",
        graph_path.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--landmarks",
        "2",
    ]);

    let decoded = decode_alt(&fs::read(&output).unwrap()).unwrap();
    let car = decoded.value.table(Mode::Car).expect("carの表がない");
    assert_eq!(car.count(), 2);
    assert!(car.to.is_none(), "既定ではtoを作らない");
    assert!(decoded.value.table(Mode::Bike).is_none());
    assert!(decoded.value.table(Mode::Walk).is_none());
    assert!(output.with_extension("audit.json").exists());
}
