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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_new_lines_by_default() {
        let mut scroll = Scroll::default();
        assert_eq!(scroll.update(5, 10), 0);
        assert_eq!(scroll.update(30, 10), 20);
        assert_eq!(scroll.update(40, 10), 30);
    }

    #[test]
    fn scrolling_up_stops_following() {
        let mut scroll = Scroll::default();
        scroll.update(30, 10);
        scroll.up(5);
        assert_eq!(scroll.update(30, 10), 15);
        // New output doesn't move the view.
        assert_eq!(scroll.update(50, 10), 15);
    }

    #[test]
    fn reaching_the_bottom_follows_again() {
        let mut scroll = Scroll::default();
        scroll.update(30, 10);
        scroll.up(5);
        scroll.update(30, 10);
        scroll.down(100);
        assert_eq!(scroll.update(30, 10), 20);
        assert_eq!(scroll.update(40, 10), 30);
    }

    #[test]
    fn pages_by_half_the_viewport() {
        let mut scroll = Scroll::default();
        scroll.update(100, 10);
        scroll.page_up();
        assert_eq!(scroll.update(100, 10), 85);
        scroll.page_down();
        assert_eq!(scroll.update(100, 10), 90);
    }

    #[test]
    fn follow_jumps_back_to_the_bottom() {
        let mut scroll = Scroll::default();
        scroll.update(100, 10);
        scroll.up(50);
        scroll.update(100, 10);
        scroll.follow();
        assert_eq!(scroll.update(120, 10), 110);
    }

    #[test]
    fn clamps_when_output_shrinks() {
        let mut scroll = Scroll::default();
        scroll.update(100, 10);
        scroll.up(10);
        scroll.update(100, 10);
        assert_eq!(scroll.update(20, 10), 10);
    }
}
