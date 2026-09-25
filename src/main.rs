use anyhow::Result;
use tuimon::{App, MouseMode};

mod runners;
mod screens;
mod ui;
mod utils;

fn main() -> Result<()> {
    // Clicks, drags and the wheel are all Scratch uses; plain movement would only
    // cause redraws.
    App::new(screens::Language::new())?
        .with_mouse(MouseMode::ClickDrag)?
        .run()
}
