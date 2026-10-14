//! Event detection and root-finding during integration.
//!
//! An [`Event`] is a scalar function `g(t, y)`; the solver reports every time
//! `g` changes sign along the trajectory, located to round-off by bisection on
//! the step's cubic-Hermite dense output. Terminal events stop the integration
//! at the crossing. Use [`OdeProblem::solve_events`](crate::OdeProblem::solve_events)
//! to run with events.

use std::fmt;

/// Which sign change of an event function counts as a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Direction {
    /// Only `g` crossing from negative to positive.
    Rising,
    /// Only `g` crossing from positive to negative.
    Falling,
    /// Any sign change.
    Both,
}

/// A scalar event function `g(t, y)` watched during integration.
/// Event function signature: `g(t, y)`.
type EventFn = dyn Fn(f64, &[f64]) -> f64;

pub struct Event {
    g: Box<EventFn>,
    direction: Direction,
    terminal: bool,
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Event")
            .field("direction", &self.direction)
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
    }
}

impl Event {
    /// Create a non-terminal event that fires on any sign change of `g`.
    pub fn new<G>(g: G) -> Self
    where
        G: Fn(f64, &[f64]) -> f64 + 'static,
    {
        Self {
            g: Box::new(g),
            direction: Direction::Both,
            terminal: false,
        }
    }

    /// Restrict the event to crossings in the given [`Direction`].
    #[must_use]
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// Mark the event as terminal: integration stops at the first hit.
    #[must_use]
    pub fn terminal(mut self) -> Self {
        self.terminal = true;
        self
    }

    /// Evaluate the event function at `(t, y)`.
    pub(crate) fn eval(&self, t: f64, y: &[f64]) -> f64 {
        (self.g)(t, y)
    }

    pub(crate) fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// Whether a crossing from `g_old` to `g_new` matches this event's direction.
    /// Returns the crossing direction when it is a hit.
    pub(crate) fn crossing(&self, g_old: f64, g_new: f64) -> Option<Direction> {
        // A crossing needs a strict sign change, or landing exactly on zero
        // from a non-zero value. Starting exactly on zero does not count.
        let rising = g_old < 0.0 && g_new >= 0.0;
        let falling = g_old > 0.0 && g_new <= 0.0;
        let dir = if rising {
            Direction::Rising
        } else if falling {
            Direction::Falling
        } else {
            return None;
        };
        match self.direction {
            Direction::Both => Some(dir),
            wanted if wanted == dir => Some(dir),
            _ => None,
        }
    }
}

/// One detected event crossing.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EventHit {
    /// Index of the event in the slice passed to `solve_events`.
    pub index: usize,
    /// Time of the crossing, located to bisection precision.
    pub t: f64,
    /// Interpolated state at the crossing.
    pub y: Vec<f64>,
    /// Direction of the sign change.
    pub direction: Direction,
}

/// Result of an integration with events.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EventSolution {
    /// Time at which integration stopped: `t_final`, or the time of the first
    /// terminal event.
    pub t_end: f64,
    /// State at `t_end`.
    pub y_end: Vec<f64>,
    /// Every event crossing up to `t_end`, in time order.
    pub hits: Vec<EventHit>,
}

/// Locate the root of `h` on `[a, b]` by bisection, given that `h(a)` and `h(b)`
/// have opposite signs (or `h(b) == 0`). Returns the time of the root.
///
/// Bisection stops when the bracket is within a few ulps of `|t|`, or after a
/// fixed iteration cap, so it always terminates.
pub(crate) fn bisect_root(mut a: f64, mut ha: f64, mut b: f64, h: impl Fn(f64) -> f64) -> f64 {
    for _ in 0..200 {
        let tol = 4.0 * f64::EPSILON * a.abs().max(b.abs()).max(f64::MIN_POSITIVE);
        if (b - a).abs() <= tol {
            break;
        }
        let m = 0.5 * (a + b);
        let hm = h(m);
        if hm == 0.0 {
            return m;
        }
        if (hm < 0.0) == (ha < 0.0) {
            a = m;
            ha = hm;
        } else {
            b = m;
        }
    }
    // Return the endpoint on which the function is closer to zero.
    let hb = h(b);
    if hb.abs() <= ha.abs() { b } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_respects_direction() {
        let rising = Event::new(|_, _| 0.0).direction(Direction::Rising);
        assert_eq!(rising.crossing(-1.0, 1.0), Some(Direction::Rising));
        assert_eq!(rising.crossing(1.0, -1.0), None);
        assert_eq!(rising.crossing(1.0, 1.0), None);

        let any = Event::new(|_, _| 0.0);
        assert_eq!(any.crossing(1.0, -1.0), Some(Direction::Falling));
        assert_eq!(any.crossing(0.0, 1.0), None);
    }

    #[test]
    fn bisection_locates_sqrt2() {
        let root = bisect_root(1.0, -1.0, 2.0, |t| t * t - 2.0);
        assert!((root - 2f64.sqrt()).abs() < 1e-12);
    }
}
