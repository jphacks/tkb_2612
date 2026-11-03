//! releaseの起動時ロードと検証（behavior.md 3章）。
//!
//! gzipと祝日表の実ファイルから `dataset_id`・`release_id` を再計算してmetadataと照合し、
//! schema_version・件数・列挙値を再検証する。メッセージに位置や原本行を含めない。
//! schema 3では8つの `.bin` のファイル全体のSHA-256・`DEPS`・節の件数を確かめ、
//! `graph.bin`・`alt.bin` はファイル全体を `Vec` に置かず、節ごとに型の `Vec` へ読み込む。

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::NaiveDate;
use flate2::read::GzDecoder;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::contract::geo::{LAT_RANGE, LON_RANGE};
use crate::contract::{DatasetInfo, DayType, Mode, Period, RouteEngine, Weather};
use crate::domain::routing::{EdgeAccidentIndex, RoutingData, RoutingEngine, UnassignedGrid};
use crate::domain::{AccidentColumns, AccidentSnapshot, HolidayCalendar};

use super::LoadError;
use super::binfile::BinFileReader;
use super::holidays::parse_holiday_csv;
use super::preprocess::rows::OUTPUT_COLUMNS;
use super::release::{
    ACCIDENTS_FILE, HOLIDAYS_FILE, METADATA_FILE, ROUTING_FILES, SCHEMA_VERSION,
    SCHEMA_VERSION_ROUTING, release_id, routing_file_spec, sha256_hex, try_routing_release_id,
};
use super::routing_files::{
    decode_accident_attrs, decode_alt_from_file, decode_assignment, decode_graph_from_file,
    decode_odindex, decode_restrictions, decode_risk, decode_topology, required_deps,
};

fn invalid(message: impl Into<String>) -> LoadError {
    LoadError::Invalid(message.into())
}

/// ファイル名を添えた検証エラー。I/Oの失敗はそのまま返す。
fn with_file(file: &str, error: LoadError) -> LoadError {
    match error {
        LoadError::Invalid(message) => LoadError::Invalid(format!("{file}: {message}")),
        other => other,
    }
}

/// metadata.jsonの `schema_version` だけを読む（`load_engine` がエンジンを選ぶために使う）。
///
/// JSONでない、または `schema_version` が整数でない場合は `LoadError::Invalid`。
/// 値の検査（2か3か）は呼び出し側が行う。
pub fn read_schema_version(release_dir: &Path) -> Result<u64, LoadError> {
    let metadata = read_metadata_value(release_dir)?;
    metadata
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("schema_versionが整数ではありません"))
}

fn read_metadata_value(release_dir: &Path) -> Result<Value, LoadError> {
    let bytes = fs::read(release_dir.join(METADATA_FILE))?;
    serde_json::from_slice(&bytes).map_err(|_| invalid("metadata.jsonがJSONではありません"))
}

/// schema 3のreleaseを読み、形式書4.3節の検証をしてから `RoutingEngine` を作る。
///
/// schema 2と同じ検証（祝日表・`dataset_id`・事故CSVの全件解析・`rows`）に加えて、
/// 8つの `.bin` の有無・ファイル全体のSHA-256・`routing_release_id`・マジックナンバー・
/// `format_version`・節の `count`・`DEPS` を確かめ、各 `decode_*` と型の検査を行う。
pub fn load_routing_engine(release_dir: &Path) -> Result<Arc<dyn RouteEngine>, LoadError> {
    Ok(load_routing_engine_typed(release_dir)?)
}

/// [`load_routing_engine`] と同じ検証・読込を行い、`RoutingEngine` の型のまま返す。
///
/// 応答に出さない計測用の値を読む評価コマンド（`routing_eval engine-batch`）が使う。
pub fn load_routing_engine_typed(release_dir: &Path) -> Result<Arc<RoutingEngine>, LoadError> {
    let started = Instant::now();
    let metadata = read_metadata_routing(release_dir)?;
    let files = routing_files_metadata(&metadata)?;
    let inputs = read_release_inputs(release_dir, &metadata)?;

    // 8ファイルのハッシュは固定長のバッファで逐次に計算する（ファイル全体を保持しない）。
    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    hashes.insert(ACCIDENTS_FILE.to_owned(), inputs.dataset_id.clone());
    hashes.insert(HOLIDAYS_FILE.to_owned(), inputs.holiday_sha256.clone());
    for &file in &ROUTING_FILES {
        let expected = files
            .get(file)
            .ok_or_else(|| invalid(format!("metadataのrouting.filesに{file}がありません")))?;
        let path = release_dir.join(file);
        let length = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(_) => return Err(invalid(format!("{file}がありません"))),
        };
        if length != expected.bytes {
            return Err(invalid(format!("{file}のバイト数がmetadataと一致しません")));
        }
        let hash = sha256_file(&path)?;
        if hash != expected.sha256 {
            return Err(invalid(format!("{file}のSHA-256がmetadataと一致しません")));
        }
        hashes.insert(file.to_owned(), hash);
    }

    let pairs: Vec<(&str, &str)> = ROUTING_FILES
        .iter()
        .map(|&file| {
            (
                file,
                hashes
                    .get(file)
                    .map(String::as_str)
                    .expect("8ファイルのハッシュを計算した"),
            )
        })
        .collect();
    let expected_release_id =
        try_routing_release_id(&inputs.dataset_id, &inputs.holiday_sha256, &pairs)
            .ok_or_else(|| invalid("8つの.binのハッシュが揃っていません"))?;
    if metadata_str(&metadata, &["release_id"])? != expected_release_id {
        return Err(invalid("release_idが実ファイルと一致しません"));
    }

    // ここからschema 2と同じ事故CSVの検証。解析し終えたらgzipは保持しない。
    let dataset_id = inputs.dataset_id.clone();
    let (snapshot, data_years, rows) = snapshot_from_inputs(inputs)?;
    if metadata.get("rows").and_then(Value::as_u64) != Some(rows) {
        return Err(invalid("事故の件数がmetadataと一致しません"));
    }
    let metadata_years: Vec<u64> = metadata
        .get("data_years")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("metadataのdata_yearsが配列ではありません"))?
        .iter()
        .map(|year| {
            year.as_u64()
                .ok_or_else(|| invalid("metadataのdata_yearsが整数ではありません"))
        })
        .collect::<Result<_, _>>()?;
    if metadata_years != data_years.iter().map(|&y| u64::from(y)).collect::<Vec<_>>() {
        return Err(invalid("data_yearsがmetadataと一致しません"));
    }

    // 各ファイルの容器・metadataのマジックナンバー・節の件数・DEPSを先に確かめる。
    for &file in &ROUTING_FILES {
        let expected = files
            .get(file)
            .ok_or_else(|| invalid(format!("metadataのrouting.filesに{file}がありません")))?;
        check_bin_file(release_dir, file, expected, &hashes)?;
    }

    // 大きいファイルから順に、節ごとに型の `Vec` へ読む。
    let graph = decode_graph_from_file(&release_dir.join(super::release::GRAPH_FILE))
        .map_err(|e| with_file(super::release::GRAPH_FILE, e))?
        .value;
    let landmarks = decode_alt_from_file(&release_dir.join(super::release::ALT_FILE))
        .map_err(|e| with_file(super::release::ALT_FILE, e))?
        .value;
    check_alt(&landmarks, &graph)?;

    let restrictions =
        decode_small_file(release_dir, super::release::RESTRICTIONS_FILE, |bytes| {
            decode_restrictions(bytes).map(|d| d.value)
        })?;
    restrictions
        .validate(&graph)
        .map_err(|e| with_file(super::release::RESTRICTIONS_FILE, invalid(e.to_string())))?;

    let topology = decode_small_file(release_dir, super::release::TOPOLOGY_FILE, |bytes| {
        decode_topology(bytes).map(|d| d.value)
    })?;
    topology
        .validate(&graph)
        .map_err(|e| with_file(super::release::TOPOLOGY_FILE, invalid(e.to_string())))?;
    let cluster_count = topology.cluster_count();

    let odindex = decode_small_file(release_dir, super::release::ODINDEX_FILE, |bytes| {
        decode_odindex(bytes).map(|d| d.value)
    })?;
    check_odindex(&odindex, graph.edge_count())?;

    let accident_attrs =
        decode_small_file(release_dir, super::release::ACCIDENT_ATTRS_FILE, |bytes| {
            decode_accident_attrs(bytes).map(|d| d.value)
        })?;
    if accident_attrs.attrs.len() as u64 != rows {
        return Err(with_file(
            super::release::ACCIDENT_ATTRS_FILE,
            invalid(format!(
                "事故数{}がmetadataのrows {rows}と一致しません",
                accident_attrs.attrs.len()
            )),
        ));
    }
    drop(accident_attrs);

    let assignment = decode_small_file(release_dir, super::release::ASSIGNMENT_FILE, |bytes| {
        decode_assignment(bytes).map(|d| d.value)
    })?;
    if assignment.accident_count() as u64 != rows {
        return Err(with_file(
            super::release::ASSIGNMENT_FILE,
            invalid(format!(
                "事故数{}がmetadataのrows {rows}と一致しません",
                assignment.accident_count()
            )),
        ));
    }
    assignment
        .validate(graph.edge_count(), cluster_count)
        .map_err(|e| with_file(super::release::ASSIGNMENT_FILE, invalid(e)))?;
    let accidents = EdgeAccidentIndex::build(&assignment, graph.edge_count(), cluster_count);
    let unassigned = UnassignedGrid::build(&assignment, &snapshot);
    drop(assignment);

    let risk = decode_small_file(release_dir, super::release::RISK_FILE, |bytes| {
        decode_risk(bytes).map(|d| d.value)
    })?;
    if risk.edge_k.len() != graph.edge_count() {
        return Err(with_file(
            super::release::RISK_FILE,
            invalid(format!(
                "EDGEKの件数{}がgraph.binの辺数{}と一致しません",
                risk.edge_k.len(),
                graph.edge_count()
            )),
        ));
    }
    if risk.cluster_j.len() != cluster_count {
        return Err(with_file(
            super::release::RISK_FILE,
            invalid(format!(
                "CLUSJの件数{}がtopology.binのクラスタ数{cluster_count}と一致しません",
                risk.cluster_j.len()
            )),
        ));
    }

    let data = RoutingData {
        graph,
        restrictions,
        topology,
        odindex,
        accidents,
        unassigned,
        risk,
        landmarks,
    };
    let dataset = DatasetInfo {
        release_id: expected_release_id,
        dataset_id,
        accidents: snapshot.len() as u64,
        data_years,
        load_seconds: started.elapsed().as_secs_f64(),
        grid_cells: Some(snapshot.grid_cells()),
    };
    Ok(Arc::new(RoutingEngine::new(
        Arc::new(data),
        Arc::new(snapshot),
        dataset,
    )))
}

/// ファイル全体のSHA-256（小文字hex）。固定長のバッファで逐次に読む。
fn sha256_file(path: &Path) -> Result<String, LoadError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// metadata.jsonを読み、schema_versionを確かめる（schema 2）。
pub fn read_metadata(release_dir: &Path) -> Result<Value, LoadError> {
    let metadata = read_metadata_value(release_dir)?;
    if metadata.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(invalid("schema_versionが2ではありません"));
    }
    Ok(metadata)
}

/// metadata.jsonを読み、schema_versionが3であることを確かめる。
fn read_metadata_routing(release_dir: &Path) -> Result<Value, LoadError> {
    let metadata = read_metadata_value(release_dir)?;
    if metadata.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION_ROUTING) {
        return Err(invalid("schema_versionが3ではありません"));
    }
    Ok(metadata)
}

fn metadata_str<'a>(metadata: &'a Value, path: &[&str]) -> Result<&'a str, LoadError> {
    let mut v = metadata;
    for key in path {
        v = v
            .get(key)
            .ok_or_else(|| invalid(format!("metadataに{}がありません", path.join("."))))?;
    }
    v.as_str().ok_or_else(|| {
        invalid(format!(
            "metadataの{}が文字列ではありません",
            path.join(".")
        ))
    })
}

/// `routing.files[]` の1ファイル分（形式書4.1節）。
struct RoutingFileMetadata {
    sha256: String,
    bytes: u64,
    magic: String,
    format_version: u32,
    sections: BTreeMap<String, u64>,
}

/// `metadata.routing.files[]` を読み、8ファイルがちょうど1つずつあることを確かめる。
fn routing_files_metadata(
    metadata: &Value,
) -> Result<BTreeMap<String, RoutingFileMetadata>, LoadError> {
    let routing = metadata
        .get("routing")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("metadataにroutingがありません"))?;
    let files = routing
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("metadataのrouting.filesが配列ではありません"))?;
    if files.len() != ROUTING_FILES.len() {
        return Err(invalid("metadataのrouting.filesが8ファイルではありません"));
    }
    let mut out = BTreeMap::new();
    for value in files {
        let object = value
            .as_object()
            .ok_or_else(|| invalid("metadataのrouting.filesの要素がオブジェクトではありません"))?;
        let file = object
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("metadataのrouting.files[].fileが文字列ではありません"))?;
        if !ROUTING_FILES.contains(&file) {
            return Err(invalid(format!(
                "metadataのrouting.filesに8ファイル以外の{file}があります"
            )));
        }
        let sha256 = object
            .get("sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("metadataの{file}.sha256が文字列ではありません")))?;
        let bytes = object
            .get("bytes")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid(format!("metadataの{file}.bytesが整数ではありません")))?;
        let magic = object
            .get("magic")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("metadataの{file}.magicが文字列ではありません")))?;
        let format_version = object
            .get("format_version")
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| {
                invalid(format!(
                    "metadataの{file}.format_versionが32ビットの整数ではありません"
                ))
            })?;
        let sections = object
            .get("sections")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                invalid(format!(
                    "metadataの{file}.sectionsがオブジェクトではありません"
                ))
            })?;
        let mut counts = BTreeMap::new();
        for (name, count) in sections {
            let count = count.as_u64().ok_or_else(|| {
                invalid(format!(
                    "metadataの{file}.sections.{name}が非負の整数ではありません"
                ))
            })?;
            counts.insert(name.clone(), count);
        }
        let entry = RoutingFileMetadata {
            sha256: sha256.to_owned(),
            bytes,
            magic: magic.to_owned(),
            format_version,
            sections: counts,
        };
        if out.insert(file.to_owned(), entry).is_some() {
            return Err(invalid(format!(
                "metadataのrouting.filesに{file}が重複しています"
            )));
        }
    }
    for &file in &ROUTING_FILES {
        if !out.contains_key(file) {
            return Err(invalid(format!(
                "metadataのrouting.filesに{file}がありません"
            )));
        }
    }
    Ok(out)
}

/// `.bin` の容器・マジックナンバー・`format_version`・節の `count`・`DEPS` を確かめる。
fn check_bin_file(
    release_dir: &Path,
    file: &str,
    expected: &RoutingFileMetadata,
    hashes: &BTreeMap<String, String>,
) -> Result<(), LoadError> {
    let path = release_dir.join(file);
    let mut reader = BinFileReader::open(&path).map_err(|e| with_file(file, e))?;
    let (magic, format_version) = routing_file_spec(file)
        .ok_or_else(|| invalid(format!("{file}は8ファイルに含まれません")))?;
    reader
        .expect(magic, format_version)
        .map_err(|e| with_file(file, e))?;
    let actual_magic = String::from_utf8(reader.magic().to_vec()).expect("マジックナンバーはASCII");
    if actual_magic != expected.magic {
        return Err(invalid(format!(
            "{file}のマジックナンバー{actual_magic}がmetadataと一致しません"
        )));
    }
    if reader.format_version() != expected.format_version {
        return Err(invalid(format!(
            "{file}のformat_versionがmetadataと一致しません"
        )));
    }
    let sections = reader.section_counts();
    if sections != expected.sections {
        return Err(invalid(format!("{file}の節の件数がmetadataと一致しません")));
    }
    let deps = reader.deps().map_err(|e| with_file(file, e))?;
    for key in required_deps(file) {
        if !deps.contains_key(*key) {
            return Err(invalid(format!("{file}のDEPSに{key}がありません")));
        }
    }
    for (key, hash) in deps {
        // release外の中間生成物のキーは照合しない（形式書4.1節）。
        if let Some(actual) = hashes.get(&key)
            && actual != &hash
        {
            return Err(invalid(format!(
                "{file}のDEPSの{key}が実ファイルのSHA-256と一致しません"
            )));
        }
    }
    Ok(())
}

/// 小さい `.bin` をバイト列から復号する。ファイル名を添えたエラーにする。
fn decode_small_file<T>(
    release_dir: &Path,
    file: &str,
    decode: impl FnOnce(&[u8]) -> Result<T, LoadError>,
) -> Result<T, LoadError> {
    let path = release_dir.join(file);
    let bytes = fs::read(&path).map_err(|e| with_file(file, LoadError::Io(e)))?;
    decode(&bytes).map_err(|e| with_file(file, e))
}

/// `alt.bin` の表の件数を `graph.bin` と照合する（P9 `package` と同じ検査）。
fn check_alt(
    landmarks: &crate::domain::routing::Landmarks,
    graph: &crate::domain::routing::Graph,
) -> Result<(), LoadError> {
    for mode in Mode::ALL {
        let Some(table) = landmarks.table(mode) else {
            continue;
        };
        if table.count() > 0
            && table.from.len() as u64 != graph.node_count() as u64 * table.count() as u64
        {
            return Err(with_file(
                super::release::ALT_FILE,
                invalid(format!(
                    "{}の表の件数がgraph.binのノード数と一致しません",
                    mode.as_str()
                )),
            ));
        }
        if let Some(to) = &table.to
            && to.len() != table.from.len()
        {
            return Err(with_file(
                super::release::ALT_FILE,
                invalid(format!("{}のTOの件数がFRと一致しません", mode.as_str())),
            ));
        }
        if table
            .nodes
            .iter()
            .any(|&node| u64::from(node) >= graph.node_count() as u64)
        {
            return Err(with_file(
                super::release::ALT_FILE,
                invalid(format!(
                    "{}のランドマークがgraph.binのノード数の範囲外です",
                    mode.as_str()
                )),
            ));
        }
    }
    Ok(())
}

/// `odindex.bin` の辺番号を `graph.bin` と照合する。
fn check_odindex(
    index: &crate::domain::routing::OdIndex,
    edge_count: usize,
) -> Result<(), LoadError> {
    if index
        .cell_edges
        .iter()
        .any(|&edge| edge as usize >= edge_count)
    {
        return Err(with_file(
            super::release::ODINDEX_FILE,
            invalid("CELLEDGにgraph.binの辺数の範囲外があります"),
        ));
    }
    if let Some(edges) = index.raw_params.get("edges").and_then(Value::as_u64)
        && edges != edge_count as u64
    {
        return Err(with_file(
            super::release::ODINDEX_FILE,
            invalid("PARAMSのedgesがgraph.binの辺数と一致しません"),
        ));
    }
    Ok(())
}

/// 祝日表を読み、metadataのハッシュと照合する。ハッシュも返す。
fn read_holidays(
    release_dir: &Path,
    metadata: &Value,
) -> Result<(HolidayCalendar, String), LoadError> {
    let bytes = fs::read(release_dir.join(HOLIDAYS_FILE))?;
    let sha256 = sha256_hex(&bytes);
    if metadata_str(metadata, &["holiday_table", "sha256"])? != sha256 {
        return Err(invalid("祝日表のハッシュがmetadataと一致しません"));
    }
    let calendar = parse_holiday_csv(&bytes).map_err(|e| invalid(e.to_string()))?;
    Ok((calendar, sha256))
}

/// 祝日表だけを読む（`load_calendar`）。事故データの全件ロードを待たない。
///
/// schema 2と3のどちらでも、祝日表の置き方とmetadataの `holiday_table` は同じ（形式書4章）。
pub fn load_holiday_calendar(release_dir: &Path) -> Result<HolidayCalendar, LoadError> {
    let metadata = read_metadata_value(release_dir)?;
    let schema = metadata.get("schema_version").and_then(Value::as_u64);
    if schema != Some(SCHEMA_VERSION) && schema != Some(SCHEMA_VERSION_ROUTING) {
        return Err(invalid("schema_versionが2または3ではありません"));
    }
    read_holidays(release_dir, &metadata).map(|(calendar, _)| calendar)
}

fn parse_field<T: std::str::FromStr>(value: &str) -> Result<T, LoadError> {
    value
        .parse()
        .map_err(|_| invalid("accidents.csv.gzに不正な値があります"))
}

fn check(ok: bool) -> Result<(), LoadError> {
    if ok {
        Ok(())
    } else {
        Err(invalid("accidents.csv.gzに範囲外の値があります"))
    }
}

/// gzip CSVを列配列に読む。先頭10列（`day_type` まで）だけを数値化して検証する。
pub fn parse_accidents(gzip: &[u8]) -> Result<AccidentColumns, LoadError> {
    // 展開しながら読む（展開後の全体をメモリに置かない）
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(GzDecoder::new(gzip));
    let header = reader
        .headers()
        .map_err(|_| invalid("accidents.csv.gzのヘッダを読めません"))?;
    if header.iter().ne(OUTPUT_COLUMNS) {
        return Err(invalid("accidents.csv.gzの列が17列の形式と一致しません"));
    }
    let mut c = AccidentColumns::default();
    for record in reader.records() {
        // 列数の不一致もここでCSVのエラーになる
        let record = record.map_err(|_| invalid("accidents.csv.gzの行を読めません"))?;
        let lat: f64 = parse_field(&record[0])?;
        let lon: f64 = parse_field(&record[1])?;
        let modes: u8 = parse_field(&record[2])?;
        let period: u8 = parse_field(&record[3])?;
        let weather: u8 = parse_field(&record[4])?;
        let day_type: u8 = parse_field(&record[5])?;
        let deaths: u16 = parse_field(&record[6])?;
        let injuries: u16 = parse_field(&record[7])?;
        let source_year: u16 = parse_field(&record[8])?;
        let date: u32 = parse_field(&record[9])?;
        check(LAT_RANGE.contains(&lat) && LON_RANGE.contains(&lon))?;
        check(modes <= 7)?;
        check(Period::from_code(period).is_some())?;
        check(Weather::from_code(weather).is_some())?;
        check(DayType::from_code(day_type).is_some())?;
        // 人数は各0〜999で合計1以上（behavior.md 2章）
        check(deaths <= 999 && injuries <= 999 && deaths + injuries >= 1)?;
        // 発生日は実在する8桁のYYYYMMDDで、1900 <= 発生年 <= source_year（behavior.md 2章。
        // openapi.yamlの上限 99991231 のとおり年は4桁まで）
        let year = date / 10_000;
        check(
            (1900..=u32::from(source_year).min(9999)).contains(&year)
                && NaiveDate::from_ymd_opt(year as i32, (date / 100) % 100, date % 100).is_some(),
        )?;
        c.lat.push(lat);
        c.lon.push(lon);
        c.modes.push(modes);
        c.period.push(period);
        c.weather.push(weather);
        c.day_type.push(day_type);
        c.deaths.push(deaths);
        c.injuries.push(injuries);
        c.source_year.push(source_year);
        c.date.push(date);
    }
    if c.is_empty() {
        return Err(invalid("accidents.csv.gzに事故がありません"));
    }
    Ok(c)
}

/// releaseの実ファイルから読んだ、schema 2と共通の入力。
struct ReleaseInputs {
    gzip: Vec<u8>,
    dataset_id: String,
    holiday_sha256: String,
}

/// 祝日表・事故CSVを読み、`dataset_id` を計算してmetadataと照合する。
fn read_release_inputs(release_dir: &Path, metadata: &Value) -> Result<ReleaseInputs, LoadError> {
    let (_, holiday_sha256) = read_holidays(release_dir, metadata)?;
    let gzip = fs::read(release_dir.join(ACCIDENTS_FILE))?;
    let dataset_id = sha256_hex(&gzip);
    if metadata_str(metadata, &["dataset_id"])? != dataset_id {
        return Err(invalid("dataset_idがaccidents.csv.gzと一致しません"));
    }
    Ok(ReleaseInputs {
        gzip,
        dataset_id,
        holiday_sha256,
    })
}

/// 事故CSVを全件解析し、`rows` を照合してスナップショットと年度を作る。
fn snapshot_from_inputs(
    inputs: ReleaseInputs,
) -> Result<(AccidentSnapshot, Vec<u16>, u64), LoadError> {
    let columns = parse_accidents(&inputs.gzip)?;
    let rows = columns.len() as u64;
    let mut data_years = columns.source_year.clone();
    data_years.sort_unstable();
    data_years.dedup();
    let snapshot = AccidentSnapshot::new(columns);
    Ok((snapshot, data_years, rows))
}

/// releaseを読み、識別子と件数を検証してスナップショットを作る。
pub fn load_snapshot(release_dir: &Path) -> Result<(AccidentSnapshot, DatasetInfo), LoadError> {
    let started = Instant::now();
    let metadata = read_metadata(release_dir)?;
    let inputs = read_release_inputs(release_dir, &metadata)?;
    let release_id = release_id(&inputs.dataset_id, &inputs.holiday_sha256);
    if metadata_str(&metadata, &["release_id"])? != release_id {
        return Err(invalid("release_idが実ファイルと一致しません"));
    }
    let (snapshot, data_years, rows) = snapshot_from_inputs(inputs)?;
    if metadata.get("rows").and_then(Value::as_u64) != Some(rows) {
        return Err(invalid("事故の件数がmetadataと一致しません"));
    }
    let dataset = DatasetInfo {
        release_id,
        dataset_id: metadata_str(&metadata, &["dataset_id"])?.to_owned(),
        accidents: snapshot.len() as u64,
        data_years,
        load_seconds: started.elapsed().as_secs_f64(),
        grid_cells: Some(snapshot.grid_cells()),
    };
    Ok((snapshot, dataset))
}
