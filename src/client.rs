// use std::error::Error;
use clap::{App, Arg};
use config::{Config, Environment, File};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use log::{error, info, LevelFilter};
use std::{
    io::stdout,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tui::{backend::CrosstermBackend, Terminal};

mod client_service;
mod proto;
mod ui;

use client_service::{ChatRoomEvent, Command, MigchatClient};
use proto::UserInfo;

const CONFIG: &str = "config";
const CONFIG_DEF: &str = "client.toml";
const CONFIG_ENV: &str = "MIGC";

// Events
pub enum Event {
    // crossterm input events, keyboard
    Input(KeyEvent),
    // timer ticks
    Tick,
    // gRPC client events
    Client(ChatRoomEvent),
    // exit reqired
    Exit,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // commnad line
    let matches = App::new("migchat-client")
        .version("0.1")
        .author("0xAAE <avramenko.a@gmail.com>")
        .about(
            "The MiGChat command line client. Use MIGC_* environment variables to override config file settings",
        )
        .arg(
            Arg::with_name(CONFIG)
                .short("c")
                .long(CONFIG)
                .value_name("FILE")
                .help("Sets a custom config file")
                .takes_value(true),
        )
        .get_matches();
    let config_file = matches.value_of(CONFIG).unwrap_or(CONFIG_DEF);
    info!("Using config: {}", config_file);

    // config
    let mut settings = Config::default();
    settings
        // Add in `./Settings.toml`
        .merge(File::with_name(config_file))
        .unwrap()
        // Add in settings from the environment (with a prefix of APP)
        // Eg.. `APP_DEBUG=1 ./target/app` would set the `debug` key
        .merge(Environment::with_prefix(CONFIG_ENV))
        .unwrap();

    // logging
    if tui_logger::init_logger(log::LevelFilter::Debug).is_err() {
        // failed initializing logger
        return Err("failed initializing logger".to_string().into());
    }
    tui_logger::set_default_level(log::LevelFilter::Debug);
    tui_logger::set_level_for_target("h2", LevelFilter::Warn);
    tui_logger::set_level_for_target("h2::client", LevelFilter::Warn);
    tui_logger::set_level_for_target("h2::codec::framed_read", LevelFilter::Warn);
    tui_logger::set_level_for_target("h2::codec::framed_write", LevelFilter::Warn);
    tui_logger::set_level_for_target("h2::proto::connection", LevelFilter::Warn);
    tui_logger::set_level_for_target("h2::proto::settings", LevelFilter::Warn);
    tui_logger::set_level_for_target("tower", LevelFilter::Warn);
    tui_logger::set_level_for_target("tower::buffer::worker", LevelFilter::Warn);
    tui_logger::set_level_for_target("hyper", LevelFilter::Warn);
    tui_logger::set_level_for_target("hyper::client::connect::http", LevelFilter::Warn);

    let exit_flag = Arc::new(AtomicBool::new(false));

    // intercom channels
    let (tx_event, mut rx_event) = mpsc::channel::<Event>(16);
    let (tx_command, rx_command) = mpsc::channel::<Command>(16);

    // launch input / ticks handler
    let tick_rate = Duration::from_millis(250);
    let exit_flag_copy = exit_flag.clone();
    let tx_event_copy = tx_event.clone();
    let event_handler = tokio::spawn(async move {
        let mut last_tick = Instant::now();
        loop {
            // poll for tick rate duration, if no events, sent tick event.
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));
            if event::poll(timeout).unwrap() {
                if let CEvent::Key(key) = event::read().unwrap() {
                    if tx_event_copy.send(Event::Input(key)).await.is_err() {
                        error!("failed sending key");
                        break;
                    }
                }
            }
            if last_tick.elapsed() >= tick_rate {
                if tx_event_copy.send(Event::Tick).await.is_err() {
                    break;
                }
                last_tick = Instant::now();
            }
            if exit_flag_copy.load(Ordering::SeqCst) {
                break;
            }
        }
        println!("event handler stopped");
    });

    // launch client
    let mut client = MigchatClient::new(&settings, rx_command);
    let exit_flag_copy = exit_flag.clone();
    let tx_event_copy = tx_event.clone();
    let chat_service = tokio::spawn(async move {
        let _ = client.launch(tx_event_copy, exit_flag_copy).await;
        println!("chat service stopped");
    });

    // launch UI
    let user = UserInfo {
        name: settings.get_str("name").unwrap_or("Anonimous".to_string()),
        short_name: settings.get_str("short_name").unwrap_or("Nemo".to_string()),
    };
    let extended_log = settings.get_bool("extended_log").unwrap_or(false);
    tokio::task::block_in_place(move || {
        if enable_raw_mode().is_ok() {
            let mut stdout = stdout();
            if execute!(stdout, EnterAlternateScreen, EnableMouseCapture).is_ok() {
                let backend = CrosstermBackend::new(stdout);
                if let Ok(mut terminal) = Terminal::new(backend) {
                    if terminal.clear().is_ok() {
                        let mut app = ui::App::new(user, tx_command, tx_event, extended_log);
                        loop {
                            if terminal.draw(|f| ui::draw(f, &mut app)).is_err() {
                                println!("failed drawing UI");
                                break;
                            }
                            if let Some(event) = rx_event.blocking_recv() {
                                match event {
                                    Event::Input(event) => match event.code {
                                        KeyCode::Esc => app.on_esc(),
                                        KeyCode::Enter => app.on_enter(),
                                        KeyCode::Char(c) => app.on_key(
                                            c,
                                            event.modifiers.contains(KeyModifiers::CONTROL),
                                            event.modifiers.contains(KeyModifiers::ALT),
                                        ),
                                        KeyCode::Left => app.on_left(),
                                        KeyCode::Up => app.on_up(),
                                        KeyCode::Right => app.on_right(),
                                        KeyCode::Down => app.on_down(),
                                        KeyCode::Backspace => app.on_backspace(),
                                        _ => {}
                                    },
                                    Event::Tick => {
                                        app.on_tick();
                                    }
                                    Event::Client(chat_event) => match chat_event {
                                        ChatRoomEvent::Registered(user_id) => {
                                            app.on_registered(user_id);
                                        }
                                        ChatRoomEvent::UserEntered(user) => {
                                            app.on_user_entered(user);
                                        }
                                        ChatRoomEvent::ChatUpdated(chat) => {
                                            app.on_chat_updated(chat);
                                        }
                                        ChatRoomEvent::Invitation(invitation) => {
                                            app.on_get_invited(invitation);
                                        }
                                        ChatRoomEvent::NewPost(post) => {
                                            app.on_new_post(post);
                                        }
                                        ChatRoomEvent::ChatDeleted(chat_id) => {
                                            app.on_chat_deleted(chat_id);
                                        }
                                        ChatRoomEvent::UserGone(user_id) => {
                                            app.on_user_gone(user_id);
                                        }
                                    },
                                    Event::Exit => {
                                        exit_flag.store(true, Ordering::SeqCst);
                                        break;
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    let _ = disable_raw_mode();
                    let _ = execute!(
                        terminal.backend_mut(),
                        LeaveAlternateScreen,
                        DisableMouseCapture
                    );
                    let _ = terminal.show_cursor();
                }
            }
        }
        println!("UI stopped");
    });

    let _ = chat_service.await;
    let _ = event_handler.await;
    println!("exitting migchat-client application");
    Ok(())
}
