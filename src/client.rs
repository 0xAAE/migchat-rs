// use std::error::Error;
use clap::{App, Arg};
use config::{Config, Environment, File};
use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use log::{debug, error, info, warn};
use std::{
    error::Error,
    io::stdout,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tonic::transport::Endpoint;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode},
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

enum Event<I> {
    Input(I),
    Tick,
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
    // Builder::from_env(Env::default().default_filter_or("error,h2=warn,tower=warn,hyper=warn"))
    //     .target(Target::Stdout)
    //     .format_timestamp(Some(TimestampPrecision::Seconds))
    //     .init();
    if tui_logger::init_logger(log::LevelFilter::Debug).is_err() {
        // failed initializing logger
        return Err("failed initializing logger".to_string().into());
    }
    tui_logger::set_default_level(log::LevelFilter::Debug);

    // launch input / ticks handler
    let (tx_event, mut rx_event) = mpsc::channel(16);
    let tick_rate = Duration::from_millis(250);
    tokio::spawn(async move {
        let mut last_tick = Instant::now();
        loop {
            // poll for tick rate duration, if no events, sent tick event.
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));
            if event::poll(timeout).unwrap() {
                if let CEvent::Key(key) = event::read().unwrap() {
                    if tx_event.send(Event::Input(key)).await.is_err() {
                        break;
                    }
                    if key.code == KeyCode::Esc {
                        break;
                    }
                }
            }
            if last_tick.elapsed() >= tick_rate {
                let _ = tx_event.send(Event::Tick).await;
                last_tick = Instant::now();
            }
        }
        print!("event handler is closed");
    });

    // launch client
    let mut client = MigchatClient::new(&settings);
    tokio::spawn(async move {
        let _ = client.launch().await;
    });

    // launch UI
    let users: ui::SharedUsers = Arc::new(Mutex::new(Vec::new()));
    let chats: ui::SharedChats = Arc::new(Mutex::new(Vec::new()));
    let users_copy = users.clone();
    let chats_copy = chats.clone();
    tokio::task::block_in_place(move || {
        if enable_raw_mode().is_ok() {
            let mut stdout = stdout();
            if execute!(stdout, EnterAlternateScreen, EnableMouseCapture).is_ok() {
                let backend = CrosstermBackend::new(stdout);
                if let Ok(mut terminal) = Terminal::new(backend) {
                    if terminal.clear().is_ok() {
                        let mut app = ui::App::new(users_copy, chats_copy);
                        loop {
                            if terminal.draw(|f| ui::draw(f, &mut app)).is_err() {
                                // failed draw
                                break;
                            }
                            if let Some(event) = rx_event.blocking_recv() {
                                match event {
                                    Event::Input(event) => match event.code {
                                        KeyCode::Esc => {
                                            let _ = disable_raw_mode();
                                            let _ = execute!(
                                                terminal.backend_mut(),
                                                LeaveAlternateScreen,
                                                DisableMouseCapture
                                            );
                                            let _ = terminal.show_cursor();
                                            break;
                                        }
                                        KeyCode::Char(c) => app.on_key(c),
                                        KeyCode::Left => app.on_left(),
                                        KeyCode::Up => app.on_up(),
                                        KeyCode::Right => app.on_right(),
                                        KeyCode::Down => app.on_down(),
                                        _ => {}
                                    },
                                    Event::Tick => {
                                        app.on_tick();
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
        }
    });

    print!("exitting migchat-client\n");
    Ok(())
}

type SharedInvitations = Arc<Mutex<Vec<proto::Invitation>>>;

struct MigchatClient {
    name: String,
    short_name: String,
    login: Option<Session>,
    invitations: SharedInvitations,
}

impl MigchatClient {
    fn new(settings: &Config) -> Self {
        let name = settings.get_str("name").unwrap_or("Anonimous".to_string());
        let short_name = settings.get_str("short_name").unwrap_or("Nemo".to_string());
        MigchatClient {
            name,
            short_name,
            login: None,
            invitations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn launch(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let channel = Endpoint::from_static("http://0.0.0.0:50051")
            .timeout(Duration::from_secs(10))
            .connect()
            .await?;

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
                    let invitations = self.invitations.clone();
                    let req_invitations = RequestInvitations { session_id };
                    tokio::spawn(async move {
                        match client
                            .get_invitations(tonic::Request::new(req_invitations))
                            .await
                        {
                            Ok(response) => {
                                let mut stream = response.into_inner();
                                while let Some(invitation) = stream.message().await.ok().flatten() {
                                    debug!("invitation: {:?}", &invitation);
                                    if let Ok(mut invitations) = invitations.lock() {
                                        invitations.push(invitation);
                                    } else {
                                        error!("failed access invitations");
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("no more invitations: {:?}", e);
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
        tokio::time::sleep(Duration::from_secs(5)).await;
        info!("exitting, bye!");

        Ok(())
    }
}
