//! 上流へのHTTP通信の共通部分。
//!
//! 識別UA・`Accept: application/json`・リダイレクトなし・再試行なしで1回だけ送り、
//! `min(1通信の期限, 要求全体の残り)` で打ち切る。応答本文は上限バイト数まで読み、
//! 超えたら失敗にする（遅い分割応答も1通信の期限で打ち切る）。
//!
//! 失敗の詳細（URL・座標・住所・鍵を含み得る）は呼出し元へ返さず、ログにも出さない。

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, HeaderValue};

use crate::contract::{CancelToken, Interrupt};

/// 1通信の失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FetchError {
    /// 接続・HTTP失敗（429を含む）・1通信の期限切れ・サイズ超過。
    Failed,
    /// 要求全体の期限切れまたは取消しで打ち切った。
    Interrupted(Interrupt),
}

impl From<Interrupt> for FetchError {
    fn from(value: Interrupt) -> Self {
        FetchError::Interrupted(value)
    }
}

/// 上流HTTPクライアント。複製は同じ接続プールを共有する。
#[derive(Debug, Clone)]
pub struct UpstreamClient {
    client: reqwest::Client,
}

/// クライアントを作れなかった（UAにヘッダーとして使えない文字がある等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientBuildError;

impl UpstreamClient {
    pub fn new(user_agent: &str) -> Result<Self, ClientBuildError> {
        let user_agent = HeaderValue::from_str(user_agent).map_err(|_| ClientBuildError)?;
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .redirect(reqwest::redirect::Policy::none())
            // 環境変数のプロキシ設定に上流の座標入りURLを渡さない
            .no_proxy()
            .build()
            .map_err(|_| ClientBuildError)?;
        Ok(Self { client })
    }

    /// GETを1回送り、2xxの本文を返す。
    ///
    /// `timeout` は1通信の期限。要求全体の残りの方が短ければそちらで打ち切り、
    /// その場合は [`FetchError::Interrupted`] を返す。
    pub async fn get(
        &self,
        url: reqwest::Url,
        timeout: Duration,
        max_bytes: usize,
        cancel: &CancelToken,
    ) -> Result<Bytes, FetchError> {
        cancel.check()?;
        // 要求全体の残りが1通信の期限より短いなら、そちらで打ち切る
        let limited_by_request = cancel.remaining().is_some_and(|r| r < timeout);
        let wait = cancel.remaining().map_or(timeout, |r| r.min(timeout));
        let deadline = tokio::time::Instant::now() + wait;
        let request = self
            .client
            .get(url)
            .header(ACCEPT, HeaderValue::from_static("application/json"));
        let exchange = async {
            let response = request.send().await.map_err(|_| FetchError::Failed)?;
            if !response.status().is_success() {
                return Err(FetchError::Failed);
            }
            if response
                .content_length()
                .is_some_and(|n| n > max_bytes as u64)
            {
                return Err(FetchError::Failed);
            }
            let mut body = BytesMut::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| FetchError::Failed)?;
                if body.len() + chunk.len() > max_bytes {
                    return Err(FetchError::Failed);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(body.freeze())
        };
        let result = tokio::select! {
            biased;
            interrupt = crate::providers::gate::until_interrupted(cancel) => Err(interrupt.into()),
            result = tokio::time::timeout_at(deadline, exchange) => match result {
                Ok(result) => result,
                Err(_) if limited_by_request => Err(Interrupt::DeadlineExceeded.into()),
                Err(_) => Err(FetchError::Failed),
            },
        };
        // 要求全体の期限で打ち切った場合は、1通信の失敗ではなく中断として返す
        match result {
            Err(FetchError::Failed) => match cancel.check() {
                Err(interrupt) => Err(interrupt.into()),
                Ok(()) => Err(FetchError::Failed),
            },
            other => other,
        }
    }
}
