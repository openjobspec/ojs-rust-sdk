use crate::errors::OjsError;
use crate::worker::JobContext;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A boxed future used throughout the middleware system.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The result type returned by job handlers.
pub type HandlerResult = Result<serde_json::Value, OjsError>;

/// A handler function that processes a job.
///
/// Handlers receive a [`JobContext`] and return a JSON result value on success.
pub type HandlerFn = Arc<
    dyn Fn(JobContext) -> BoxFuture<'static, HandlerResult> + Send + Sync,
>;

/// Represents the next handler in the middleware chain.
///
/// Call `run` to pass control to the next middleware or the final handler.
pub struct Next {
    inner: Box<dyn FnOnce(JobContext) -> BoxFuture<'static, HandlerResult> + Send>,
}

impl Next {
    pub(crate) fn new(
        f: impl FnOnce(JobContext) -> BoxFuture<'static, HandlerResult> + Send + 'static,
    ) -> Self {
        Self { inner: Box::new(f) }
    }

    /// Pass control to the next middleware or handler.
    pub fn run(self, ctx: JobContext) -> BoxFuture<'static, HandlerResult> {
        (self.inner)(ctx)
    }
}

// ---------------------------------------------------------------------------
// Middleware trait (tower-inspired)
// ---------------------------------------------------------------------------

/// Middleware that wraps job handler execution.
///
/// Middleware follows a tower-inspired pattern where each middleware wraps the
/// next handler in the chain. This enables cross-cutting concerns like logging,
/// tracing, metrics, timeouts, and error recovery.
///
/// # Example
///
/// ```rust,ignore
/// use ojs::{Middleware, Next, JobContext};
/// use std::time::Instant;
///
/// struct TimingMiddleware;
///
/// #[async_trait::async_trait]
/// impl Middleware for TimingMiddleware {
///     async fn handle(&self, ctx: JobContext, next: Next) -> Result<serde_json::Value, ojs::OjsError> {
///         let start = Instant::now();
///         let result = next.run(ctx).await;
///         tracing::info!(elapsed_ms = start.elapsed().as_millis(), "job processed");
///         result
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait Middleware: Send + Sync + 'static {
    /// Process a job, optionally delegating to the next handler.
    async fn handle(&self, ctx: JobContext, next: Next) -> HandlerResult;
}

// ---------------------------------------------------------------------------
// Middleware chain
// ---------------------------------------------------------------------------

pub(crate) struct NamedMiddleware {
    #[allow(dead_code)]
    pub name: String,
    pub middleware: Arc<dyn Middleware>,
}

/// An ordered chain of middleware that wraps a handler.
pub(crate) struct MiddlewareChain {
    middleware: Vec<NamedMiddleware>,
}

impl MiddlewareChain {
    pub fn new() -> Self {
        Self {
            middleware: Vec::new(),
        }
    }

    /// Append middleware to the end of the chain (outermost position).
    pub fn add(&mut self, name: impl Into<String>, mw: impl Middleware) {
        self.middleware.push(NamedMiddleware {
            name: name.into(),
            middleware: Arc::new(mw),
        });
    }

    /// Prepend middleware to the beginning of the chain (innermost position).
    #[allow(dead_code)]
    pub fn prepend(&mut self, name: impl Into<String>, mw: impl Middleware) {
        self.middleware.insert(
            0,
            NamedMiddleware {
                name: name.into(),
                middleware: Arc::new(mw),
            },
        );
    }

    /// Insert middleware before an existing named middleware.
    #[allow(dead_code)]
    pub fn insert_before(
        &mut self,
        existing: &str,
        name: impl Into<String>,
        mw: impl Middleware,
    ) {
        let pos = self
            .middleware
            .iter()
            .position(|m| m.name == existing)
            .unwrap_or(0);

        self.middleware.insert(
            pos,
            NamedMiddleware {
                name: name.into(),
                middleware: Arc::new(mw),
            },
        );
    }

    /// Insert middleware after an existing named middleware.
    #[allow(dead_code)]
    pub fn insert_after(
        &mut self,
        existing: &str,
        name: impl Into<String>,
        mw: impl Middleware,
    ) {
        let pos = self
            .middleware
            .iter()
            .position(|m| m.name == existing)
            .map(|i| i + 1)
            .unwrap_or(self.middleware.len());

        self.middleware.insert(
            pos,
            NamedMiddleware {
                name: name.into(),
                middleware: Arc::new(mw),
            },
        );
    }

    /// Remove a named middleware from the chain.
    #[allow(dead_code)]
    pub fn remove(&mut self, name: &str) {
        self.middleware.retain(|m| m.name != name);
    }

    /// Build the final handler by wrapping the base handler with all middleware.
    ///
    /// Middleware executes in order: first added = outermost wrapper.
    /// The chain is built from inside out (last middleware wraps the handler first).
    pub fn wrap(&self, handler: HandlerFn) -> HandlerFn {
        let mut h = handler;

        // Build from inside out: last middleware wraps handler first,
        // so first middleware in the list executes outermost.
        for named in self.middleware.iter().rev() {
            let mw = named.middleware.clone();
            let next_handler = h;
            h = Arc::new(move |ctx: JobContext| -> BoxFuture<'static, HandlerResult> {
                let mw = mw.clone();
                let next_handler = next_handler.clone();
                Box::pin(async move {
                    let next = Next::new(move |ctx| next_handler(ctx));
                    mw.handle(ctx, next).await
                })
            });
        }

        h
    }
}

// ---------------------------------------------------------------------------
// Convenience: implement Middleware for async closures via a wrapper
// ---------------------------------------------------------------------------

/// A middleware constructed from a closure.
pub struct FnMiddleware<F> {
    f: F,
}

impl<F, Fut> FnMiddleware<F>
where
    F: Fn(JobContext, Next) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = HandlerResult> + Send + 'static,
{
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

#[async_trait::async_trait]
impl<F, Fut> Middleware for FnMiddleware<F>
where
    F: Fn(JobContext, Next) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = HandlerResult> + Send + 'static,
{
    async fn handle(&self, ctx: JobContext, next: Next) -> HandlerResult {
        (self.f)(ctx, next).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestMiddleware {
        prefix: String,
    }

    #[async_trait::async_trait]
    impl Middleware for TestMiddleware {
        async fn handle(&self, ctx: JobContext, next: Next) -> HandlerResult {
            let mut result = next.run(ctx).await?;
            if let Some(obj) = result.as_object_mut() {
                obj.insert(
                    "middleware".to_string(),
                    serde_json::Value::String(self.prefix.clone()),
                );
            }
            Ok(result)
        }
    }

    #[test]
    fn test_middleware_chain_ordering() {
        let mut chain = MiddlewareChain::new();
        chain.add("first", TestMiddleware { prefix: "first".into() });
        chain.add("second", TestMiddleware { prefix: "second".into() });

        // Chain should have 2 middleware
        assert_eq!(chain.middleware.len(), 2);
        assert_eq!(chain.middleware[0].name, "first");
        assert_eq!(chain.middleware[1].name, "second");
    }

    #[test]
    fn test_middleware_chain_remove() {
        let mut chain = MiddlewareChain::new();
        chain.add("first", TestMiddleware { prefix: "first".into() });
        chain.add("second", TestMiddleware { prefix: "second".into() });
        chain.remove("first");

        assert_eq!(chain.middleware.len(), 1);
        assert_eq!(chain.middleware[0].name, "second");
    }
}
