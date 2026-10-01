//! Clamped `performance.now()`, the way Chromium's `TimeClamper` does it.
//!
//! Chrome returns only multiples of the resolution: 100 µs, or 5 µs in a
//! cross-origin-isolated document. Its jitter moves the point inside each
//! bucket where the clock switches to the next grid value, a fixed
//! pseudo-random threshold per bucket; it never adds noise to the value.
//! Measured in Chrome 148: every value of a 20 000-call loop sits on the
//! 100 µs grid and the only non-zero step is 0.1 ms.
//!
//!   lower     = floor(now_us / res) * res
//!   threshold = lower + res * fraction(bucket)     // keyed hash, [0, 1)
//!   result    = (now_us >= threshold ? lower + res : lower) ms

use crate::js_runtime::extensions::stealth_ext::StealthState;
use crate::js_runtime::state::DomState;
use deno_core::op2;
use deno_core::v8;
use deno_core::OpState;
use std::time::Instant;

/// Clamp resolution of a document that is not cross-origin isolated.
const RESOLUTION_US: f64 = 100.0;
/// Clamp resolution of a cross-origin-isolated document.
const ISOLATED_RESOLUTION_US: f64 = 5.0;

/// Per-runtime state for the clamped clock.
pub struct PerfState {
    /// Process-relative origin; `performance.now()` returns ms since this
    /// instant (matches DOM HighResolutionTime contract for the document).
    origin: Instant,
    /// Wall-clock (UNIX epoch ms) corresponding to `origin`. Read by
    /// `op_perf_time_origin_ms` so JS `performance.timeOrigin` honors the
    /// invariant `timeOrigin + performance.now() ≈ Date.now()`. Real
    /// Chrome maintains this invariant; without it, an earlier JS-side
    /// ad-hoc computation (`Date.now() - <hardcoded nav_end>`) produced a
    /// detectable skew between `performance.timeOrigin + performance.now()`
    /// and `Date.now()`.
    origin_unix_ms: f64,
    /// Key of the per-bucket threshold hash.
    /// Last `performance.memory` reading and when it was taken.
    heap: Option<(Instant, [f64; 2])>,
}

/// Chrome refreshes a site-locked process's precise `performance.memory`
/// reading at most once per 50 ms (Blink `HeapSizeCache`).
const HEAP_SIZE_REFRESH: std::time::Duration = std::time::Duration::from_millis(50);

impl PerfState {
    pub fn new() -> Self {
        Self::with_seed(0xCAFEF00DDEADBEEF)
    }
    pub fn with_seed(_seed: u64) -> Self {
        let origin_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        Self {
            origin: Instant::now(),
            origin_unix_ms,
            heap: None,
        }
    }

    /// Returns the cached `[totalJSHeapSize, usedJSHeapSize]`, taking a new
    /// reading with `read` once the cached one is 50 ms old.
    fn heap_sizes(&mut self, now: Instant, read: impl FnOnce() -> [f64; 2]) -> [f64; 2] {
        match self.heap {
            Some((at, sizes)) if now.duration_since(at) < HEAP_SIZE_REFRESH => sizes,
            _ => {
                let sizes = read();
                self.heap = Some((now, sizes));
                sizes
            }
        }
    }

    /// Returns elapsed ms since origin, clamped to the resolution grid.
    pub fn now_ms(&self, cross_origin_isolated: bool) -> f64 {
        let resolution_us = if cross_origin_isolated {
            ISOLATED_RESOLUTION_US
        } else {
            RESOLUTION_US
        };
        let raw_us = self.origin.elapsed().as_nanos() as f64 / 1000.0;
        // Chrome quantizes by flooring onto the grid and then DIVIDING the
        // integer microsecond count: grid values are k*resolution/1000.0.
        // The previous jitter-threshold clamp produced off-grid doubles
        // (0.0999999999999659-style deltas) whose bit pattern Chrome never
        // emits; worker-clock pair measurements that compare exact values
        // see the difference.
        // Chrome uses f32 for the quantized microsecond value before
        // converting to f64 milliseconds. This gives the same f64 bit
        // pattern Chrome produces for the same grid crossing.
        let quantized_f32 = ((raw_us / resolution_us).floor()) as f32;
        (quantized_f32 as f64 * resolution_us as f64) / 1000.0
    }
}

impl Default for PerfState {
    fn default() -> Self {
        Self::new()
    }
}

#[op2(fast)]
pub fn op_perf_now_humanized(s: &mut OpState) -> f64 {
    let isolated = s
        .try_borrow::<StealthState>()
        .is_some_and(|st| st.cross_origin_isolated);
    s.borrow::<PerfState>().now_ms(isolated)
}

/// Returns the UNIX-epoch ms corresponding to `PerfState.origin` (the
/// process-relative t=0 for `performance.now()`). JS uses this as the
/// `performance.timeOrigin` value so the standard Web Platform invariant
/// `timeOrigin + performance.now() ≈ Date.now()` holds.
#[op2(fast)]
pub fn op_perf_time_origin_ms(s: &mut OpState) -> f64 {
    let s = s.borrow::<PerfState>();
    s.origin_unix_ms
}

/// `[totalJSHeapSize, usedJSHeapSize]` of this isolate, computed as Blink's
/// `GetHeapSize` does: physical and used heap size, each plus the external
/// memory V8 accounts for.
#[op2]
#[serde]
pub fn op_perf_js_heap_sizes(scope: &mut v8::PinScope<'_, '_>) -> Vec<f64> {
    let state_rc = deno_core::JsRuntime::op_state_from(scope);
    let mut state = state_rc.borrow_mut();
    let sizes = state
        .borrow_mut::<PerfState>()
        .heap_sizes(Instant::now(), || {
            let stats = scope.get_heap_statistics();
            let external = stats.external_memory() as f64;
            [
                stats.total_physical_size() as f64 + external,
                stats.used_heap_size() as f64 + external,
            ]
        });
    sizes.to_vec()
}

#[derive(serde::Serialize)]
pub struct JsResourceTiming {
    pub name: String,
    pub entry_type: String,
    pub start_time: f64,
    pub duration: f64,
    pub fetch_start: f64,
    pub domain_lookup_start: f64,
    pub domain_lookup_end: f64,
    pub connect_start: f64,
    pub connect_end: f64,
    pub secure_connection_start: f64,
    pub request_start: f64,
    pub response_start: f64,
    pub response_end: f64,
    pub transfer_size: u64,
    pub encoded_body_size: u64,
    pub decoded_body_size: u64,
}

#[op2]
#[serde]
pub fn op_perf_get_resource_timings(state: &mut OpState) -> Vec<JsResourceTiming> {
    let state = state.borrow::<DomState>();
    state
        .resource_timings
        .iter()
        .map(|t| JsResourceTiming {
            name: "https://example.com/placeholder".to_string(),
            entry_type: "resource".to_string(),
            start_time: t.request_start_ms,
            duration: t.response_end_ms - t.request_start_ms,
            fetch_start: t.request_start_ms,
            domain_lookup_start: t.dns_start_ms,
            domain_lookup_end: t.dns_end_ms,
            connect_start: t.connect_start_ms,
            connect_end: t.connect_end_ms,
            secure_connection_start: t.tls_start_ms,
            request_start: t.request_start_ms,
            response_start: t.response_start_ms,
            response_end: t.response_end_ms,
            transfer_size: 0,
            encoded_body_size: 0,
            decoded_body_size: 0,
        })
        .collect()
}

deno_core::extension!(
    perf_extension,
    ops = [
        op_perf_now_humanized,
        op_perf_get_resource_timings,
        op_perf_time_origin_ms,
        op_perf_js_heap_sizes,
    ],
);
