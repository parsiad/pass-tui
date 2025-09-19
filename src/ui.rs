use crate::app::{App, Modal, PendingAction, PreviewMode};
use crate::store::{path_to_store_key, StoreEntry};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Terminal;
use std::io;
use std::time::Duration;

pub fn run_tui(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run(app, &mut terminal);

    disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    terminal.show_cursor()?;

    res
}

fn run(app: &mut App, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let poll_timeout = Duration::from_millis(500);
    app.apply_filter();
    app.update_preview();
    let mut needs_redraw = true;
    loop {
        if needs_redraw {
            terminal.draw(|f| draw_ui(f, app))?;
            needs_redraw = false;
        }

        if crossterm::event::poll(poll_timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    if handle_key(app, key)? {
                        needs_redraw = true;
                    }
                }
                Event::Resize(width, height) => {
                    terminal.resize(Rect::new(0, 0, width, height))?;
                    needs_redraw = true;
                }
                _ => {}
            }
        }

        // Run any pending actions. Suspend only for interactive ones (edit/add).
        if let Some(action) = app.pending.take() {
            let res = match action {
                PendingAction::Edit(_) | PendingAction::Add(_) => {
                    suspend_and_run(terminal, || run_action(app, action))
                }
                _ => run_action(app, action),
            };
            if let Err(e) = res {
                app.status = Some(e.to_string());
            }
            if let Err(e) = app.refresh() {
                app.status = Some(e.to_string());
            }
            app.update_preview();
            needs_redraw = true;
        }

        if let Some((rel, mode)) = app.take_pending_preview() {
            let qr = mode == PreviewMode::Qr;
            let backend = app.backend.as_ref();
            let entry_for_unlock = rel.clone();
            let unlock_result =
                suspend_and_run(terminal, move || backend.unlock(&entry_for_unlock, qr));
            if let Err(e) = unlock_result {
                app.status = Some(e.to_string());
            }
            if let Err(e) = app.load_preview_after_unlock(rel, mode) {
                app.status = Some(e.to_string());
            }
            needs_redraw = true;
        }

        if app.quit {
            break;
        }
    }
    Ok(())
}

fn draw_ui(f: &mut ratatui::Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(2)])
        .split(f.size());

    // Breadcrumb and header right content (help or filter)
    let breadcrumb = app
        .cwd
        .iter()
        .filter_map(|c| c.to_str())
        .collect::<Vec<_>>()
        .join("/");
    let header_right = if app.filter_mode || !app.filter.is_empty() {
        Line::from(vec![
            Span::raw(" ["),
            Span::styled("Filter:", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(" "),
            Span::styled(
                if app.filter_mode {
                    app.filter_input.as_str()
                } else {
                    app.filter.as_str()
                },
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("]"),
        ])
    } else if let Some(msg) = &app.status {
        Line::from(vec![Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Yellow),
        )])
    } else {
        Line::from("[/] filter  [a] add  [c] qr code  [d] delete  [e] edit  [enter] view  [h/l/←/→] collapse/expand  [j/k/↑/↓] move  [q] quit  [r] rename  [y] yank")
    };
    f.render_widget(Clear, chunks[0]);
    let header = Paragraph::new(Line::from(vec![
        Span::raw("pass-tui  "),
        Span::raw(breadcrumb),
        Span::raw("  "),
    ]))
    .wrap(Wrap { trim: true });
    f.render_widget(header, chunks[0]);
    // Render the right-side content by drawing another Paragraph overlaid aligned to right
    let right = Paragraph::new(header_right).wrap(Wrap { trim: true });
    f.render_widget(right, chunks[0]);

    // Body: list + raw preview
    f.render_widget(Clear, chunks[1]);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| {
            render_row(
                &app.entries[row.idx],
                &row.branches,
                app.filter_mode,
                if app.filter_mode {
                    app.filter_input.as_str()
                } else {
                    app.filter.as_str()
                },
            )
        })
        .collect();
    let store_title = app.store_dir.to_string_lossy().into_owned();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(store_title))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    let mut state = list_state(app);
    f.render_stateful_widget(list, body[0], &mut state);

    let mut style = Style::default();
    let current_sel = app.selected_entry_path();
    let mut raw_text: String = String::new();
    if let (Some(sel), Some(prev)) = (current_sel.as_ref(), app.preview_key.as_ref()) {
        if sel == prev {
            raw_text = app.preview_text.clone();
        }
    }
    if raw_text.is_empty() {
        raw_text = "Press Enter (or C for QR code) to view selected file".to_string();
        style = style.fg(Color::DarkGray);
    } else if app.preview_is_error {
        style = style.fg(Color::Red);
    }
    let raw = Paragraph::new(raw_text)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Preview"))
        .style(style);
    f.render_widget(raw, body[1]);

    // Footer removed to avoid persistent bottom line

    // Modal overlay
    if let Some(m) = &app.modal {
        let area = centered_rect(60, 40, f.size());
        f.render_widget(Clear, area); // clear the area beneath
        match m {
            Modal::Input { title, buffer, .. } => {
                let block = Block::default()
                    .title(title.as_str())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan));
                let text = Paragraph::new(vec![
                    Line::from(buffer.as_str()),
                    Line::from(Span::styled(
                        "Enter to create, Esc to cancel",
                        Style::default().fg(Color::DarkGray),
                    )),
                ])
                .wrap(Wrap { trim: false })
                .block(block);
                f.render_widget(text, area);
            }
            Modal::Confirm {
                title,
                message,
                selected_ok,
                ..
            } => {
                let block = Block::default()
                    .title(title.as_str())
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red));
                // Render message and buttons
                let msg = Paragraph::new(message.as_str()).wrap(Wrap { trim: true });
                f.render_widget(block, area);
                let inner = area.inner(&ratatui::layout::Margin {
                    vertical: 1,
                    horizontal: 2,
                });
                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1), Constraint::Length(1)])
                    .split(inner);
                f.render_widget(msg, rows[0]);
                // Buttons
                let ok_style = if *selected_ok {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                let cancel_style = if !*selected_ok {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                let buttons = Paragraph::new(Line::from(vec![
                    Span::styled("[ OK ]  ", ok_style),
                    Span::styled("[ Cancel ]", cancel_style),
                ]));
                f.render_widget(buttons, rows[1]);
            }
        }
    }
}

fn render_row(
    e: &StoreEntry,
    branches: &[bool],
    filter_active: bool,
    filter: &str,
) -> ListItem<'static> {
    let mut prefix = String::new();
    if let Some((&is_last, parents)) = branches.split_last() {
        for branch in parents {
            prefix.push_str(if *branch { "   " } else { "│  " });
        }
        prefix.push_str(if is_last { "└─ " } else { "├─ " });
    }

    let icon = if e.is_dir() { "📁 " } else { "📄 " };
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(4);
    spans.push(Span::raw(prefix));
    spans.push(Span::raw(icon.to_string()));

    let name = e.display_name();
    if filter_active && !filter.is_empty() {
        let highlight = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        spans.extend(highlight_matches(&name, filter, highlight));
    } else {
        spans.push(Span::raw(name));
    }

    if e.is_dir() {
        spans.push(Span::raw("/".to_string()));
    }

    ListItem::new(Line::from(spans))
}

fn highlight_matches(name: &str, needle: &str, highlight: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = name;

    while let Some(pos) = rest.find(needle) {
        let (head, tail) = rest.split_at(pos);
        if !head.is_empty() {
            spans.push(Span::raw(head.to_owned()))
        }
        let (matched, remaining) = tail.split_at(needle.len());
        spans.push(Span::styled(matched.to_owned(), highlight));
        rest = remaining;
    }

    if !rest.is_empty() {
        spans.push(Span::raw(rest.to_owned()));
    }

    spans
}

fn list_state(app: &App) -> ratatui::widgets::ListState {
    let mut state = ratatui::widgets::ListState::default();
    let len = app.rows.len();
    if len > 0 {
        state.select(Some(app.cursor.min(len - 1)));
    }
    state
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    if handle_modal_key(app, key)? {
        return Ok(true);
    }

    if let Some(redraw) = handle_filter_key(app, key) {
        return Ok(redraw);
    }

    let mut changed = false;
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => app.quit = true,
        KeyCode::Down | KeyCode::Char('j') => {
            if app.cursor + 1 < app.rows.len() {
                app.cursor += 1;
                changed = true;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.cursor > 0 {
                app.cursor -= 1;
                changed = true;
            }
        }
        KeyCode::Enter => {
            if app.selected_entry_path().is_some() {
                app.update_preview();
            } else {
                app.enter();
            }
            changed = true;
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if app.selected_entry_path().is_some() {
                app.update_preview_qr();
                changed = true;
            }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if let Some(row) = app.rows.get(app.cursor) {
                let entry = &app.entries[row.idx];
                if entry.is_dir() {
                    let relative = entry.path.strip_prefix(&app.cwd).unwrap_or(&entry.path);
                    let key = path_to_store_key(relative);
                    if app.expanded.contains(&key) {
                        app.expanded.remove(&key);
                        app.apply_filter();
                        changed = true;
                    }
                }
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if let Some(row) = app.rows.get(app.cursor) {
                let entry = &app.entries[row.idx];
                if entry.is_dir() {
                    let relative = entry.path.strip_prefix(&app.cwd).unwrap_or(&entry.path);
                    let key = path_to_store_key(relative);
                    if !app.expanded.contains(&key) {
                        app.expanded.insert(key);
                        app.apply_filter();
                        changed = true;
                    }
                }
            }
        }
        KeyCode::Char('/') => {
            app.filter_mode = true;
            app.filter_input = app.filter.clone();
            changed = true;
        }
        KeyCode::Esc => {
            app.filter.clear();
            app.apply_filter();
            app.status = None;
            changed = true;
        }
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            if let Some(rel) = app.selected_entry_path() {
                if let Err(e) = app.backend.yank(&rel) {
                    app.status = Some(e.to_string());
                }
                changed = true;
            }
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            if let Some(rel) = app.selected_entry_path() {
                app.pending = Some(PendingAction::Edit(rel));
                changed = true;
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            app.open_rename_modal();
            changed = true;
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            app.open_add_modal();
            changed = true;
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            app.open_delete_modal();
            changed = true;
        }
        _ => {}
    }
    Ok(changed)
}

fn handle_modal_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    let Some(_) = app.modal else {
        return Ok(false);
    };

    let mut submit = false;
    let mut dismiss = false;

    {
        let modal = app.modal.as_mut().expect("checked modal exists");
        match modal {
            Modal::Input { buffer, .. } => match key.code {
                KeyCode::Esc => dismiss = true,
                KeyCode::Enter => submit = true,
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    buffer.push(c);
                }
                _ => {}
            },
            Modal::Confirm { selected_ok, .. } => match key.code {
                KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                    *selected_ok = !*selected_ok;
                }
                KeyCode::Esc => dismiss = true,
                KeyCode::Enter => submit = true,
                _ => {}
            },
        }
    }

    if dismiss {
        app.modal = None;
        return Ok(true);
    }

    if submit {
        if let Some(action) = app.submit_modal() {
            app.pending = Some(action);
        }
        return Ok(true);
    }

    Ok(true)
}

fn handle_filter_key(app: &mut App, key: KeyEvent) -> Option<bool> {
    if !app.filter_mode {
        return None;
    }

    match key.code {
        KeyCode::Esc => {
            app.filter_mode = false;
            app.filter.clear();
            app.filter_input.clear();
            app.apply_filter();
        }
        KeyCode::Enter => {
            app.filter = app.filter_input.clone();
            app.filter_mode = false;
            app.apply_filter();
        }
        KeyCode::Backspace => {
            app.filter_input.pop();
        }
        KeyCode::Char(c) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
            app.filter_input.push(c);
        }
        _ => {}
    }

    Some(true)
}

fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    r: ratatui::layout::Rect,
) -> ratatui::layout::Rect {
    use ratatui::layout::Margin;
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1]);

    horizontal[1].inner(&Margin {
        vertical: 1,
        horizontal: 2,
    })
}

fn suspend_and_run<F>(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, f: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    // leave raw mode and alt screen
    disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    )?;
    let result = f();
    // re-enter
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::cursor::Hide
    )?;
    enable_raw_mode()?;
    // ensure a clean screen on resume
    terminal.clear()?;
    result
}

fn run_action(app: &mut App, action: PendingAction) -> Result<()> {
    match action {
        PendingAction::Edit(rel) => app.backend.edit(&rel),
        PendingAction::Add(path) => app.backend.add(&path),
        PendingAction::Delete => app.delete_selected(),
        PendingAction::Rename { from, to } => app.backend.mv(&from, &to),
    }
}
