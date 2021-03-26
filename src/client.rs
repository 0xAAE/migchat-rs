// use std::error::Error;
use clap::{App, Arg};
use config::{Config, Environment, File};
use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use log::{debug, error, info, warn};
use tokio::sync::mpsc;

mod proto;

use proto::chat_room_service_client::ChatRoomServiceClient;
use proto::{event::Payload, Registration, UserInfo};

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

struct MigchatClient {
    name: String,
    short_name: String,
}

impl MigchatClient {
    fn new(settings: &Config) -> Self {
        let name = settings.get_str("name").unwrap_or("Anonimous".to_string());
        let short_name = settings.get_str("short_name").unwrap_or("Nemo".to_string());
        MigchatClient { name, short_name }
    }

    async fn launch(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut client = ChatRoomServiceClient::connect("http://0.0.0.0:50051").await?;

        // register
        let reg_info = UserInfo {
            name: self.name.clone(),
            short_name: self.short_name.clone(),
        };
        info!("registering as {:?}", reg_info);
        let reg_req = tonic::Request::new(reg_info);
        if let Ok(reg_res) = client.register(reg_req).await {
            if let Some(own_uuid) = reg_res.into_inner().uuid {
                // login
                let (tx, mut rx) = mpsc::channel(4);
                tokio::spawn(async move {
                    let reg_info = Registration {
                        uuid: Some(own_uuid),
                    };
                    if let Ok(response) = client.login(tonic::Request::new(reg_info)).await {
                        let mut stream = response.into_inner();
                        while let Some(event) = stream.message().await.ok().flatten() {
                            if let Some(payload) = event.payload {
                                match payload {
                                    Payload::Login(data) => {
                                        let _ = tx.send(data).await;
                                    }
                                    Payload::Invitation(_data) => {
                                        //todo: handle invitation
                                        debug!("todo: handle invitation");
                                    }
                                }
                            } else {
                                error!("no payload in event");
                            }
                        }
                    } else {
                        error!("login failed");
                    }
                });
                if let Some(login) = rx.recv().await {
                    info!("logged as {:?}", login);
                } else {
                    error!("login failed, reply stream closed");
                }
            } else {
                //return Err("bad registration data returned".into());
                warn!("bad registration data returned");
            }
        } else {
            //return Err("registration failed".into());
            warn!("registration failed");
        }
        info!("exitting, bye!");

        Ok(())
    }
}
