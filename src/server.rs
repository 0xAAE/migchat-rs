use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use log::{debug, error, info};
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
            let removed = listeners.len() - before;
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
            let removed = listeners.len() - before;
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

    async fn notify_new_post(&self, post: Post) {
        // найти чат по id, запомнить список user_id-получателей - всех участников, включая автора поста
        let mut users = Vec::new();
        if let Ok(Some(chat)) = self.storage.read_chat(post.chat_id) {
            for u in chat.users.as_slice() {
                users.push(*u);
            }
        }
        if users.is_empty() {
            return;
        }
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
            for tx in send_list {
                if let Err(e) = tx.send(send_post.clone()).await {
                    error!("failed to send post: {}", e);
                    //no break;
                }
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
    Builder::from_env(Env::default().default_filter_or("debug,h2=info,tower=info,hyper=info"))
        .target(Target::Stdout)
        .format_timestamp(Some(TimestampPrecision::Seconds))
        .init();

    let addr = "0.0.0.0:50051".parse().unwrap();
    let chat_room = ChatRoomImpl::new("migchat_server.db")?;
    info!("Chat room is listening on {}", addr);

    let svc = ChatRoomServiceServer::new(chat_room);
    Server::builder().add_service(svc).serve(addr).await?;

    Ok(())
}
