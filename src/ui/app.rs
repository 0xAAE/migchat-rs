use crate::proto::{self, ChatId, UserId, NOT_USER_ID};
use crate::Command;
use log::{error, warn};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tui::widgets::ListState;
use tui_logger::{TuiWidgetEvent, TuiWidgetState};

// pub type SharedChats = Arc<Mutex<Vec<proto::Chat>>>;
// pub type SharedUsers = Arc<Mutex<Vec<proto::User>>>;
// pub type SharedPosts = Arc<Mutex<Vec<proto::Post>>>;

#[derive(PartialEq, Clone, Copy, Debug)]
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
#[derive(PartialEq)]
enum InputResult {
    NewChat, // new chat name
    NewPost, // new post text
    UserInfo,
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

    pub fn new_user_info() -> Self {
        InputMode {
            purpose: InputResult::UserInfo,
            title: "Login, Full Name".to_string(),
            text: String::with_capacity(512),
        }
    }
}

pub struct App {
    pub title: String,
    pub users: Vec<proto::User>,
    pub online: Vec<UserId>,
    pub users_state: ListState,
    pub chats: HashMap<ChatId, proto::Chat>,
    pub chats_state: ListState,
    pub posts: Vec<proto::Post>,
    pub posts_state: ListState,
    pub logger_state: TuiWidgetState,
    pub user_description: String,
    pub user: proto::User,
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
        let need_user_info = user.name.is_empty() && user.short_name.is_empty();
        let modal = if need_user_info {
            Widget::Input
        } else {
            Widget::App
        };
        let input = if need_user_info {
            Some(InputMode::new_user_info())
        } else {
            if let Err(e) = tx_command.blocking_send(Command::Register(user.clone())) {
                error!("failed to send command to register: {}", e);
            }
            None
        };
        App {
            title: "MiGChat".to_string(),
            users: Vec::new(),
            online: Vec::new(),
            users_state: ListState::default(),
            chats: HashMap::new(),
            chats_state: ListState::default(),
            posts: Vec::with_capacity(128),
            posts_state: ListState::default(),
            logger_state: TuiWidgetState::new(),
            user_description: format!("{}", user),
            user: proto::User {
                id: NOT_USER_ID,
                name: user.name,
                short_name: user.short_name,
            },
            extended_log,
            tx_command,
            focused: Widget::Chats,
            modal,
            input,
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
        match self.modal {
            Widget::Log => {
                self.logger_state.transition(&TuiWidgetEvent::UpKey);
            }
            Widget::App => match self.focused {
                Widget::Users => App::list_previous(&mut self.users_state, self.users.len()),
                Widget::Chats => App::list_previous(&mut self.chats_state, self.chats.len()),
                Widget::Posts => App::list_previous(&mut self.posts_state, self.posts.len()),
                _ => {}
            },
            _ => {}
        }
    }

    pub fn on_down(&mut self) {
        match self.modal {
            Widget::Log => {
                self.logger_state.transition(&TuiWidgetEvent::DownKey);
            }
            Widget::App => match self.focused {
                Widget::Chats => App::list_next(&mut self.chats_state, self.chats.len()),
                Widget::Users => App::list_next(&mut self.users_state, self.users.len()),
                Widget::Posts => App::list_next(&mut self.posts_state, self.posts.len()),
                _ => {}
            },
            _ => {}
        }
    }

    pub fn on_right(&mut self) {
        match self.modal {
            Widget::Log => {
                self.logger_state.transition(&TuiWidgetEvent::RightKey);
            }
            Widget::App => match self.focused {
                Widget::Users => self.focused = Widget::Chats,
                Widget::Chats => self.focused = Widget::Posts,
                _ => {}
            },
            _ => {}
        }
    }

    pub fn on_left(&mut self) {
        match self.modal {
            Widget::Log => {
                self.logger_state.transition(&TuiWidgetEvent::LeftKey);
            }
            Widget::App => match self.focused {
                Widget::Chats => self.focused = Widget::Users,
                Widget::Posts => self.focused = Widget::Chats,
                _ => {}
            },
            _ => {}
        }
    }

    pub fn on_enter(&mut self) {
        match self.modal {
            Widget::Log => {
                self.logger_state.transition(&TuiWidgetEvent::FocusKey);
            }
            Widget::Input => {
                // accept input:
                if let Some(input) = &self.input {
                    match input.purpose {
                        InputResult::NewChat => {
                            let mut desired_users = Vec::new();
                            if let Some(user) = self.get_sel_user() {
                                desired_users.push(user.id);
                            }
                            if let Err(e) = self.tx_command.blocking_send(Command::CreateChat(
                                proto::ChatInfo {
                                    user_id: self.user.id,
                                    permanent: true,
                                    auto_enter: true,
                                    description: input.text.clone(),
                                    desired_users,
                                },
                            )) {
                                error!("failed creating chat: {}", e);
                            }
                        }
                        InputResult::NewPost => {
                            if let Some(chat) = self.get_sel_chat() {
                                let chat_id = chat.id;
                                if let Err(e) =
                                    self.tx_command.blocking_send(Command::Post(proto::Post {
                                        id: proto::NOT_POST_ID,
                                        user_id: self.user.id,
                                        chat_id,
                                        text: input.text.clone(),
                                        attachments: Vec::new(),
                                    }))
                                {
                                    error!("failed creating post: {}", e);
                                }
                            }
                        }
                        InputResult::UserInfo => {
                            if let Ok(info) = input.text.parse::<proto::UserInfo>() {
                                self.user_description = format!("{}", &info);
                                self.user.name = info.name.clone();
                                self.user.short_name = info.short_name.clone();
                                if let Err(e) =
                                    self.tx_command.blocking_send(Command::Register(info))
                                {
                                    error!("failed to send command to register: {}", e);
                                }
                            } else {
                                // remaining modal state of input
                                return;
                            }
                        }
                    }
                }
                self.input = None;
                // restore previous modal widget:
                self.modal = Widget::App;
            }
            _ => {}
        };
    }

    pub fn on_esc(&mut self) {
        match self.modal {
            Widget::Input => {
                if let Some(mode) = &self.input {
                    if mode.purpose != InputResult::UserInfo {
                        self.modal = Widget::App
                    }
                }
            }
            Widget::App => match self.focused {
                Widget::Users => {
                    self.users_state.select(None);
                }
                Widget::Chats => {
                    self.chats_state.select(None);
                }
                Widget::Posts => {
                    self.posts_state.select(None);
                }
                _ => {}
            },
            _ => {
                error!("widget {:?} must not be modal", self.modal);
                self.modal = Widget::App;
            }
        }
    }

    pub fn on_key(&mut self, c: char, ctrl: bool, alt: bool) {
        // exit in any modal widget
        if ctrl && c == 'q' {
            if let Err(e) = self.tx_command.blocking_send(Command::Exit) {
                error!("failed sending Exit command: {}", e);
            }
            return;
        }
        if self.modal == Widget::Input {
            if let Some(input) = self.input.as_mut() {
                input.text.push(c);
            } else {
                error!("input mode is not init properly");
            }
        } else {
            match c {
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
                    if ctrl {
                        // create new item
                        match self.focused {
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
                }
                'p' => {
                    // create new post
                    if self.get_sel_chat().is_some() {
                        self.modal = Widget::Input;
                        self.input = Some(InputMode::new_post());
                    }
                }
                'i' => {
                    if alt {
                        match self.focused {
                            Widget::Users => {
                                // invite selected user into selected chat
                                if let Some(user) = self.get_sel_user() {
                                    if let Some(chat) = self.get_sel_chat() {
                                        if let Err(e) = self.tx_command.blocking_send(
                                            Command::Invite(proto::Invitation {
                                                chat_id: chat.id,
                                                from_user_id: self.user.id,
                                                to_user_id: user.id,
                                            }),
                                        ) {
                                            error!(
                                                "failed inviting {} to {}: {}",
                                                user.short_name, chat.description, e
                                            );
                                        }
                                    }
                                }
                            }
                            Widget::Chats => {
                                // also create new post
                                if self.get_sel_chat().is_some() {
                                    self.modal = Widget::Input;
                                    self.input = Some(InputMode::new_post());
                                }
                            }
                            _ => {}
                        }
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

    pub fn get_sel_chat(&self) -> Option<&proto::Chat> {
        self.chats_state
            .selected()
            .and_then(|idx| self.chats.values().nth(idx))
    }

    pub fn get_chat(&self, chat_id: ChatId) -> Option<&proto::Chat> {
        self.chats.get(&chat_id)
    }

    pub fn get_posts_count(&self, chat_id: proto::ChatId) -> usize {
        self.posts
            .iter()
            .filter(|post| post.chat_id == chat_id)
            .count()
    }

    pub fn get_sel_user(&self) -> Option<&proto::User> {
        self.users_state
            .selected()
            .and_then(|idx| self.users.get(idx))
    }

    pub fn get_user(&self, user_id: UserId) -> Option<&proto::User> {
        if self.user.id == user_id {
            Some(&self.user)
        } else {
            self.users.iter().find(|u| u.id == user_id)
        }
    }

    pub fn get_user_description(user: &proto::User) -> String {
        format!("{}", proto::UserInfo::from(user.clone()))
    }

    fn list_next(state: &mut ListState, count: usize) {
        if count == 0 {
            state.select(None);
        } else {
            state.select(state.selected().map(|cur| count.min(cur + 1)).or(Some(0)));
        }
    }

    fn list_previous(state: &mut ListState, count: usize) {
        if count == 0 {
            state.select(None);
        } else {
            state.select(
                state
                    .selected()
                    .map(|cur| if cur > 0 { count.min(cur - 1) } else { 0 })
                    .or(Some(0)),
            );
        }
    }

    // chat events handling

    pub fn on_registered(&mut self, user_id: UserId) {
        self.user.id = user_id;
    }

    pub fn on_user_info(&mut self, user: proto::User) {
        if !self.users.iter().any(|u| u.id == user.id) {
            self.users.push(user);
        }
    }

    pub fn on_user_entered(&mut self, id: UserId) {
        self.online.push(id);
    }

    pub fn on_user_gone(&mut self, id: UserId) {
        self.online.retain(|item| *item != id);
    }

    pub fn on_chat_updated(&mut self, chat: proto::Chat) {
        let _prev = self.chats.insert(chat.id, chat);
    }

    pub fn on_get_invited(&mut self, invitation: proto::Invitation) {
        //todo: ask user about invitation
        if let Some(chat) = self.get_chat(invitation.chat_id) {
            if chat.users.iter().any(|u| *u == self.user.id) {
                warn!("got invitation while being in that chat");
            }
        }
        // auto enter chat
        if let Err(e) = self
            .tx_command
            .blocking_send(Command::EnterChat(invitation.chat_id))
        {
            error!("failed creating post: {}", e);
        }
    }

    pub fn on_new_post(&mut self, post: proto::Post) {
        self.posts.push(post);
    }

    pub fn on_chat_deleted(&mut self, chat_id: ChatId) {
        self.chats.remove(&chat_id);
    }
}

#[test]
fn test_vec_iter() {
    let mut v = vec![4; 4];
    if !v.iter().any(|&n| n == 7) {
        v.push(7);
    }
    assert_eq!(v.len(), 5);
}
