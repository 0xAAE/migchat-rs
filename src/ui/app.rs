use crate::proto;
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
}

pub enum State {
    Normal,
    Focused,
    Modal,
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
            current_user: format!("{} ({})", user.short_name, user.name),
            extended_log,
            focused: Widget::Chats,
            modal: Widget::App,
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
            // pass event into focused
            if self.modal == Widget::Log {
                self.logger_state.transition(&TuiWidgetEvent::UpKey);
            }
        }
    }

    pub fn on_down(&mut self) {
        if !self.test_next(true) {
            // pass event into focused
            if self.modal == Widget::Log {
                self.logger_state.transition(&TuiWidgetEvent::DownKey);
            }
        }
    }

    pub fn on_right(&mut self) {
        if !self.test_next(false) {
            // pass event into focused
            if self.modal == Widget::Log {
                self.logger_state.transition(&TuiWidgetEvent::RightKey);
            }
        }
    }

    pub fn on_left(&mut self) {
        if !self.test_prev(false) {
            // pass event into focused
            if self.modal == Widget::Log {
                self.logger_state.transition(&TuiWidgetEvent::LeftKey);
            }
        }
    }

    pub fn on_enter(&mut self) {
        if !self.test_modal() {
            if self.modal == Widget::Log {
                self.logger_state.transition(&TuiWidgetEvent::FocusKey);
            }
        }
    }

    pub fn test_exit(&mut self) -> bool {
        self.test_esc()
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
            _ => {}
        }
    }

    pub fn on_tick(&mut self) {
        // // Update progress
        // self.progress += 0.001;
        // if self.progress > 1.0 {
        //     self.progress = 0.0;
        // }

        // self.sparkline.on_tick();
        // self.signals.on_tick();

        // let log = self.logs.items.pop().unwrap();
        // self.logs.items.insert(0, log);

        // let event = self.barchart.pop().unwrap();
        // self.barchart.insert(0, event);
    }

    // true if should exit
    fn test_esc(&mut self) -> bool {
        match self.modal {
            Widget::App => true,
            _ => {
                self.modal = Widget::App;
                false
            }
        }
    }

    // true if enter modal state
    fn test_modal(&mut self) -> bool {
        match self.modal {
            Widget::App => {
                self.modal = self.focused;
                true
            }
            _ => false,
        }
    }

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
                    Widget::Log => self.focused,
                    Widget::App => Widget::Log,
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
                    Widget::Users => self.focused,
                    Widget::Log => {
                        if vert {
                            Widget::Chats
                        } else {
                            self.focused
                        }
                    }
                    Widget::App => Widget::Chats,
                };
                stored != self.focused
            }
            _ => false,
        }
    }
}
