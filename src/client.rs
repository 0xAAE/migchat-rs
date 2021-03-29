// use std::error::Error;
use clap::{App, Arg};
use config::{Config, Environment, File};
use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use log::{debug, error, info, warn, LevelFilter};
use std::{
    error::Error,
    io::stdout,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tonic::transport::Endpoint;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{backend::CrosstermBackend, Terminal};

mod proto;
mod ui;

use proto::chat_room_service_client::ChatRoomServiceClient;
use proto::{
    Invitation, Registration, RequestChats, RequestInvitations, RequestUsers, Session, UserInfo,
};

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
        println!("exitting event handler");
    });

    // launch client
    let mut client = MigchatClient::new(&settings, rx_command);
    let exit_flag_copy = exit_flag.clone();
    let chat_service = tokio::spawn(async move {
        let _ = client.launch(tx_event, exit_flag_copy).await;
        println!("exitting chat service");
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
                        let mut app = ui::App::new(user, tx_command, extended_log);
                        loop {
                            if terminal.draw(|f| ui::draw(f, &mut app)).is_err() {
                                println!("failed drawing UI");
                                break;
                            }
                            if let Some(event) = rx_event.blocking_recv() {
                                match event {
                                    Event::Input(event) => match event.code {
                                        KeyCode::Esc => {
                                            if app.test_exit() {
                                                let _ = disable_raw_mode();
                                                let _ = execute!(
                                                    terminal.backend_mut(),
                                                    LeaveAlternateScreen,
                                                    DisableMouseCapture
                                                );
                                                let _ = terminal.show_cursor();
                                                exit_flag.store(true, Ordering::SeqCst);
                                                break;
                                            }
                                        }
                                        KeyCode::Enter => app.on_enter(),
                                        KeyCode::Char(c) => app.on_key(c),
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
                                        ChatRoomEvent::UserEntered(user) => {
                                            app.on_user_entered(user);
                                        }
                                        _ => {
                                            error!(
                                                "handling this chat event is not implemented yet"
                                            );
                                        }
                                    },
                                }
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
        }
        println!("exitting TUI");
    });

    let _ = chat_service.await;
    let _ = event_handler.await;
    println!("exitting migchat-client application");
    Ok(())
}

pub enum ChatRoomEvent {
    UserEntered(proto::User),      // session_id, name, short_name
    UserGone(u32),                 // session_id
    ChatUpdated(proto::Chat),      // chat_id
    ChatDeleted(u32),              // chat_id
    Invitation(proto::Invitation), // session_id (=user), chat_id
    NewPost(proto::Post),          // chat_id, session_id (=user), text, [attachments]
}

pub enum Command {
    CreateChat(proto::ChatInfo), // create new chat
    Invite(proto::Invitation),   // invite user to chat
    Post(proto::Post),           // send new post
    Exit,                        // exit chat room
}

struct MigchatClient {
    name: String,
    short_name: String,
    login: Option<Session>,
    rx_command: mpsc::Receiver<Command>,
    test_users: Option<Vec<proto::User>>,
}

impl MigchatClient {
    fn new(settings: &Config, rx_command: mpsc::Receiver<Command>) -> Self {
        let name = settings.get_str("name").unwrap_or("Anonimous".to_string());
        let short_name = settings.get_str("short_name").unwrap_or("Nemo".to_string());
        // if there are test users in settings, create them
        let test_users = if let Ok(config_test_users) = settings.get_array("test_users") {
            let mut next_id = 100;
            let mut test_users = Vec::new();
            for item in config_test_users {
                if let Ok(names) = item.into_array() {
                    if names.len() == 2 {
                        test_users.push(proto::User {
                            session_id: next_id,
                            name: names[0].to_string(),
                            short_name: names[1].to_string(),
                        });
                        next_id += 1;
                    }
                }
            }
            Some(test_users)
        } else {
            None
        };

        MigchatClient {
            name,
            short_name,
            login: None,
            rx_command,
            test_users,
        }
    }

    async fn launch(
        &mut self,
        tx_event: mpsc::Sender<Event>,
        exit_flag: Arc<AtomicBool>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let channel = Endpoint::from_static("http://0.0.0.0:50051")
            .timeout(Duration::from_secs(10))
            .connect()
            .await?;

        if let Some(test_users) = self.test_users.take() {
            for u in test_users {
                if let Err(e) = tx_event
                    .send(Event::Client(ChatRoomEvent::UserEntered(u)))
                    .await
                {
                    error!("failed send test user: {}", e);
                }
            }
        }

        let mut client = ChatRoomServiceClient::new(channel);

        // register
        let user_info = UserInfo {
            name: self.name.clone(),
            short_name: self.short_name.clone(),
        };
        info!("registering as {:?}", user_info);
        let reg_req = tonic::Request::new(user_info);

        if let Ok(reg_res) = client.register(reg_req).await {
            if let Some(own_uuid) = reg_res.into_inner().uuid {
                //
                // login
                //
                if let Ok(response) = client
                    .login(tonic::Request::new(Registration {
                        uuid: Some(own_uuid),
                    }))
                    .await
                {
                    let login = response.into_inner();
                    let session_id = login.id;
                    info!("logged as {:?}", &login);
                    self.login = Some(login);
                    //
                    // launch accepting invitations in separate task
                    //
                    let req_invitations = RequestInvitations { session_id };
                    tokio::spawn(async move {
                        match client
                            .get_invitations(tonic::Request::new(req_invitations))
                            .await
                        {
                            Ok(response) => {
                                let mut stream = response.into_inner();
                                while let Some(invitation) = stream.message().await.ok().flatten() {
                                    debug!("new invitation: {:?}", &invitation);
                                    if let Err(e) = tx_event
                                        .send(Event::Client(ChatRoomEvent::Invitation(invitation)))
                                        .await
                                    {
                                        error!("failed to transfer invitation to UI {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("no more invitations: {}", e);
                            }
                        }
                    });
                } else {
                    error!("login failed");
                }
            } else {
                warn!("bad registration data returned");
            }
        } else {
            warn!("registration failed");
        }

        loop {
            match tokio::time::timeout(Duration::from_millis(500), self.rx_command.recv()).await {
                Err(_) => {
                    // timeout
                    if exit_flag.load(Ordering::SeqCst) {
                        break;
                    }
                }
                Ok(command) => match command {
                    Some(command) => match command {
                        Command::CreateChat(_info) => {
                            error!("creating chat is not implemented yet");
                        }
                        Command::Invite(_invitation) => {
                            error!("inviting others is not implemented yet");
                        }
                        Command::Post(_post) => {
                            error!("sendibng posts is not omplementing yet");
                        }
                        Command::Exit => {
                            error!("exitting chat room is not implemented yet");
                        }
                    },
                    None => {
                        info!("command channel has closed by receiver");
                        break;
                    }
                },
            }
        }
        // tokio::select! {
        //     _ = tokio::time::timeout(Duration::from_millis(250)) => {},
        //     Command() = self.rx_command.recv()
        // }
        // while !exit_flag.load(Ordering::SeqCst) {
        //     tokio::time::sleep().await;
        // }
        info!("exitting, bye!");

        Ok(())
    }
}
