use crate::proto;
use log::error;
use std::sync::{Arc, Mutex};
use tui::widgets::ListState;
use tui_logger::{TuiWidgetEvent, TuiWidgetState};

pub type SharedChats = Arc<Mutex<Vec<proto::Chat>>>;
pub type SharedUsers = Arc<Mutex<Vec<proto::User>>>;
pub type SharedPosts = Arc<Mutex<Vec<proto::Post>>>;

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
}

pub struct App {
    pub title: String,
    pub users: SharedUsers,
    pub users_state: ListState,
    pub chats: SharedChats,
    pub chats_state: ListState,
    pub posts: SharedPosts,
    pub posts_state: ListState,
    pub logger_state: TuiWidgetState,
    pub current_user: String,
    pub extended_log: bool,

    focused: Widget,
    modal: Widget,
    pub input: Option<InputMode>,
}

impl App {
    pub fn new(
        user: proto::UserInfo,
        users: SharedUsers,
        chats: SharedChats,
        posts: SharedPosts,
        extended_log: bool,
    ) -> Self {
        App {
            title: "MiGChat".to_string(),
            users,
            users_state: ListState::default(),
            chats,
            chats_state: ListState::default(),
            posts,
            posts_state: ListState::default(),
            logger_state: TuiWidgetState::new(),
            current_user: format!("{} ({})", user.name, user.short_name),
            extended_log,
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
                    if let Ok(chats) = self.chats.lock() {
                        App::list_previous(&mut self.chats_state, chats.len());
                    }
                }
                Widget::Users => {
                    if let Ok(users) = self.users.lock() {
                        App::list_previous(&mut self.users_state, users.len());
                    }
                }
                Widget::Posts => {
                    if let Ok(posts) = self.posts.lock() {
                        App::list_previous(&mut self.posts_state, posts.len());
                    }
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
                    if let Ok(chats) = self.chats.lock() {
                        App::list_next(&mut self.chats_state, chats.len());
                    }
                }
                Widget::Users => {
                    if let Ok(users) = self.users.lock() {
                        App::list_next(&mut self.users_state, users.len());
                    }
                }
                Widget::Posts => {
                    if let Ok(posts) = self.posts.lock() {
                        App::list_next(&mut self.posts_state, posts.len());
                    }
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
                            error!("failed creating new chat: no channel to gRPC client")
                        }
                        InputResult::NewPost => {
                            error!("failed creating new chat: no channel to gRPC client")
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
                        self.input = Some(InputMode {
                            purpose: InputResult::NewChat,
                            title: "New chat name".to_string(),
                        });
                    }
                    Widget::Posts => {
                        self.focused = Widget::Posts;
                        self.modal = Widget::Input;
                        // setup input mode:
                        self.input = Some(InputMode {
                            purpose: InputResult::NewPost,
                            title: "New post content".to_string(),
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
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
}
