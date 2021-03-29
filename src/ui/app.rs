use crate::proto;
use crate::Command;
use log::error;
use tokio::sync::mpsc;
use tui::widgets::ListState;
use tui_logger::{TuiWidgetEvent, TuiWidgetState};

// pub type SharedChats = Arc<Mutex<Vec<proto::Chat>>>;
// pub type SharedUsers = Arc<Mutex<Vec<proto::User>>>;
// pub type SharedPosts = Arc<Mutex<Vec<proto::Post>>>;

#[derive(PartialEq, Clone, Copy)]
pub enum Widget {
    App,
    Users,
    Chats,
    Posts,
    Log,
    Input,
}

pub enum State {
    Normal,
    Focused,
    Modal,
}

// input text consumer
enum InputResult {
    NewChat, // new chat name
    NewPost, // new post text
}

pub struct InputMode {
    purpose: InputResult,
    pub title: String,
    pub text: String,
}

impl InputMode {
    pub fn new_chat() -> Self {
        InputMode {
            purpose: InputResult::NewChat,
            title: "New chat name".to_string(),
            text: String::with_capacity(64),
        }
    }

    pub fn new_post() -> Self {
        InputMode {
            purpose: InputResult::NewPost,
            title: "Post content".to_string(),
            text: String::with_capacity(512),
        }
    }
}

pub struct App {
    pub title: String,
    pub users: Vec<proto::User>,
    pub users_state: ListState,
    pub chats: Vec<proto::Chat>,
    pub chats_state: ListState,
    pub posts: Vec<proto::Post>,
    pub posts_state: ListState,
    pub logger_state: TuiWidgetState,
    pub current_user: String,
    pub extended_log: bool,

    tx_command: mpsc::Sender<Command>,
    focused: Widget,
    modal: Widget,
    pub input: Option<InputMode>,
}

impl App {
    pub fn new(
        user: proto::UserInfo,
        tx_command: mpsc::Sender<Command>,
        extended_log: bool,
    ) -> Self {
        App {
            title: "MiGChat".to_string(),
            users: Vec::new(),
            users_state: ListState::default(),
            chats: Vec::new(),
            chats_state: ListState::default(),
            posts: Vec::with_capacity(128),
            posts_state: ListState::default(),
            logger_state: TuiWidgetState::new(),
            current_user: format!("{} ({})", user.name, user.short_name),
            extended_log,
            tx_command,
            focused: Widget::Chats,
            modal: Widget::App,
            input: None,
        }
    }

    pub fn get_state(&self, widget: Widget) -> State {
        if widget == self.modal {
            State::Modal
        } else if widget == self.focused {
            State::Focused
        } else {
            State::Normal
        }
    }

    pub fn on_up(&mut self) {
        if !self.test_prev(true) {
            // pass event into modal
            match self.modal {
                Widget::Log => {
                    self.logger_state.transition(&TuiWidgetEvent::UpKey);
                }
                Widget::Chats => {
                    App::list_previous(&mut self.chats_state, self.chats.len());
                }
                Widget::Users => {
                    App::list_previous(&mut self.users_state, self.users.len());
                }
                Widget::Posts => {
                    App::list_previous(&mut self.posts_state, self.posts.len());
                }
                _ => {}
            }
        }
    }

    pub fn on_down(&mut self) {
        if !self.test_next(true) {
            // pass event into modal
            match self.modal {
                Widget::Log => {
                    self.logger_state.transition(&TuiWidgetEvent::DownKey);
                }
                Widget::Chats => {
                    App::list_next(&mut self.chats_state, self.chats.len());
                }
                Widget::Users => {
                    App::list_next(&mut self.users_state, self.users.len());
                }
                Widget::Posts => {
                    App::list_next(&mut self.posts_state, self.posts.len());
                }
                _ => {}
            }
        }
    }

    pub fn on_right(&mut self) {
        if !self.test_next(false) {
            // pass event into focused
            match self.modal {
                Widget::Log => {
                    self.logger_state.transition(&TuiWidgetEvent::RightKey);
                }
                Widget::Input => {
                    error!("failed passing --> to input");
                }
                _ => {}
            }
        }
    }

    pub fn on_left(&mut self) {
        if !self.test_prev(false) {
            // pass event into focused
            match self.modal {
                Widget::Log => {
                    self.logger_state.transition(&TuiWidgetEvent::LeftKey);
                }
                Widget::Input => {
                    error!("failed passing <-- to input");
                }
                _ => {}
            }
        }
    }

    pub fn on_enter(&mut self) {
        match self.modal {
            Widget::App => {
                self.modal = self.focused;
            }
            Widget::Log => {
                self.logger_state.transition(&TuiWidgetEvent::FocusKey);
            }
            Widget::Input => {
                // accept input:
                if let Some(input) = &self.input {
                    match input.purpose {
                        InputResult::NewChat => {
                            if let Err(e) = self.tx_command.blocking_send(Command::CreateChat(
                                proto::ChatInfo {
                                    permanent: true,
                                    auto_enter: true,
                                    description: input.text.clone(),
                                    required: Vec::new(),
                                    optional: Vec::new(),
                                },
                            )) {
                                error!("failed creating chat: {}", e);
                            }
                        }
                        InputResult::NewPost => {
                            error!(
                                "failed creating post '{}': no channel to gRPC client",
                                &input.text
                            );
                        }
                    }
                }
                self.input = None;
                // restore previous modal widget:
                self.modal = self.focused;
            }
            _ => {}
        };
    }

    pub fn test_exit(&mut self) -> bool {
        match self.modal {
            Widget::Input => {
                self.modal = self.focused;
                self.input = None;
                false
            }
            Widget::App => true,
            _ => {
                self.modal = Widget::App;
                false
            }
        }
    }

    pub fn on_key(&mut self, c: char) {
        if self.modal == Widget::Input {
            if let Some(input) = self.input.as_mut() {
                input.text.push(c);
            } else {
                error!("input mode is not init properly");
            }
        } else {
            match c {
                'q' => {
                    let _ = self.test_exit();
                }
                ' ' => {
                    if self.modal == Widget::Log {
                        self.logger_state.transition(&TuiWidgetEvent::SpaceKey);
                    }
                }
                '-' => {
                    if self.modal == Widget::Log {
                        self.logger_state.transition(&TuiWidgetEvent::MinusKey);
                    }
                }
                '+' => {
                    if self.modal == Widget::Log {
                        self.logger_state.transition(&TuiWidgetEvent::PlusKey);
                    }
                }
                'n' => {
                    // create new item
                    match self.modal {
                        Widget::Chats => {
                            self.focused = Widget::Chats;
                            self.modal = Widget::Input;
                            // setup input mode:
                            self.input = Some(InputMode::new_chat());
                        }
                        Widget::Posts => {
                            self.focused = Widget::Posts;
                            self.modal = Widget::Input;
                            // setup input mode:
                            self.input = Some(InputMode::new_post());
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    pub fn on_backspace(&mut self) {
        if let Some(input) = self.input.as_mut() {
            if !input.text.is_empty() {
                input.text.pop();
            }
        }
    }

    pub fn on_tick(&mut self) {}

    // true if focus changed
    fn test_next(&mut self, vert: bool) -> bool {
        match self.modal {
            Widget::App => {
                let stored = self.focused;
                self.focused = match self.focused {
                    Widget::Chats => {
                        if vert {
                            Widget::Log
                        } else {
                            Widget::Posts
                        }
                    }
                    Widget::Posts => {
                        if vert {
                            Widget::Log
                        } else {
                            self.focused
                        }
                    }
                    Widget::Users => {
                        if vert {
                            Widget::Log
                        } else {
                            Widget::Chats
                        }
                    }
                    Widget::App => Widget::Log,
                    _ => self.focused,
                };
                stored != self.focused
            }
            _ => false,
        }
    }

    // true if focus changed
    fn test_prev(&mut self, vert: bool) -> bool {
        match self.modal {
            Widget::App => {
                let stored = self.focused;
                self.focused = match self.focused {
                    Widget::Chats => {
                        if vert {
                            self.focused
                        } else {
                            Widget::Users
                        }
                    }
                    Widget::Posts => {
                        if vert {
                            self.focused
                        } else {
                            Widget::Chats
                        }
                    }
                    Widget::Log => {
                        if vert {
                            Widget::Chats
                        } else {
                            self.focused
                        }
                    }
                    Widget::App => Widget::Chats,
                    _ => self.focused,
                };
                stored != self.focused
            }
            _ => false,
        }
    }

    fn list_next(state: &mut ListState, count: usize) {
        if count == 0 {
            state.select(None);
        } else {
            state.select(
                state
                    .selected()
                    .and_then(|cur| Some(count.min(cur + 1)))
                    .or(Some(0)),
            );
        }
    }

    fn list_previous(state: &mut ListState, count: usize) {
        if count == 0 {
            state.select(None);
        } else {
            state.select(
                state
                    .selected()
                    .and_then(|cur| {
                        if cur > 0 {
                            Some(count.min(cur - 1))
                        } else {
                            Some(0)
                        }
                    })
                    .or(Some(0)),
            );
        }
    }

    // chat events handling

    pub fn on_user_entered(&mut self, user: proto::User) {
        self.users.push(user);
    }
}
