// use std::error::Error;
use clap::{App, Arg};
use config::{Config, Environment, File};
use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use log::{debug, error, info, warn};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

mod proto;

use proto::chat_room_service_client::ChatRoomServiceClient;
use proto::{event::Payload, Event, Registration, Session, UserInfo};

const CONFIG: &str = "config";
const CONFIG_DEF: &str = "client.toml";
const CONFIG_ENV: &str = "MIGC";

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
    Builder::from_env(Env::default().default_filter_or("debug,h2=info,tower=info,hyper=info"))
        .target(Target::Stdout)
        .format_timestamp(Some(TimestampPrecision::Seconds))
        .init();

    // launch client
    let client = MigchatClient::new(&settings);
    client.launch().await
}

type SharedInvitations = Arc<Mutex<Vec<proto::Invitation>>>;

struct MigchatClient {
    name: String,
    short_name: String,
    invitations: SharedInvitations,
}

impl MigchatClient {
    fn new(settings: &Config) -> Self {
        let name = settings.get_str("name").unwrap_or("Anonimous".to_string());
        let short_name = settings.get_str("short_name").unwrap_or("Nemo".to_string());
        MigchatClient {
            name,
            short_name,
            invitations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    async fn launch(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut client = ChatRoomServiceClient::connect("http://0.0.0.0:50051").await?;

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
                // launch login + accepting invitations in separate task
                //
                let (tx, rx) = oneshot::channel();
                let invitations = self.invitations.clone();
                tokio::spawn(async move {
                    let reg_info = Registration {
                        uuid: Some(own_uuid),
                    };
                    if let Ok(response) = client.login(tonic::Request::new(reg_info)).await {
                        let stream = response.into_inner();
                        MigchatClient::proceed_login_invitations(stream, tx, invitations).await;
                    } else {
                        error!("login failed");
                    }
                });
                //
                // wait until login proceed
                //
                if let Ok(login) = rx.await {
                    info!("logged as {:?}", login);
                } else {
                    error!("login failed, reply stream closed");
                }
            } else {
                warn!("bad registration data returned");
            }
        } else {
            warn!("registration failed");
        }
        info!("exitting, bye!");

        Ok(())
    }

    async fn proceed_login_invitations(
        stream: tonic::Streaming<Event>,
        out_login: oneshot::Sender<Session>,
        out_invites: SharedInvitations,
    ) {
        let mut stream = stream;
        // 1st event = login
        if let Some(login_event) = stream.message().await.ok().flatten() {
            if let Some(payload) = login_event.payload {
                match payload {
                    Payload::Login(login_data) => {
                        let _ = out_login.send(login_data);
                    }
                    Payload::Invitation(data) => {
                        error!("unexpected invitation: {:?}", data);
                    }
                }
            } else {
                error!("no payload in logon result");
            }
        } else {
            error!("stream has closed before logon");
        }
        // further events = invitations
        while let Some(event) = stream.message().await.ok().flatten() {
            if let Some(payload) = event.payload {
                match payload {
                    Payload::Login(data) => {
                        error!("unexpected login: {:?}", data);
                    }
                    Payload::Invitation(data) => {
                        debug!("invitation: {:?}", &data);
                        if let Ok(mut invitations) = out_invites.lock() {
                            invitations.push(data);
                        } else {
                            error!("failed access invitations");
                        }
                    }
                }
            } else {
                error!("no payload in event");
            }
        }
    }
}
