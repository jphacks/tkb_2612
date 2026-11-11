//! 安全危険情報（地点ごとの「安全」「危険」の投票と口コミ）の保存（Issue #23）。
//!
//! 投票の判定（[`new_report`]・[`apply_vote`]）は保存方式から分けた純粋な処理で、保存方式は
//! [`CommunityStore`] で差し替える（ローカル開発用の [`LocalCommunityStore`]、本番のFirestore）。
//!
//! 重複投票は匿名の投票者ID（ブラウザーごとの乱数）で判定する。保存するのはIDのハッシュだけで、
//! 元のIDは保存しない。IDを消せば再投票できるので、厳密な本人確認ではない。

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contract::BoxFuture;

/// 口コミ1件の文字数の上限（trim後、Unicode文字数）。
pub const MAX_TEXT_CHARS: usize = 500;
/// 1地点に残す口コミの上限。超えたら古いものから消す（票は消さない）。
pub const MAX_COMMENTS: usize = 100;
/// 1地点の票の上限。超えた投票は受け付けない（文書の大きさを抑えるため）。
pub const MAX_VOTES: usize = 5_000;
/// 一覧で返す地点の上限（新しい順）。
pub const MAX_LIST: usize = 500;
/// 投票者IDの文字数（クライアントが作る乱数。英数字・`-`・`_`）。
pub const VOTER_ID_CHARS: std::ops::RangeInclusive<usize> = 16..=64;

/// 投票の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SafetyKind {
    Safe,
    Danger,
}

impl SafetyKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "safe" => Some(SafetyKind::Safe),
            "danger" => Some(SafetyKind::Danger),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SafetyKind::Safe => "safe",
            SafetyKind::Danger => "danger",
        }
    }
}

/// 匿名の投票者ID。保持するのはクライアントのIDのハッシュ。
#[derive(Clone, PartialEq, Eq)]
pub struct VoterId(String);

impl std::fmt::Debug for VoterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VoterId(..)")
    }
}

impl VoterId {
    /// クライアントのIDを検査してハッシュにする。形式が違えば `None`。
    pub fn from_client(raw: &str) -> Option<Self> {
        let valid = VOTER_ID_CHARS.contains(&raw.len())
            && raw
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !valid {
            return None;
        }
        let digest = Sha256::digest(format!("acciless-community-voter:{raw}").as_bytes());
        Some(Self(hex(&digest[..16])))
    }

    pub fn hashed(&self) -> &str {
        &self.0
    }
}

/// 保存する1票。`voter` は [`VoterId`] のハッシュ。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredVote {
    pub voter: String,
    pub kind: SafetyKind,
    /// UNIXミリ秒。
    pub at_ms: i64,
}

/// 保存する口コミ。投票と一緒にだけ書ける（投票者ごとに1件）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredComment {
    pub id: String,
    pub kind: SafetyKind,
    pub text: String,
    /// UNIXミリ秒。
    pub at_ms: i64,
}

/// 保存する地点。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredReport {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    /// UNIXミリ秒。
    pub created_at_ms: i64,
    pub votes: Vec<StoredVote>,
    pub comments: Vec<StoredComment>,
}

impl StoredReport {
    pub fn count(&self, kind: SafetyKind) -> usize {
        self.votes.iter().filter(|v| v.kind == kind).count()
    }

    /// その投票者の票。
    pub fn vote_of(&self, voter: &VoterId) -> Option<SafetyKind> {
        self.votes
            .iter()
            .find(|v| v.voter == voter.hashed())
            .map(|v| v.kind)
    }
}

/// 1回の投票の内容。`text` はtrim済みで1〜[`MAX_TEXT_CHARS`]文字（空ならNone）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ballot {
    pub kind: SafetyKind,
    pub text: Option<String>,
}

/// 地点の範囲（両端を含む）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
}

impl Bounds {
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        (self.min_lat..=self.max_lat).contains(&lat) && (self.min_lon..=self.max_lon).contains(&lon)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunityError {
    /// 地点が無い。
    NotFound,
    /// この投票者は投票済み。
    AlreadyVoted,
    /// 票の上限に達した。
    Full,
    /// 保存先の読み書きの失敗・競合の未解消。
    Unavailable,
}

/// 新しい地点（作成者の1票と、あれば口コミ付き）。
pub fn new_report(
    lat: f64,
    lon: f64,
    voter: &VoterId,
    ballot: Ballot,
    now_ms: i64,
) -> StoredReport {
    let mut report = StoredReport {
        id: new_id(),
        lat,
        lon,
        created_at_ms: now_ms,
        votes: Vec::new(),
        comments: Vec::new(),
    };
    apply_vote(&mut report, voter, ballot, now_ms).expect("空の地点には必ず投票できる");
    report
}

/// 地点に1票を加える。投票済み・上限到達なら変更しない。
pub fn apply_vote(
    report: &mut StoredReport,
    voter: &VoterId,
    ballot: Ballot,
    now_ms: i64,
) -> Result<(), CommunityError> {
    if report.vote_of(voter).is_some() {
        return Err(CommunityError::AlreadyVoted);
    }
    if report.votes.len() >= MAX_VOTES {
        return Err(CommunityError::Full);
    }
    report.votes.push(StoredVote {
        voter: voter.hashed().to_owned(),
        kind: ballot.kind,
        at_ms: now_ms,
    });
    if let Some(text) = ballot.text {
        report.comments.push(StoredComment {
            id: new_id(),
            kind: ballot.kind,
            text,
            at_ms: now_ms,
        });
        let excess = report.comments.len().saturating_sub(MAX_COMMENTS);
        report.comments.drain(..excess);
    }
    Ok(())
}

/// 範囲内の地点を新しい順に最大 [`MAX_LIST`] 件にする。
pub fn select_listed(
    reports: impl IntoIterator<Item = StoredReport>,
    bounds: Bounds,
) -> Vec<StoredReport> {
    let mut listed: Vec<StoredReport> = reports
        .into_iter()
        .filter(|r| bounds.contains(r.lat, r.lon))
        .collect();
    listed.sort_by(|a, b| {
        b.created_at_ms
            .cmp(&a.created_at_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    listed.truncate(MAX_LIST);
    listed
}

/// 推測しにくい32桁の16進ID（地点・口コミ）。
pub fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    // RandomStateはプロセスごとに乱数で初期化される
    let random = |salt: u64| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u128(nanos);
        hasher.finish()
    };
    let mut seed = Sha256::new();
    seed.update(random(count).to_le_bytes());
    seed.update(random(!count).to_le_bytes());
    seed.update(count.to_le_bytes());
    seed.update(nanos.to_le_bytes());
    hex(&seed.finalize()[..16])
}

/// 地点IDの形式（[`new_id`] が作る32桁の小文字16進）か。
pub fn is_report_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 地点の保存方式。
pub trait CommunityStore: Send + Sync {
    /// 範囲内の地点（[`select_listed`] の順と件数）。
    fn list(&self, bounds: Bounds) -> BoxFuture<'_, Result<Vec<StoredReport>, CommunityError>>;

    /// 新しい地点を保存する。
    fn create(&self, report: StoredReport) -> BoxFuture<'_, Result<(), CommunityError>>;

    /// 地点に [`apply_vote`] で1票を加え、加えた後の地点を返す。
    fn vote<'a>(
        &'a self,
        id: &'a str,
        voter: &'a VoterId,
        ballot: Ballot,
        now_ms: i64,
    ) -> BoxFuture<'a, Result<StoredReport, CommunityError>>;
}

/// プロセス内だけに保存する（ローカル開発専用。再起動で消える）。
#[derive(Debug, Default)]
pub struct LocalCommunityStore {
    reports: Mutex<HashMap<String, StoredReport>>,
}

impl LocalCommunityStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CommunityStore for LocalCommunityStore {
    fn list(&self, bounds: Bounds) -> BoxFuture<'_, Result<Vec<StoredReport>, CommunityError>> {
        let reports: Vec<StoredReport> = self.reports.lock().unwrap().values().cloned().collect();
        Box::pin(async move { Ok(select_listed(reports, bounds)) })
    }

    fn create(&self, report: StoredReport) -> BoxFuture<'_, Result<(), CommunityError>> {
        self.reports
            .lock()
            .unwrap()
            .insert(report.id.clone(), report);
        Box::pin(async { Ok(()) })
    }

    fn vote<'a>(
        &'a self,
        id: &'a str,
        voter: &'a VoterId,
        ballot: Ballot,
        now_ms: i64,
    ) -> BoxFuture<'a, Result<StoredReport, CommunityError>> {
        let result = (|| {
            let mut reports = self.reports.lock().unwrap();
            let report = reports.get_mut(id).ok_or(CommunityError::NotFound)?;
            apply_vote(report, voter, ballot, now_ms)?;
            Ok(report.clone())
        })();
        Box::pin(async move { result })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voter(n: u8) -> VoterId {
        VoterId::from_client(&format!("voter-{n:02}-abcdefghijk")).unwrap()
    }

    fn now() -> i64 {
        1_794_268_800_000
    }

    fn ballot(kind: SafetyKind, text: Option<&str>) -> Ballot {
        Ballot {
            kind,
            text: text.map(str::to_owned),
        }
    }

    #[test]
    fn voter_ids_are_checked_and_hashed() {
        assert!(VoterId::from_client("short").is_none());
        assert!(VoterId::from_client(&"a".repeat(65)).is_none());
        assert!(VoterId::from_client("abcdefghijklmnop!").is_none());
        let id = VoterId::from_client("abcdefghijklmnop").unwrap();
        assert_eq!(id.hashed().len(), 32);
        assert!(!id.hashed().contains("abcdefghijklmnop"));
        assert_eq!(Some(id), VoterId::from_client("abcdefghijklmnop"));
    }

    #[test]
    fn the_creator_votes_once_and_others_vote_separately() {
        let mut report = new_report(
            35.0,
            139.0,
            &voter(1),
            ballot(SafetyKind::Danger, Some("暗い")),
            now(),
        );
        assert!(is_report_id(&report.id));
        assert_eq!(report.count(SafetyKind::Danger), 1);
        assert_eq!(report.comments.len(), 1);
        assert_eq!(
            apply_vote(
                &mut report,
                &voter(1),
                ballot(SafetyKind::Safe, None),
                now()
            ),
            Err(CommunityError::AlreadyVoted)
        );
        apply_vote(
            &mut report,
            &voter(2),
            ballot(SafetyKind::Safe, None),
            now(),
        )
        .unwrap();
        assert_eq!(report.count(SafetyKind::Safe), 1);
        assert_eq!(report.count(SafetyKind::Danger), 1);
        assert_eq!(report.comments.len(), 1);
        assert_eq!(report.vote_of(&voter(2)), Some(SafetyKind::Safe));
        assert_eq!(report.vote_of(&voter(3)), None);
    }

    #[test]
    fn only_the_newest_comments_are_kept() {
        let mut report = new_report(
            35.0,
            139.0,
            &voter(0),
            ballot(SafetyKind::Safe, Some("0")),
            now(),
        );
        for n in 1..=MAX_COMMENTS {
            let id = VoterId::from_client(&format!("voter-{n:04}-abcdefghij")).unwrap();
            apply_vote(
                &mut report,
                &id,
                ballot(SafetyKind::Safe, Some(&n.to_string())),
                now(),
            )
            .unwrap();
        }
        assert_eq!(report.comments.len(), MAX_COMMENTS);
        assert_eq!(report.comments[0].text, "1");
        assert_eq!(report.votes.len(), MAX_COMMENTS + 1);
    }

    #[test]
    fn votes_stop_at_the_limit() {
        let mut report = new_report(
            35.0,
            139.0,
            &voter(0),
            ballot(SafetyKind::Safe, None),
            now(),
        );
        report.votes = (0..MAX_VOTES)
            .map(|n| StoredVote {
                voter: format!("{n}"),
                kind: SafetyKind::Safe,
                at_ms: now(),
            })
            .collect();
        assert_eq!(
            apply_vote(
                &mut report,
                &voter(1),
                ballot(SafetyKind::Danger, None),
                now()
            ),
            Err(CommunityError::Full)
        );
    }

    #[test]
    fn ids_differ() {
        let ids: std::collections::HashSet<String> = (0..1_000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 1_000);
        assert!(ids.iter().all(|id| is_report_id(id)));
    }
}
