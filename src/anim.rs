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
    /// Set by the first frame that draws it, not by the command: a frame that comes late (a mod's
    /// command arrives asynchronously, a busy frame before it) would otherwise eat the glide and it
    /// jumps — or doesn't show at all.
    start: Option<Instant>,
    /// When the command asked for it: a node that is never drawn still ends its glide on time.
    made: Instant,
}

impl Glide {
    fn t(&self) -> f32 {
        (self.start.unwrap_or(self.made).elapsed().as_secs_f32() / DURATION.as_secs_f32()).min(1.0)
    }
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
    GLIDES.with(|g| g.borrow_mut().insert(id.to_owned(), Glide { from, to, start: None, made: Instant::now() }));
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
        let glide = g.get_mut(id)?;
        if glide.start.is_none() {
            // Late by more than a few frames: the glide still plays whole, from now.
            glide.start = Some(if glide.made.elapsed() < DURATION { Instant::now() } else { glide.made });
        }
        let t = glide.t();
        if t >= 1.0 {
            g.remove(id);
            return None;
        }
        BUSY.with(|b| b.set(true));
        let eased = 1.0 - (1.0 - t).powi(3);
        Some(glide.from + (glide.to - glide.from) * eased)
    })
}

/// Whether some glide is still under way. Terminals keep their size meanwhile and fit once it ends:
/// resizing one every frame makes the program in it redraw its whole screen each time (it stutters).
pub fn gliding() -> bool {
    GLIDES.with(|g| g.borrow().values().any(|glide| glide.t() < 1.0))
}

/// Whether node `id` is still gliding (a hide marks it hidden only after this turns false).
pub fn active(id: &str) -> bool {
    GLIDES.with(|g| g.borrow().get(id).is_some_and(|glide| glide.t() < 1.0))
}

/// Whether anything glided during the last frame (then the next frame should come right away).
pub fn take_busy() -> bool {
    BUSY.with(|b| b.replace(false))
}
