//! The sandboxed Rhai engine: limits, cancellation, and the functions cells may call.
//!
//! The engine registers no file, network or process functions. Its only host
//! functions are output capture and `uniform`, and `uniform` is deterministic.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rhai::{Dynamic, Engine};

use crate::error::{ComputeError, Result};

/// The language name a code cell uses to run here.
pub const LANGUAGE: &str = "rhai";

/// Resource limits for one cell's evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Operations a cell may perform before it is stopped.
    pub max_operations: u64,
    /// Wall-clock time a cell may run before it is stopped.
    pub timeout: Duration,
    /// Largest string a cell may build, in bytes.
    pub max_string_bytes: usize,
    /// Most elements an array or map may hold.
    pub max_collection_len: usize,
    /// Deepest function call nesting.
    pub max_call_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_operations: 10_000_000,
            timeout: Duration::from_secs(10),
            max_string_bytes: 1024 * 1024,
            max_collection_len: 100_000,
            max_call_depth: 64,
        }
    }
}

impl Limits {
    /// Reject limits that would stop every cell before it starts.
    pub fn validate(&self) -> Result<()> {
        if self.max_operations == 0 {
            return Err(ComputeError::InvalidLimit("max_operations is zero".into()));
        }
        if self.timeout.is_zero() {
            return Err(ComputeError::InvalidLimit("timeout is zero".into()));
        }
        if self.max_string_bytes == 0 || self.max_collection_len == 0 || self.max_call_depth == 0 {
            return Err(ComputeError::InvalidLimit("a size limit is zero".into()));
        }
        Ok(())
    }
}

/// Lets another thread stop a run. Clones share the flag.
#[derive(Debug, Clone, Default)]
pub struct Control {
    cancelled: Arc<AtomicBool>,
}

impl Control {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the run to stop. The current cell is stopped at its next check.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Build an engine for one cell. `prints` receives everything the cell prints.
pub(crate) fn build(
    limits: &Limits,
    control: &Control,
    deadline: Instant,
    seed: u64,
    prints: Rc<RefCell<Vec<String>>>,
) -> Engine {
    let mut engine = Engine::new();
    engine.set_max_operations(limits.max_operations);
    engine.set_max_string_size(limits.max_string_bytes);
    engine.set_max_array_size(limits.max_collection_len);
    engine.set_max_map_size(limits.max_collection_len);
    engine.set_max_call_levels(limits.max_call_depth);

    let stop = control.clone();
    engine.on_progress(move |_operations| {
        if stop.is_cancelled() {
            Some(Dynamic::from("cancelled"))
        } else if Instant::now() >= deadline {
            Some(Dynamic::from("timeout"))
        } else {
            None
        }
    });

    crate::bindings::register(&mut engine, seed, limits.clone());

    let print_sink = prints.clone();
    engine.on_print(move |text| print_sink.borrow_mut().push(text.to_string()));
    engine.on_debug(move |text, _source, _position| prints.borrow_mut().push(text.to_string()));

    engine
}

/// A deterministic value in `[0, 1)` for `index` under `seed`.
pub(crate) fn uniform_at(seed: u64, index: i64) -> f64 {
    let bits = splitmix64(seed ^ splitmix64(index as u64));
    (bits >> 11) as f64 / (1u64 << 53) as f64
}

pub(crate) fn splitmix64(input: u64) -> u64 {
    let mut z = input.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_is_deterministic_and_in_range() {
        for i in 0..1000 {
            let v = uniform_at(42, i);
            assert!((0.0..1.0).contains(&v));
            assert_eq!(v, uniform_at(42, i));
        }
        assert_ne!(uniform_at(1, 0), uniform_at(2, 0));
    }

    #[test]
    fn default_limits_are_valid() {
        assert!(Limits::default().validate().is_ok());
        let zero = Limits {
            max_operations: 0,
            ..Limits::default()
        };
        assert!(zero.validate().is_err());
    }
}
