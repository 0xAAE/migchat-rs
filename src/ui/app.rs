use crate::proto;
use std::sync::{Arc, Mutex};
use tui_logger::TuiWidgetState;

pub type SharedChats = Arc<Mutex<Vec<proto::Chat>>>;
pub type SharedUsers = Arc<Mutex<Vec<proto::User>>>;

pub struct App {
    pub title: String,
    pub users: SharedUsers,
    pub chats: SharedChats,
    pub logger_state: TuiWidgetState,
}

impl App {
    pub fn new(users: SharedUsers, chats: SharedChats) -> Self {
        App {
            title: "MiGChat".to_string(),
            users,
            chats,
            logger_state: TuiWidgetState::new(),
        }
    }

    pub fn on_up(&mut self) {
        //self.tasks.previous();
    }

    pub fn on_down(&mut self) {
        //self.tasks.next();
    }

    pub fn on_right(&mut self) {
        //self.tabs.next();
    }

    pub fn on_left(&mut self) {
        //self.tabs.previous();
    }

    pub fn on_key(&mut self, c: char) {
        match c {
            // 'q' => {
            //     self.should_quit = true;
            // }
            // 't' => {
            //     self.show_chart = !self.show_chart;
            // }
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
}
