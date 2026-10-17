//! 后台任务守护：panic 不再让轮询或维护循环静默停止。

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures_util::FutureExt;

/// 执行一轮工作；panic 时记 error 日志并返回 `None`，调用方继续下一轮。
pub async fn catch<T>(name: &str, work: impl Future<Output = T>) -> Option<T> {
    match AssertUnwindSafe(work).catch_unwind().await {
        Ok(value) => Some(value),
        Err(payload) => {
            tracing::error!(task = name, "后台任务 panic: {}", panic_text(&*payload));
            None
        }
    }
}

/// 启动一个常驻任务。任务 panic 后按 30 秒起、最长 10 分钟的退避重新创建；
/// 正常返回则结束。
pub fn spawn<F, Fut>(name: &'static str, make: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut panics: u32 = 0;
        loop {
            if catch(name, make()).await.is_some() {
                return;
            }
            panics = panics.saturating_add(1);
            let delay = restart_delay(panics);
            tracing::error!(task = name, "后台任务将在 {} 秒后重启", delay.as_secs());
            tokio::time::sleep(delay).await;
        }
    });
}

fn restart_delay(panics: u32) -> Duration {
    let secs = 30u64
        .saturating_mul(1 << panics.saturating_sub(1).min(5))
        .min(600);
    if cfg!(test) {
        Duration::from_millis(secs)
    } else {
        Duration::from_secs(secs)
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "未知 panic".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn catch_turns_a_panic_into_none() {
        assert_eq!(catch("ok", async { 7 }).await, Some(7));
        let caught: Option<()> = catch("boom", async { panic!("坏数据") }).await;
        assert!(caught.is_none());
    }

    #[tokio::test]
    async fn spawned_task_is_restarted_after_a_panic() {
        let runs = Arc::new(AtomicUsize::new(0));
        let counter = runs.clone();
        spawn("test", move || {
            let counter = counter.clone();
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) < 2 {
                    panic!("第一次和第二次都 panic");
                }
            }
        });
        for _ in 0..100 {
            if runs.load(Ordering::SeqCst) >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(runs.load(Ordering::SeqCst), 3);
        assert!(restart_delay(1) < restart_delay(2));
        assert_eq!(restart_delay(50), restart_delay(6));
    }
}
