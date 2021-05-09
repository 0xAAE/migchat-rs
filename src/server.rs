use clap::{App, Arg};
use config::{Config, Environment, File};
use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use log::{debug, error, info, warn};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::Path,
    sync::{Arc, RwLock},
};
use tokio::sync::mpsc;
use tonic::transport::Server;

mod proto;
mod storage;

use proto::chat_room_service_server::ChatRoomServiceServer;
pub use proto::{Chat, ChatId, User, UserId};
use proto::{Invitation, Post};
use storage::Storage;

type InternalError = Box<dyn std::error::Error + Send + Sync + 'static>;

mod server_service;

const APP_NAME: &str = "migchat-server";
const CONFIG: &str = "config";
const CONFIG_DEF: &str = "migchat-server.toml";
const CONFIG_ENV: &str = "MIGSRV";
const DEF_ENDPOINT: &str = "0.0.0.0:50051";
const DEF_DB_FILE: &str = "migchat_server.db";

#[derive(Clone)]
enum UserChanged {
    Info(Arc<User>),
    Online(UserId),
    Offline(UserId),
}

#[derive(Clone)]
enum ChatChanged {
    Updated(Arc<Chat>),
    Closed(ChatId),
}

pub struct ChatRoomImpl {
    storage: Storage,
    // new users:
    users_listeners: RwLock<HashMap<UserId, mpsc::Sender<UserChanged>>>,
    // new invitations:
    invitations_listeners: RwLock<HashMap<UserId, mpsc::Sender<Invitation>>>,
    // new chats:
    chats_listeners: RwLock<HashMap<UserId, mpsc::Sender<ChatChanged>>>,
    // new posts:
    posts_listeners: RwLock<HashMap<UserId, mpsc::Sender<Arc<Post>>>>,
    // users statuses, stores online users:
    online_users: RwLock<HashSet<UserId>>,
}

impl ChatRoomImpl {
    fn new<P: AsRef<Path>>(db_file: P) -> Result<Self, InternalError> {
        let storage = Storage::new(db_file)?;
        Ok(Self {
            storage,
            // chats: RwLock::new(HashMap::new()),
            users_listeners: RwLock::new(HashMap::new()),
            invitations_listeners: RwLock::new(HashMap::new()),
            chats_listeners: RwLock::new(HashMap::new()),
            posts_listeners: RwLock::new(HashMap::new()),
            online_users: RwLock::new(HashSet::new()),
        })
    }

    fn actualize_chat_listeners(&self) {
        if let Ok(mut listeners) = self.chats_listeners.write() {
            let before = listeners.len();
            listeners.retain(|_k, v| !v.is_closed());
            let removed = before - listeners.len();
            if removed > 0 {
                info!(
                    "{} outdated chat listener(s) was/were found and removed",
                    removed
                );
            }
        }
    }

    // returns true if all listeners were notified, otherwise if at least one
    // failed to notify returns false
    // call to actualize_chat_listeners() is recommended if the method returns false
    async fn notify_chat_changed(&self, notification: ChatChanged) -> bool {
        let mut send_list = Vec::new();
        if let Ok(listeners) = self.chats_listeners.read() {
            for listener in listeners.values() {
                send_list.push(listener.clone());
            }
        }
        if !send_list.is_empty() {
            let mut fails = false;
            for tx in send_list {
                if let Err(e) = tx.send(notification.clone()).await {
                    error!("failed to broadcast new chat: {}", e);
                    fails = true;
                }
            }
            !fails
        } else {
            true
        }
    }

    fn actualize_user_listeners(&self) {
        if let Ok(mut listeners) = self.users_listeners.write() {
            let before = listeners.len();
            listeners.retain(|_k, v| !v.is_closed());
            let removed = before - listeners.len();
            if removed > 0 {
                info!(
                    "{} outdated user listener(s) was/were found and removed",
                    removed
                );
            }
        }
    }

    // returns true if all listeners were notified, orhewise, if at least one
    // failed to notify, returns false
    // call to actualize_user_listeners() is recommended if the method returns false
    async fn notify_user_changed(&self, notification: UserChanged) -> bool {
        let mut send_list = Vec::new();
        if let Ok(listeners) = self.users_listeners.read() {
            for listener in listeners.values() {
                send_list.push(listener.clone());
            }
        }
        if !send_list.is_empty() {
            let mut fails = false;
            for tx in send_list {
                if let Err(e) = tx.send(notification.clone()).await {
                    error!("failed to broadcast user info: {}", e);
                    fails = true;
                }
            }
            !fails
        } else {
            true
        }
    }

    fn actualize_post_listeners(&self) {
        if let Ok(mut listeners) = self.posts_listeners.write() {
            let before = listeners.len();
            listeners.retain(|_k, v| !v.is_closed());
            let removed = before - listeners.len();
            if removed > 0 {
                info!(
                    "{} outdated post listener(s) was/were found and removed",
                    removed
                );
            }
        }
    }

    // notifies all chat members about new post
    // returns true if all listeners were notified, orhewise, if at least one
    // failed to notify, returns false
    // call to actualize_post_listeners() is recommended if the method returns false
    async fn notify_new_post(&self, post: Post) -> bool {
        // найти чат по id, запомнить список user_id-получателей - всех участников, включая автора поста
        let mut users = Vec::new();
        if let Ok(Some(chat)) = self.storage.read_chat(post.chat_id) {
            for u in chat.users.as_slice() {
                users.push(*u);
            }
        }
        if users.is_empty() {
            true
        } else {
            // создать список каналов к получателям поста из списка получателей
            let mut send_list = Vec::new();
            if let Ok(listeners) = self.posts_listeners.read() {
                for user_id in users {
                    debug!("search channel to {} for post", user_id);
                    if let Some(listener) = listeners.get(&user_id) {
                        debug!("found channel to {} for post", user_id);
                        send_list.push(listener.clone());
                    }
                }
            }
            // подготовить пост и разослать
            if !send_list.is_empty() {
                let send_post = Arc::new(post);
                let mut fails = false;
                for tx in send_list {
                    if let Err(e) = tx.send(send_post.clone()).await {
                        error!("failed to send post: {}", e);
                        fails = true;
                    }
                }
                !fails
            } else {
                true
            }
        }
    }
}

impl fmt::Debug for ChatRoomImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatRoomImpl")
            .field("users_listeners", &format!("{:?}", self.users_listeners))
            .field(
                "invitations_listeners",
                &format!("{:?}", self.invitations_listeners),
            )
            .field("chats_listeners", &format!("{:?}", self.chats_listeners))
            .field("posts_listeners", &format!("{:?}", self.posts_listeners))
            .finish()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // commnad line
    let matches = App::new(APP_NAME)
        .version("0.1")
        .author("0xAAE <avramenko.a@gmail.com>")
        .about(
            "The MiGChat server. Use MIGSRV_* environment variables to override config file settings",
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

    Builder::from_env(Env::default().default_filter_or("debug,h2=info,tower=info,hyper=info"))
        .target(Target::Stdout)
        .format_timestamp(Some(TimestampPrecision::Seconds))
        .init();

    let endpoint = if let Ok(addr) = settings.get_str("endpoint") {
        addr
    } else {
        warn!("server connection is not set, use default {}", DEF_ENDPOINT);
        String::from(DEF_ENDPOINT)
    };

    let dbfile = if let Ok(name) = settings.get_str("dbfile") {
        info!("use {} as DB storage", name);
        name
    } else {
        warn!("DB file is not set, use default {}", DEF_DB_FILE);
        String::from(DEF_DB_FILE)
    };

    let addr = endpoint.parse().unwrap();
    let chat_room = ChatRoomImpl::new(dbfile)?;
    info!("Chat room is listening on {}", addr);

    let svc = ChatRoomServiceServer::new(chat_room);
    Server::builder().add_service(svc).serve(addr).await?;

    Ok(())
}
