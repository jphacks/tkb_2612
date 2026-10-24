//! 経路取得のインターフェース。
//!
//! ③が提供元アダプタ（OSRM等）として [`Router`] を実装し、②が候補生成の中から呼ぶ。
//! - ③：URL組立て・識別UA・上流ゲート（直列化・開始間隔・待ち行列）・8秒の通信期限・
//!   応答サイズ上限・JSONの構造の解釈・提供元固有エラーの変換。
//! - ②：追加候補の要否、左右経由点の計算、呼出し順（初回→左→右）、値の検証、
//!   80km判定、重複排除、並べ替え、警告。
//!
//! 不正な経路（[`MalformedRoute`] と、②の値の検証で弾いたもの）の扱いは②が決める。
//! 初回の呼出しでは要求全体を `ExternalApi` にし、追加の呼出しではそれより前の正常な
//! 経路を残して警告にする。
//!
//! 1回の `route` 呼出しが上流1通信に対応する。アダプタは再試行しない。

use super::BoxFuture;
use super::cancel::{CancelToken, Interrupt};
use super::conditions::Mode;
use super::geo::{LatLon, LonLat};

/// 1回の呼出しで返す候補の上限。提供元がそれより多く返しても先頭だけを解釈する。
pub const MAX_ROUTES_PER_CALL: usize = 6;

/// 経路取得の入力。経由点は0または1個。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutingRequest {
    pub origin: LatLon,
    pub dest: LatLon,
    pub mode: Mode,
    pub via: Option<LatLon>,
}

/// 提供元が返した1経路。値の妥当性はまだ検証していない。
#[derive(Debug, Clone, PartialEq)]
pub struct RoutedPath {
    /// 推定所要時間（秒）。
    pub duration_s: f64,
    /// 提供元の経路距離（m）。
    pub distance_m: f64,
    /// 経路の頂点列（始点→終点）。標高は持たない。
    pub coordinates: Vec<LonLat>,
}

/// 提供元の1経路が、型として解釈できない形だった。
///
/// geometryがLineStringでない、頂点が数値2個の `[lon, lat]` でない（標高つきを含む）、
/// duration・distanceが数値でない、など。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MalformedRoute;

/// アダプタが返す1経路分の結果。
///
/// 1経路の形の不正は呼出し全体の失敗にせず、その位置に置く。追加候補の取得では、
/// 不正な経路より前の正常な経路を②が残すため（behavior.md 5章）。
pub type ProviderRoute = Result<RoutedPath, MalformedRoute>;

/// 経路取得の失敗。HTTP応答への変換は③が行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutingError {
    /// 提供元が経路なしと答えた（OSRMのcodeがOk以外、routesが空、外枠がobjectでない）。
    NoRoute,
    /// 通信失敗・上流HTTP失敗（429を含む）・8秒の通信期限切れ・サイズ超過・
    /// JSONの解析失敗や外枠の構造の不正（routesが配列でない等）。1経路の形の不正は含まない。
    ExternalApi,
    /// 待ち行列の満杯、共有上流ゲートの読取失敗・競合未解消。
    Busy,
    /// 要求全体の期限切れまたは取消し。
    Interrupted(Interrupt),
}

impl RoutingError {
    /// 追加候補の取得中でも要求全体を止める失敗か。
    ///
    /// `NoRoute` と `ExternalApi` は追加候補なら警告にできるが、
    /// `Busy` と `Interrupted` は部分成功にしない（behavior.md 5章）。
    pub fn aborts_request(&self) -> bool {
        matches!(self, RoutingError::Busy | RoutingError::Interrupted(_))
    }
}

impl From<Interrupt> for RoutingError {
    fn from(value: Interrupt) -> Self {
        RoutingError::Interrupted(value)
    }
}

/// 経路提供元のアダプタ。
///
/// 実装の約束：
/// - 成功時は提供元の順序のまま、先頭 [`MAX_ROUTES_PER_CALL`] 本までを1本ずつ解釈して返す。
/// - 形を解釈できない経路は、その位置に `Err(MalformedRoute)` を置く。②はそこで検査を
///   止めるので、それより後の経路は返さなくてよい。
/// - 数値の範囲・頂点数・距離の検証は②が行うので、アダプタでは弾かない。
/// - 送信前に `cancel.check()` を確認し、待ち時間の上限は `min(通信期限, cancel.remaining())`。
///   要求全体の期限で打ち切った場合は `ExternalApi` ではなく `Interrupted(DeadlineExceeded)` を返す。
pub trait Router: Send + Sync {
    fn route<'a>(
        &'a self,
        request: &'a RoutingRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Vec<ProviderRoute>, RoutingError>>;
}
