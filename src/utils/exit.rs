use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tuimon::ScreenAction;

pub fn is_ctrl_c(event: &Event) -> bool {
    matches!(
        event,
        Event::Key(KeyEvent {
            code: KeyCode::Char('c'),
            modifiers,
            ..
        }) if modifiers.contains(KeyModifiers::CONTROL)
    )
}

pub fn handle_exit_input(event: &Event) -> Option<ScreenAction> {
    if is_ctrl_c(event) {
        return Some(ScreenAction::Quit);
    }

    None
}
