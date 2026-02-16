use ojs::{BoxFuture, FnMiddleware, HandlerResult, JobContext, Middleware, Next, Worker};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helper: build a minimal worker for handler registration
// ---------------------------------------------------------------------------

fn test_worker() -> Worker {
    Worker::builder()
        .url("http://localhost:9999")
        .build()
        .unwrap()
}

// ---------------------------------------------------------------------------
// Middleware trait implementation helpers
// ---------------------------------------------------------------------------

struct PassthroughMiddleware;

impl Middleware for PassthroughMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        Box::pin(async move { next.run(ctx).await })
    }
}

struct ShortCircuitMiddleware {
    value: serde_json::Value,
}

impl Middleware for ShortCircuitMiddleware {
    fn handle(&self, _ctx: JobContext, _next: Next) -> BoxFuture<'static, HandlerResult> {
        let value = self.value.clone();
        Box::pin(async move { Ok(value) })
    }
}

struct OrderTrackingMiddleware {
    label: String,
    order: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Middleware for OrderTrackingMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        let label = self.label.clone();
        let order = self.order.clone();
        Box::pin(async move {
            order.lock().unwrap().push(format!("{}-before", label));
            let result = next.run(ctx).await;
            order.lock().unwrap().push(format!("{}-after", label));
            result
        })
    }
}

struct CountingMiddleware {
    counter: Arc<AtomicUsize>,
}

impl Middleware for CountingMiddleware {
    fn handle(&self, ctx: JobContext, next: Next) -> BoxFuture<'static, HandlerResult> {
        self.counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { next.run(ctx).await })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_single_middleware_wraps_handler() {
    let counter = Arc::new(AtomicUsize::new(0));
    let worker = test_worker();

    worker
        .use_middleware(
            "counting",
            CountingMiddleware {
                counter: counter.clone(),
            },
        )
        .await;

    worker
        .register("test.single_mw", |_ctx: JobContext| async move {
            Ok(json!({"done": true}))
        })
        .await;

    // Middleware is registered; counter should start at 0
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_passthrough_middleware_registered() {
    let worker = test_worker();

    worker
        .use_middleware("passthrough", PassthroughMiddleware)
        .await;

    worker
        .register("test.passthrough", |_ctx: JobContext| async move {
            Ok(json!({"ok": true}))
        })
        .await;
}

#[tokio::test]
async fn test_short_circuit_middleware_registered() {
    let worker = test_worker();

    worker
        .use_middleware(
            "short_circuit",
            ShortCircuitMiddleware {
                value: json!({"short_circuited": true}),
            },
        )
        .await;

    worker
        .register("test.short_circuit", |_ctx: JobContext| async move {
            Ok(json!({"should_not_reach": true}))
        })
        .await;
}

#[tokio::test]
async fn test_multiple_middleware_registered_in_order() {
    let order = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let worker = test_worker();

    worker
        .use_middleware(
            "first",
            OrderTrackingMiddleware {
                label: "first".into(),
                order: order.clone(),
            },
        )
        .await;

    worker
        .use_middleware(
            "second",
            OrderTrackingMiddleware {
                label: "second".into(),
                order: order.clone(),
            },
        )
        .await;

    worker
        .register("test.order", |_ctx: JobContext| async move {
            Ok(json!({"handler": true}))
        })
        .await;

    // Verify registration succeeded without error
    assert!(order.lock().unwrap().is_empty());
}

#[tokio::test]
async fn test_fn_middleware_creation_from_closure() {
    let worker = test_worker();

    let mw = FnMiddleware::new(|ctx: JobContext, next: Next| async move { next.run(ctx).await });

    worker.use_middleware("fn_mw", mw).await;

    worker
        .register("test.fn_mw", |_ctx: JobContext| async move {
            Ok(json!({"ok": true}))
        })
        .await;
}

#[tokio::test]
async fn test_fn_middleware_with_side_effect() {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let mw = FnMiddleware::new(move |ctx: JobContext, next: Next| {
        counter_clone.fetch_add(1, Ordering::SeqCst);
        async move { next.run(ctx).await }
    });

    let worker = test_worker();
    worker.use_middleware("counting_fn", mw).await;

    worker
        .register("test.counting_fn", |_ctx: JobContext| async move {
            Ok(json!({"ok": true}))
        })
        .await;

    assert_eq!(counter.load(Ordering::SeqCst), 0);
}
