//! Benchmark tests for OJS Rust SDK core operations.
//!
//! Run with: cargo bench

#![allow(unused)]

use std::hint::black_box;
use std::time::Instant;

#[cfg(test)]
mod benchmarks {
    use super::*;

    const MINIMAL_JOB_JSON: &str = r#"{
        "id": "019539a4-b68c-7def-8000-1a2b3c4d5e6f",
        "type": "email.send",
        "state": "available",
        "queue": "default",
        "args": [{"to": "user@example.com"}],
        "attempt": 0,
        "created_at": "2024-01-15T10:30:00Z"
    }"#;

    const FULL_JOB_JSON: &str = r#"{
        "id": "019539a4-b68c-7def-8000-1a2b3c4d5e6f",
        "type": "email.send",
        "state": "active",
        "queue": "email",
        "args": [{"to": "user@example.com", "subject": "Welcome!"}],
        "priority": 10,
        "attempt": 2,
        "max_retries": 5,
        "tags": ["onboarding", "email"],
        "meta": {"campaign_id": "123", "source": "api"},
        "created_at": "2024-01-15T10:30:00Z",
        "scheduled_at": "2024-01-15T11:00:00Z"
    }"#;

    fn bench_loop<F: FnMut()>(name: &str, iterations: u64, mut f: F) {
        // Warmup
        for _ in 0..100 {
            f();
        }

        let start = Instant::now();
        for _ in 0..iterations {
            f();
        }
        let elapsed = start.elapsed();
        let per_op_ns = elapsed.as_nanos() as f64 / iterations as f64;

        if per_op_ns < 1000.0 {
            println!("  {name}: {per_op_ns:.1} ns/op ({iterations} iterations)");
        } else {
            let per_op_us = per_op_ns / 1000.0;
            println!("  {name}: {per_op_us:.2} µs/op ({iterations} iterations)");
        }
    }

    #[test]
    fn bench_json_deserialize_minimal() {
        bench_loop("serde_json::from_str (minimal job)", 100_000, || {
            let _: serde_json::Value = black_box(serde_json::from_str(MINIMAL_JOB_JSON).unwrap());
        });
    }

    #[test]
    fn bench_json_deserialize_full() {
        bench_loop("serde_json::from_str (full job)", 100_000, || {
            let _: serde_json::Value = black_box(serde_json::from_str(FULL_JOB_JSON).unwrap());
        });
    }

    #[test]
    fn bench_json_serialize_minimal() {
        let value: serde_json::Value = serde_json::from_str(MINIMAL_JOB_JSON).unwrap();
        bench_loop("serde_json::to_string (minimal job)", 100_000, || {
            let _ = black_box(serde_json::to_string(&value).unwrap());
        });
    }

    #[test]
    fn bench_json_serialize_full() {
        let value: serde_json::Value = serde_json::from_str(FULL_JOB_JSON).unwrap();
        bench_loop("serde_json::to_string (full job)", 100_000, || {
            let _ = black_box(serde_json::to_string(&value).unwrap());
        });
    }

    #[test]
    fn bench_json_roundtrip_minimal() {
        bench_loop("JSON roundtrip (minimal job)", 100_000, || {
            let v: serde_json::Value = serde_json::from_str(MINIMAL_JOB_JSON).unwrap();
            let _ = black_box(serde_json::to_string(&v).unwrap());
        });
    }

    #[test]
    fn bench_json_roundtrip_full() {
        bench_loop("JSON roundtrip (full job)", 100_000, || {
            let v: serde_json::Value = serde_json::from_str(FULL_JOB_JSON).unwrap();
            let _ = black_box(serde_json::to_string(&v).unwrap());
        });
    }
}
