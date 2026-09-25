use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use nib_core::{Editor, Grid, KeyCode, KeyEvent, Modifiers};

use crate::draw;

pub fn run(editor: &mut Editor) -> io::Result<()> {
    let _guard = TerminalGuard::enter()?;
    let (width, height) = terminal::size()?;
    editor.resize(width, height);

    let mut stdout = io::stdout().lock();
    let mut prev = Grid::default();
    let mut prev_cursor = None;
    let mut next = Grid::default();
    let mut out = Vec::new();
    loop {
        let cursor = editor.render(&mut next);
        if next != prev || cursor != prev_cursor {
            out.clear();
            draw::draw(&mut out, &prev, &next, cursor)?;
            stdout.write_all(&out)?;
            stdout.flush()?;
            std::mem::swap(&mut prev, &mut next);
            prev_cursor = cursor;
        }
        // Work left for after the frame, such as highlighting a file just
        // opened, then draw again before waiting for keys.
        if editor.catch_up() {
            continue;
        }

        // Wait for input, but only until the next timer is due.
        if let Some(due) = editor.next_timer()
            && !event::poll(due.saturating_duration_since(Instant::now()))?
        {
            editor.run_timers();
            if editor.should_quit() {
                return Ok(());
            }
            continue;
        }
        handle(editor, event::read()?);
        // Handle everything already queued, then draw once.
        while event::poll(Duration::ZERO)? {
            handle(editor, event::read()?);
        }
        if editor.should_quit() {
            return Ok(());
        }
    }
}

fn handle(editor: &mut Editor, event: Event) {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            if let Some(key) = convert_key(key) {
                editor.handle_key(key);
            }
        }
        Event::Resize(width, height) => editor.resize(width, height),
        _ => {}
    }
}

fn convert_key(key: event::KeyEvent) -> Option<KeyEvent> {
    use event::KeyCode as K;

    let code = match key.code {
        K::Char(c) => KeyCode::Char(c),
        K::Enter => KeyCode::Enter,
        K::Esc => KeyCode::Escape,
        K::Tab | K::BackTab => KeyCode::Tab,
        K::Backspace => KeyCode::Backspace,
        K::Delete => KeyCode::Delete,
        K::Up => KeyCode::Up,
        K::Down => KeyCode::Down,
        K::Left => KeyCode::Left,
        K::Right => KeyCode::Right,
        K::Home => KeyCode::Home,
        K::End => KeyCode::End,
        K::PageUp => KeyCode::PageUp,
        K::PageDown => KeyCode::PageDown,
        K::F(n) => KeyCode::F(n),
        _ => return None,
    };
    let m = key.modifiers;
    Some(KeyEvent {
        code,
        modifiers: Modifiers {
            ctrl: m.contains(KeyModifiers::CONTROL),
            alt: m.contains(KeyModifiers::ALT),
            // A char already says whether it is shifted ('Q', '!'), so keymaps
            // never have to tell "Q" from "S-q".
            shift: !matches!(code, KeyCode::Char(_))
                && (m.contains(KeyModifiers::SHIFT) || key.code == K::BackTab),
            super_: m.contains(KeyModifiers::SUPER),
        },
    })
}

/// Puts the terminal into raw mode on the alternate screen, and restores it
/// when dropped or when the program panics.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = TerminalGuard;
        execute!(io::stdout(), terminal::EnterAlternateScreen)?;
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            default_hook(info);
        }));
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let _ = execute!(
        io::stdout(),
        cursor::SetCursorStyle::DefaultUserShape,
        cursor::Show,
        terminal::LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: event::KeyCode, modifiers: KeyModifiers) -> Option<KeyEvent> {
        convert_key(event::KeyEvent::new(code, modifiers))
    }

    #[test]
    fn chars_drop_shift() {
        let converted = key(event::KeyCode::Char('Q'), KeyModifiers::SHIFT).unwrap();
        assert_eq!(converted, KeyEvent::new(KeyCode::Char('Q')));
        assert_eq!(
            key(event::KeyCode::Char('q'), KeyModifiers::CONTROL),
            Some(KeyEvent::ctrl('q'))
        );
    }

    #[test]
    fn back_tab_is_shift_tab() {
        let converted = key(event::KeyCode::BackTab, KeyModifiers::SHIFT).unwrap();
        assert_eq!(converted.code, KeyCode::Tab);
        assert!(converted.modifiers.shift);
    }
}
