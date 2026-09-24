use anyhow::Result;
use tuimon::App;

mod runners;
mod screens;
mod ui;
mod utils;

fn main() -> Result<()> {
    // Ticks at the spinner's frame rate, which is also plenty for live output.
    App::new(screens::Language::new())?
        .with_tick_rate(ui::SPINNER_INTERVAL)
        .run()
}
