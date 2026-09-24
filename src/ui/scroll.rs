/// Vertical scroll position that sticks to the bottom until the user scrolls up.
pub struct Scroll {
    offset: usize,
    follow: bool,
    viewport: usize,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            offset: 0,
            follow: true,
            viewport: 0,
        }
    }
}

impl Scroll {
    /// Clamps the offset for `total` lines in a `viewport` tall area and returns it.
    pub fn update(&mut self, total: usize, viewport: usize) -> usize {
        let max = total.saturating_sub(viewport);
        self.viewport = viewport;

        if self.follow || self.offset >= max {
            self.offset = max;
            self.follow = true;
        }

        self.offset
    }

    pub fn up(&mut self, lines: usize) {
        self.offset = self.offset.saturating_sub(lines);
        self.follow = false;
    }

    pub fn down(&mut self, lines: usize) {
        // `update` clamps and re-enables following at the bottom.
        self.offset = self.offset.saturating_add(lines);
    }

    /// Jumps to the end and follows new lines again.
    pub fn follow(&mut self) {
        self.follow = true;
    }

    pub fn page_up(&mut self) {
        self.up((self.viewport / 2).max(1));
    }

    pub fn page_down(&mut self) {
        self.down((self.viewport / 2).max(1));
    }
}
