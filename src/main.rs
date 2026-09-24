use anyhow::Result;
use tuimon::App;

mod runners;
mod screens;
mod ui;
mod utils;

fn main() -> Result<()> {
    App::new(screens::Language::new())?.run()
}
