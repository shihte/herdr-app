//! SiaoDesk: layout nodes glide to a new size instead of jumping. `layout.resize` between two pixel
//! sizes, and `layout.show` / `layout.hide` of a pixel-sized node (it grows from / shrinks to 0), run
//! a short ease-out. The layout keeps the target size; rendering reads the in-between one.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const DURATION: Duration = Duration::from_millis(200);

struct Glide {
    from: f32,
    to: f32,
    start: Instant,
}

thread_local! {
    static GLIDES: RefCell<HashMap<String, Glide>> = RefCell::new(HashMap::new());
    static GENERATION: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

/// Starts (or restarts, from where it is now) a glide of node `id`; returns its generation, so a
/// delayed step (hiding at the end) can tell whether a newer glide replaced it.
pub fn start(id: &str, from: f32, to: f32) -> u64 {
    let from = current(id).unwrap_or(from);
    GLIDES.with(|g| g.borrow_mut().insert(id.to_owned(), Glide { from, to, start: Instant::now() }));
    BUSY.with(|b| b.set(true));
    GENERATION.with(|g| {
        let mut g = g.borrow_mut();
        let n = g.entry(id.to_owned()).or_insert(0);
        *n += 1;
        *n
    })
}

pub fn generation(id: &str) -> u64 {
    GENERATION.with(|g| g.borrow().get(id).copied().unwrap_or(0))
}

/// The size node `id` has right now, while it glides.
pub fn current(id: &str) -> Option<f32> {
    GLIDES.with(|g| {
        let mut g = g.borrow_mut();
        let glide = g.get(id)?;
        let t = (glide.start.elapsed().as_secs_f32() / DURATION.as_secs_f32()).min(1.0);
        if t >= 1.0 {
            g.remove(id);
            return None;
        }
        BUSY.with(|b| b.set(true));
        let eased = 1.0 - (1.0 - t).powi(3);
        Some(glide.from + (glide.to - glide.from) * eased)
    })
}

/// Whether anything glided during the last frame (then the next frame should come right away).
pub fn take_busy() -> bool {
    BUSY.with(|b| b.replace(false))
}
