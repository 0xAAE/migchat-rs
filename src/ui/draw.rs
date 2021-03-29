use super::{App, Widget, WidgetState};
use tui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use tui_logger::{TuiLoggerSmartWidget, TuiLoggerWidget};

pub fn draw<B: Backend>(f: &mut Frame<B>, app: &mut App) {
    //
    // styles
    //
    let caption_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let selected_style = Style::default()
        .bg(Color::LightGreen)
        .add_modifier(Modifier::BOLD);
    //
    // layout
    //
    let rows = Layout::default()
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(10),
            ]
            .as_ref(),
        )
        .split(f.size());
    //
    // title
    //
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(&app.title, caption_style));
    let paragraph = Paragraph::new(app.current_user.as_str())
        .block(block)
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, rows[0]);
    //
    // working area
    //
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 6),
            Constraint::Ratio(1, 6),
            Constraint::Ratio(4, 6),
        ])
        .split(rows[1]);
    //
    // users
    //
    // Iterate through all elements in the `items` app and append some debug text to it.
    let users: Vec<ListItem> = app
        .users
        .iter()
        .map(|u| ListItem::new(format!("{} ({})", u.name, u.short_name)))
        .collect();
    let users = List::new(users)
        .block(Block::default().borders(Borders::ALL).title("Users"))
        .style(get_style(app.get_state(Widget::Users)))
        .highlight_style(selected_style.clone())
        .highlight_symbol(">> ");
    f.render_stateful_widget(users, columns[0], &mut app.users_state);
    //
    // chats
    //
    let chats: Vec<ListItem> = app
        .chats
        .iter()
        .map(|c| ListItem::new(format!("chat #{}", c.id)))
        .collect();
    let chats = List::new(chats)
        .block(Block::default().borders(Borders::ALL).title("Chats"))
        .style(get_style(app.get_state(Widget::Chats)))
        .highlight_style(selected_style.clone())
        .highlight_symbol(">> ");
    f.render_stateful_widget(chats, columns[1], &mut app.chats_state);
    //
    // selected chat content
    //
    let content: Vec<ListItem> = vec![ListItem::new("Select chat to view posts")];
    let content = List::new(content)
        .block(Block::default().borders(Borders::ALL).title("Posts"))
        .style(get_style(app.get_state(Widget::Posts)))
        .highlight_style(selected_style.clone())
        .highlight_symbol(">> ");
    f.render_stateful_widget(content, columns[2], &mut app.posts_state);
    //
    // logger
    //
    if app.extended_log {
        let tui_sm = TuiLoggerSmartWidget::default()
            .border_style(get_style(app.get_state(Widget::Log)))
            .style_error(Style::default().fg(Color::Red))
            .style_debug(Style::default().fg(Color::Green))
            .style_warn(Style::default().fg(Color::Yellow))
            .style_trace(Style::default().fg(Color::Magenta))
            .style_info(Style::default().fg(Color::Cyan))
            .state(&mut app.logger_state);
        f.render_widget(tui_sm, rows[2]);
    } else {
        let tui_w: TuiLoggerWidget = TuiLoggerWidget::default()
            .block(
                Block::default()
                    .title("Independent Tui Logger View")
                    .border_style(get_style(app.get_state(Widget::Log)))
                    .borders(Borders::ALL),
            )
            .style(get_style(app.get_state(Widget::Log)));
        f.render_widget(tui_w, rows[2]);
    }
    //
    // input
    //
    if let Some(input) = &app.input {
        let block = Paragraph::new(input.text.as_ref())
            .style(get_style(app.get_state(Widget::Input)))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .style(get_style(app.get_state(Widget::Input)))
                    .title(input.title.as_str()),
            );
        let area = Rect::new(columns[1].left() + 5, columns[1].top() + 5, 60, 3); // centered_rect(60, 10, f.size());
        f.render_widget(Clear, area); //this clears out the background
        f.render_widget(block, area);
        // Make the cursor visible and ask tui-rs to put it at the specified coordinates after rendering
        f.set_cursor(
            // Put cursor past the end of the input text
            area.x + input.text.len() as u16 + 1,
            // Move one line down, from the border to the input line
            area.y + 1,
        )
    }
}

fn get_style(state: WidgetState) -> Style {
    match state {
        WidgetState::Modal => Style::default().fg(Color::Green),
        WidgetState::Focused => Style::default().fg(Color::Magenta),
        _ => Style::default().fg(Color::Gray),
    }
}

/// helper function to create a centered rect using up
/// certain percentage of the available rect `r`
#[allow(dead_code)]
fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let top_pad = (r.height - r.height.min(height)) / 2;
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(top_pad),
                Constraint::Length(height),
                Constraint::Length(r.height - top_pad - height),
            ]
            .as_ref(),
        )
        .split(r);
    let left_pad = (r.width - r.width.min(width)) / 2;
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Length(left_pad),
                Constraint::Length(width),
                Constraint::Length(r.width - left_pad - width),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}
