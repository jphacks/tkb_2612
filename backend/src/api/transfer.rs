//! routeの応答本文の転送（behavior.md 7章）。
//!
//! 準備済みの全文を64KiBずつ渡し、送信開始（最初のフレーム）から転送期限
//! （`ROUTE_TRANSFER_TIMEOUT_MS`）を過ぎたら転送を中止する。サーバの待受ループが
//! 要求に [`ConnectionKill`] を付けていれば接続ごと切る（クライアントが読まずに
//! 送信が止まっている間も働く）。付いていなければ、次に本文が読まれた時点でエラーにする。
//! 途中切断をJSONエラーに差し替えない。

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use tokio::sync::Notify;

/// 1フレームの大きさ。
const FRAME_BYTES: usize = 64 * 1024;

/// 接続を切るための通知。サーバの待受ループが接続ごとに作り、要求の拡張に入れる。
#[derive(Debug, Clone)]
pub struct ConnectionKill(pub Arc<Notify>);

/// 転送期限を過ぎた。
#[derive(Debug)]
pub struct TransferTimeout;

impl std::fmt::Display for TransferTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("転送期限を過ぎました")
    }
}

impl std::error::Error for TransferTimeout {}

/// 期限付きの応答本文。
pub struct TransferBody {
    data: Bytes,
    timeout: Duration,
    kill: Option<Weak<Notify>>,
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl TransferBody {
    pub fn new(data: Bytes, timeout: Duration, kill: Option<&ConnectionKill>) -> Self {
        Self {
            data,
            timeout,
            kill: kill.map(|k| Arc::downgrade(&k.0)),
            deadline: None,
        }
    }

    fn start(&mut self) {
        self.deadline = Some(Box::pin(tokio::time::sleep(self.timeout)));
        if let Some(kill) = self.kill.take() {
            let timeout = self.timeout;
            // 本文を渡し終えた後も、送信バッファの書出しが終わるまでは同じ期限で見張る。
            // routeの応答は `Connection: close` なので、期限後に同じ接続で別の要求は動いていない
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                if let Some(kill) = kill.upgrade() {
                    kill.notify_one();
                }
            });
        }
    }
}

impl Body for TransferBody {
    type Data = Bytes;
    type Error = TransferTimeout;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, TransferTimeout>>> {
        let this = &mut *self;
        if this.deadline.is_none() {
            this.start();
        }
        if let Some(deadline) = this.deadline.as_mut()
            && deadline.as_mut().poll(cx).is_ready()
        {
            this.data = Bytes::new();
            return Poll::Ready(Some(Err(TransferTimeout)));
        }
        if this.data.is_empty() {
            return Poll::Ready(None);
        }
        let n = this.data.len().min(FRAME_BYTES);
        Poll::Ready(Some(Ok(Frame::data(this.data.split_to(n)))))
    }

    fn is_end_stream(&self) -> bool {
        self.data.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.data.len() as u64)
    }
}
