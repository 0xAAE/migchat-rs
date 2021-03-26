// use std::error::Error;
use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use log::{debug, error, info, warn};
use tokio::sync::mpsc;

mod proto;

use proto::chat_room_service_client::ChatRoomServiceClient;
use proto::{event::Payload, Registration, UserInfo};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    Builder::from_env(Env::default().default_filter_or("debug,h2=info,tower=info,hyper=info"))
        .target(Target::Stdout)
        .format_timestamp(Some(TimestampPrecision::Seconds))
        .init();
    let mut client = ChatRoomServiceClient::connect("http://0.0.0.0:50051").await?;

    // register
    let reg_info = UserInfo {
        name: "Alexander Avramenko".to_string(),
        short_name: "0xAAE".to_string(),
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
