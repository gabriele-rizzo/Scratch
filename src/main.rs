use std::io;

use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
};
use tuimon::App;

mod runners;
mod screens;
mod ui;
mod utils;

fn main() -> Result<()> {
    // Ticks at the spinner's frame rate, which is also plenty for live output.
    let mut app = App::new(screens::Language::new())?.with_tick_rate(ui::SPINNER_INTERVAL);

    // Without this, a paste arrives as one key press per character.
    execute!(io::stdout(), EnableBracketedPaste)?;
    let result = app.run();
    let _ = execute!(io::stdout(), DisableBracketedPaste);

    result
}
