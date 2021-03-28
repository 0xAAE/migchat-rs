use super::{App, Widget, WidgetState};
use tui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Span, Spans},
    widgets::canvas::{Canvas, Line, Map, MapResolution, Rectangle},
    widgets::{
        Axis, BarChart, Block, Borders, Cell, Chart, Dataset, Gauge, LineGauge, List, ListItem,
        Paragraph, Row, Sparkline, Table, Tabs, Wrap,
    },
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
    let users: Vec<ListItem> = if let Ok(users) = app.users.lock() {
        users
            .iter()
            .map(|u| ListItem::new(format!("{} ({})", u.name, u.short_name)))
            .collect()
    } else {
        // failed to lock users
        vec![ListItem::new("failed")]
    };
    let users = List::new(users)
        .block(Block::default().borders(Borders::ALL).title("Users"))
        .style(get_style(app.get_state(Widget::Users)))
        .highlight_style(selected_style.clone())
        .highlight_symbol(">> ");
    f.render_stateful_widget(users, columns[0], &mut app.users_state);
    //
    // chats
    //
    let chats: Vec<ListItem> = if let Ok(chats) = app.chats.lock() {
        chats
            .iter()
            .map(|c| ListItem::new(format!("chat #{}", c.id)))
            .collect()
    } else {
        // failed to lock users
        vec![ListItem::new("failed")]
    };
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
}

fn get_style(state: WidgetState) -> Style {
    match state {
        WidgetState::Modal => Style::default().fg(Color::Green),
        WidgetState::Focused => Style::default().fg(Color::Magenta),
        _ => Style::default().fg(Color::Gray),
    }
}
