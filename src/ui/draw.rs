use super::App;
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
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        &app.title,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ));
    let paragraph = Paragraph::new("user (short name)")
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
            Constraint::Ratio(1, 6),
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
        .highlight_style(
            Style::default()
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    f.render_widget(users, columns[0]);
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
        .highlight_style(
            Style::default()
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    f.render_widget(chats, columns[1]);
    //
    // selected chat content
    //
    let content: Vec<ListItem> = vec![ListItem::new("Select chat to view posts")];
    let content = List::new(content)
        .block(Block::default().borders(Borders::ALL).title("Posts"))
        .highlight_style(
            Style::default()
                .bg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");
    f.render_widget(content, columns[2]);
    //
    // logger
    //
    // let tui_sm = TuiLoggerSmartWidget::default()
    //     .style_error(Style::default().fg(Color::Red))
    //     .style_debug(Style::default().fg(Color::Green))
    //     .style_warn(Style::default().fg(Color::Yellow))
    //     .style_trace(Style::default().fg(Color::Magenta))
    //     .style_info(Style::default().fg(Color::Cyan))
    //     .state(&mut app.logger_state);
    // f.render_widget(tui_sm, rows[2]);
    let tui_w: TuiLoggerWidget = TuiLoggerWidget::default()
        .block(
            Block::default()
                .title("Independent Tui Logger View")
                .border_style(Style::default().fg(Color::White).bg(Color::Black))
                .borders(Borders::ALL),
        )
        .style(Style::default().fg(Color::White).bg(Color::Black));
    f.render_widget(tui_w, rows[2]);
}
