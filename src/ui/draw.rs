use super::{App, Widget, WidgetState};
use tui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use tui_logger::{TuiLoggerSmartWidget, TuiLoggerWidget};

fn get_style(state: WidgetState) -> Style {
    match state {
        WidgetState::Modal => Style::default().fg(Color::Green),
        WidgetState::Focused => Style::default().fg(Color::Cyan),
        _ => Style::default().fg(Color::Gray),
    }
}

pub fn draw<B: Backend>(f: &mut Frame<B>, app: &mut App) {
    //
    // styles
    //
    let caption_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let selected_style = Style::default().add_modifier(Modifier::BOLD);
    let users_style = get_style(app.get_state(Widget::Users));
    let chats_style = get_style(app.get_state(Widget::Chats));
    let posts_style = get_style(app.get_state(Widget::Posts));
    let log_style = get_style(app.get_state(Widget::Log));
    let input_style = get_style(app.get_state(Widget::Input));
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
    let paragraph = Paragraph::new(app.user_description.as_str())
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
        .map(|u| ListItem::new(App::get_user_description(u)))
        .collect();
    let users = List::new(users)
        .block(Block::default().borders(Borders::ALL).title("users"))
        .style(users_style)
        .highlight_symbol("> ")
        .highlight_style(selected_style);
    f.render_stateful_widget(users, columns[0], &mut app.users_state);
    //
    // chats
    //
    let chats: Vec<ListItem> = app
        .chats
        .values()
        .map(|c| {
            let posts_count = app.get_posts_count(c.chat_id);
            let chat_header = if posts_count > 0 {
                format!("{} ({})", c.description, posts_count)
            } else {
                c.description.clone()
            };
            let mut lines = vec![Spans::from(Span::styled(chat_header, chats_style))];
            let mut users = String::from("(");
            let mut continue_flag = false;
            for u in &c.users {
                if continue_flag {
                    users.push_str(", ");
                }
                users.push_str(
                    app.get_user(*u)
                        .map(|u| u.short_name.clone())
                        .unwrap_or_else(|| format!("{}", u))
                        .as_str(),
                );
                continue_flag = true;
            }
            users.push(')');
            let users_style = chats_style.add_modifier(Modifier::ITALIC);
            if users.len() > 2 {
                lines.push(Spans::from(Span::styled(users, users_style)));
            } else {
                lines.push(Spans::from(Span::styled("(empty)", users_style)));
            }
            ListItem::new(lines).style(chats_style)
        })
        .collect();
    let chats = List::new(chats)
        .block(Block::default().borders(Borders::ALL).title("select chat"))
        .style(chats_style)
        .highlight_symbol("> ")
        .highlight_style(selected_style);
    f.render_stateful_widget(chats, columns[1], &mut app.chats_state);
    //
    // selected chat content
    //
    let content: Vec<ListItem> = if let Some(chat) = app.get_sel_chat() {
        app.posts
            .iter()
            .filter(|post| post.chat_id == chat.chat_id)
            .map(|post| {
                ListItem::new(Spans::from(vec![
                    Span::styled(
                        app.get_user(post.user_id)
                            .map(|u| {
                                if u.user_id == app.user.user_id {
                                    String::from("me")
                                } else {
                                    u.short_name.clone()
                                }
                            })
                            .unwrap_or_else(|| format!("{}", post.user_id)),
                        selected_style.add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(": {}", post.text.as_str()), posts_style),
                ]))
            })
            .collect()
    } else {
        vec![ListItem::new("select chat to view posts")]
    };
    let posts_title = if let Some(chat) = app.get_sel_chat() {
        format!(
            "{} ({})",
            chat.description.clone(),
            app.get_posts_count(chat.chat_id)
        )
    } else {
        String::from("No chat selected")
    };
    let content = List::new(content)
        .block(Block::default().borders(Borders::ALL).title(posts_title))
        .style(posts_style)
        .highlight_symbol("> ")
        .highlight_style(selected_style);
    f.render_stateful_widget(content, columns[2], &mut app.posts_state);
    //
    // logger
    //
    if app.extended_log {
        let tui_sm = TuiLoggerSmartWidget::default()
            .border_style(log_style)
            .style_error(Style::default().fg(Color::Red))
            .style_debug(Style::default().fg(Color::Green))
            .style_warn(Style::default().fg(Color::Yellow))
            .style_trace(Style::default().fg(Color::Magenta))
            .style_info(Style::default().fg(Color::Cyan))
            .state(&app.logger_state);
        f.render_widget(tui_sm, rows[2]);
    } else {
        let tui_w: TuiLoggerWidget = TuiLoggerWidget::default()
            .block(
                Block::default()
                    .title("Events viewer")
                    .border_style(log_style)
                    .borders(Borders::ALL),
            )
            .style(log_style);
        f.render_widget(tui_w, rows[2]);
    }
    //
    // input
    //
    if let Some(input) = &app.input {
        let block = Paragraph::new(input.text.as_ref())
            .style(input_style)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .style(input_style)
                    .title(input.title.as_str()),
            );
        //let area = Rect::new(columns[1].left() + 5, columns[1].top() + 5, 60, 3);
        let area = centered_rect(60, 3, f.size());
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
