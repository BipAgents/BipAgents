//! Agents made in a task of their own (26/11, Alex testing 1.1 on his own install: agents Bip proposed were said
//! « n'a pas pu être créé », then offered on the home as « Relier » a while later).
//!
//! A creation used to run inside its request. On an install with no spare agents (agent_pool.rs: none on a
//! self-install) one took longer than the apps wait (Android 60 s, the iPhone 90 s), the more so with several cards
//! tapped in a row (one creation at a time, `agents_admin::LOCK`). The app gave up and said it failed; its connection
//! closed, the request's future was dropped, and the creation stopped wherever it was: its Hermes profile made but not
//! yet in the bridge's config, which the home then listed as a profile « created on the server » to link.
//!
//! Now the work runs in a task of its own, under a key (the card's id, the form's `client_id`): the request waits for
//! it up to a few seconds and answers what was made, or `202 {creating: true}` while it goes on; the same request
//! again waits for the same work (nothing made twice) and answers its end. A real failure is answered as such, the
//! card left there to tap again.
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::web::HttpError;

/// What a creation answers: its status and body, or its failure.
pub type Made = Result<(u16, Value), HttpError>;

/// How long a request waits for its creation before answering `202 {creating: true}` (`[guide]
/// answer_wait_seconds`, 25 s): well under the apps' own waits (Android's 60 s read timeout, the iPhone's 90 s), so
/// that a slow creation is never taken for a failure.
pub fn answer_wait(config: &crate::config::Config) -> Duration {
    Duration::from_secs(config.guide.answer_wait_seconds)
}

/// How long a creation's answer is kept for the same request again (the form's `client_id`), once it ended well.
pub const KEPT: Duration = Duration::from_secs(15 * 60);

/// The creations that ended well lately, by key: when, and their answer.
type Done = HashMap<String, (std::time::Instant, (u16, Value))>;
static DONE: LazyLock<Mutex<Done>> = LazyLock::new(Default::default);

fn done_now() -> std::sync::MutexGuard<'static, Done> {
    DONE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The creations running now, by key: their answer once they end.
type Running = HashMap<String, tokio::sync::watch::Receiver<Option<Made>>>;
static RUNNING: LazyLock<Mutex<Running>> = LazyLock::new(Default::default);

fn running_now() -> std::sync::MutexGuard<'static, Running> {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the creation under `key` runs now.
pub fn running(key: &str) -> bool {
    running_now().contains_key(key)
}

/// The creation under `key`: started now with `work` unless it runs already (then `work` is not called), waited for
/// up to `wait`. None: still running (answer 202). The work keeps what a later request must find once it ended (a
/// card's answer): it is no longer listed here after its end.
pub async fn run_or_wait<F, W>(key: &str, wait: Duration, work: W) -> Option<Made>
where
    W: FnOnce() -> F,
    F: Future<Output = Made> + Send + 'static,
{
    if let Some((_, made)) = done_now().get(key).filter(|(at, _)| at.elapsed() < KEPT) {
        return Some(Ok(made.clone()));
    }
    let mut answer = {
        let mut running = running_now();
        match running.get(key) {
            Some(answer) => answer.clone(),
            None => {
                let (tell, answer) = tokio::sync::watch::channel(None);
                running.insert(key.to_string(), answer.clone());
                let made = work();
                let key = key.to_string();
                tokio::spawn(async move {
                    let made = made.await;
                    if let Ok(answer) = &made {
                        let mut done = done_now();
                        done.retain(|_, (at, _)| at.elapsed() < KEPT);
                        done.insert(key.clone(), (std::time::Instant::now(), answer.clone()));
                    }
                    // out of the list once its answer is kept: a request coming now finds it there
                    running_now().remove(&key);
                    let _ = tell.send(Some(made));
                });
                answer
            }
        }
    };
    let made = match tokio::time::timeout(wait, answer.wait_for(Option::is_some)).await {
        Ok(Ok(made)) => made.clone(),
        // the task ended without an answer (it panicked): a failure, said as such
        Ok(Err(_)) => Some(Err(HttpError::new(502, "the agent could not be created: try again in a moment"))),
        Err(_) => None,
    };
    made
}

/// A creation whose request may go away (the app closed, its wait over): it runs in a task of its own and ends its
/// work whatever happens to the request.
pub async fn detached<T: Send + 'static>(work: impl Future<Output = T> + Send + 'static) -> anyhow::Result<T> {
    Ok(tokio::spawn(work).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn a_slow_creation_answers_202_then_its_end_and_is_made_once() {
        let started = Arc::new(AtomicUsize::new(0));
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let work = {
            let started = started.clone();
            move || async move {
                started.fetch_add(1, Ordering::SeqCst);
                let _ = released.await;
                Ok((201, json!({"id": "cuisine"})))
            }
        };
        // the first request gives up waiting: still running, not a failure
        assert!(run_or_wait("t1", Duration::from_millis(50), work).await.is_none());
        assert!(running("t1"));
        // the same request again: the same work waited for, none started
        let again = {
            let started = started.clone();
            move || async move {
                started.fetch_add(1, Ordering::SeqCst);
                Ok((201, json!({"id": "other"})))
            }
        };
        let waiting = tokio::spawn(async move { run_or_wait("t1", Duration::from_secs(5), again).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        release.send(()).unwrap();
        let made = waiting.await.unwrap().unwrap().unwrap();
        assert_eq!(made, (201, json!({"id": "cuisine"})));
        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert!(!running("t1"));
        // asked again once made (the answer lost on the way): the same answer, nothing made again
        let late = {
            let started = started.clone();
            move || async move {
                started.fetch_add(1, Ordering::SeqCst);
                Ok((201, json!({"id": "twice"})))
            }
        };
        assert_eq!(run_or_wait("t1", Duration::from_secs(1), late).await.unwrap().unwrap().1["id"], "cuisine");
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_request_dropped_never_cuts_its_creation() {
        let (done, finished) = tokio::sync::oneshot::channel::<()>();
        let request = tokio::spawn(async move {
            run_or_wait("t2", Duration::from_secs(30), move || async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let _ = done.send(());
                Ok((201, json!({})))
            })
            .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        request.abort(); // the app's connection closed
        tokio::time::timeout(Duration::from_secs(5), finished).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_failure_is_answered_as_one() {
        let made = run_or_wait("t3", Duration::from_secs(5), || async { Err(HttpError::new(502, "no")) }).await;
        assert_eq!(made.unwrap().unwrap_err().status.as_u16(), 502);
        assert!(!running("t3"));
        // not kept: the same request again tries again
        let again = run_or_wait("t3", Duration::from_secs(5), || async { Ok((201, json!({"id": "x"}))) }).await;
        assert_eq!(again.unwrap().unwrap().0, 201);
    }
}
