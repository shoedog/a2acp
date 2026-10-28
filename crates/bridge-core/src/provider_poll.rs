//! The shared first-provider-poll boundary. Preparation never marks dispatch.
use crate::error::BridgeError;
use std::sync::atomic::{AtomicBool, Ordering};

/// Validate and observe synchronously with the first poll, retaining the owner's
/// cancellation permit until that poll returns. No yielding await holds it.
pub async fn poll_provider_v1<F, T, P>(
    future: F,
    observed: &AtomicBool,
    guard: impl FnOnce() -> Result<P, BridgeError>,
    register: impl FnOnce(),
) -> Result<T, BridgeError>
where
    F: std::future::Future<Output = Result<T, BridgeError>>,
{
    let mut future = Box::pin(future);
    let mut guard = Some(guard);
    let mut register = Some(register);
    let first = futures::future::poll_fn(|cx| {
        let permit = match guard.take().unwrap()() {
            Ok(permit) => permit,
            Err(error) => return std::task::Poll::Ready(Err(error)),
        };
        if !observed.swap(true, Ordering::AcqRel) {
            register.take().unwrap()();
        }
        let result = future.as_mut().poll(cx);
        drop(permit);
        std::task::Poll::Ready(Ok(result))
    })
    .await?;
    match first {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => future.await,
    }
}
