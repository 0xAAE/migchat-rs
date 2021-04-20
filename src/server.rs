use bytes::BytesMut;
use env_logger::{fmt::TimestampPrecision, Builder, Env, Target};
use futures::Stream; //, StreamExt};
use fxhash::FxHasher64;
use jammdb::{self, Data};
use log::{debug, error, info};
use prost::Message;
use std::{
    collections::{HashMap, HashSet},
    fmt,
    hash::Hasher,
    ops::Deref,
    path::Path,
    pin::Pin,
    sync::{Arc, RwLock},
};
use tokio::sync::mpsc;
use tonic::{transport::Server, Request, Response, Status};

mod proto;

use proto::chat_room_service_server::{ChatRoomService, ChatRoomServiceServer};
use proto::{
    Chat, ChatId, ChatInfo, ChatReference, Invitation, Post, Registration, Result as RpcResult,
    UpdateChats, UpdateUsers, User, UserId, UserInfo, NOT_CHAT_ID, NOT_POST_ID,
};

type InternalError = Box<dyn std::error::Error + Send + Sync + 'static>;

fn get_user_id(user: &UserInfo) -> u64 {
    let mut hasher = FxHasher64::default();
    hasher.write(user.name.as_bytes());
    hasher.write(user.short_name.as_bytes());
    hasher.finish()
}

fn new_post_id() -> u64 {
    let mut v = NOT_POST_ID;
    while v == NOT_POST_ID {
        v = rand::random();
    }
    v
}

fn new_chat_id() -> u64 {
    let mut v = NOT_CHAT_ID;
    while v == NOT_CHAT_ID {
        v = rand::random();
    }
    v
}

// chat must have a predictable reproducable id
// based on its description if is not empty or its members,
// the reasons are
// - avoid having chats with empty names in chat list
// - display such a chat like a dialog of its members
// - chat must be discoverable by any member instead of creating new and new ones
fn get_chat_id(description: &str, users: &[UserId]) -> u64 {
    let mut hasher = FxHasher64::default();
    if !description.is_empty() {
        hasher.write(description.as_bytes());
        hasher.finish()
    } else if !users.is_empty() {
        for id in users {
            hasher.write(&id.to_le_bytes());
        }
        hasher.finish()
    } else {
        new_chat_id()
    }
}

const BUCKET_USERS: &str = "users";
const BUCKET_CHATS: &str = "chats";
const BUCKET_POSTS: &str = "posts";

pub struct ChatRoomImpl {
    db: jammdb::DB,
    // new users:
    users_listeners: RwLock<HashMap<UserId, mpsc::Sender<Arc<User>>>>,
    // new invitations:
    invitations_listeners: RwLock<HashMap<UserId, mpsc::Sender<Invitation>>>,
    // new chats:
    chats_listeners: RwLock<HashMap<UserId, mpsc::Sender<Arc<Chat>>>>,
    // new posts
    posts_listeners: RwLock<HashMap<UserId, mpsc::Sender<Arc<Post>>>>,
}

impl ChatRoomImpl {
    fn new<P: AsRef<Path>>(db_file: P) -> Result<Self, InternalError> {
        let db = jammdb::DB::open(db_file)?;
        // create users bucket in DB if not exists
        let tx = db.tx(true)?;
        match tx.create_bucket(BUCKET_USERS) {
            Ok(_) => {}
            Err(jammdb::Error::BucketExists) => {}
            Err(e) => return Err(format!("{}", e).into()),
        }
        tx.commit()?;
        // create chats bucket in DB if not exists
        let tx = db.tx(true)?;
        match tx.create_bucket(BUCKET_CHATS) {
            Ok(_) => {}
            Err(jammdb::Error::BucketExists) => {}
            Err(e) => return Err(format!("{}", e).into()),
        }
        tx.commit()?;
        // create posts bucket in DB if not exists
        let tx = db.tx(true)?;
        match tx.create_bucket(BUCKET_POSTS) {
            Ok(_) => {}
            Err(jammdb::Error::BucketExists) => {}
            Err(e) => return Err(format!("{}", e).into()),
        }
        tx.commit()?;

        Ok(Self {
            db,
            // chats: RwLock::new(HashMap::new()),
            users_listeners: RwLock::new(HashMap::new()),
            invitations_listeners: RwLock::new(HashMap::new()),
            chats_listeners: RwLock::new(HashMap::new()),
            posts_listeners: RwLock::new(HashMap::new()),
        })
    }

    fn read_from_db<M: Message + Default>(
        &self,
        bucket_name: &str,
        id: &[u8],
    ) -> Result<Option<M>, InternalError> {
        match self.db.tx(false) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => match bucket.get(id) {
                    None => Ok(None),
                    Some(record) => {
                        let bin = record.kv().value();
                        match M::decode(bin) {
                            Ok(item) => Ok(Some(item)),
                            Err(e) => {
                                error!("protobuf parse, {}", e);
                                Err(format!("{}", e).into())
                            }
                        }
                    }
                },
                Err(e) => Err(format!("{}", e).into()),
            },
            Err(e) => Err(format!("{}", e).into()),
        }
    }

    fn write_to_db<M: Message>(
        &self,
        bucket_name: &str,
        id: &[u8],
        item: &M,
    ) -> Result<(), InternalError> {
        match self.db.tx(true) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => {
                    let mut buf = BytesMut::new();
                    match item.encode(&mut buf) {
                        Ok(_) => match bucket.put(id, buf) {
                            Ok(_) => tx.commit().map_err(|e| e.into()),
                            Err(e) => Err(e.into()),
                        },
                        Err(e) => Err(e.into()),
                    }
                }
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    fn update_in_db<M: Message + Default + Clone, F: FnMut(&mut M)>(
        &self,
        bucket_name: &str,
        id: &[u8],
        mut updater: F,
    ) -> Result<Option<M>, InternalError> {
        match self.db.tx(true) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => {
                    if let Some(kv) = bucket.get_kv(id) {
                        // read
                        let bin = kv.value();
                        match M::decode(bin) {
                            Ok(item) => {
                                let mut new_item = item.clone();
                                // update
                                updater(&mut new_item);
                                // serialize
                                let mut buf = BytesMut::new();
                                match new_item.encode(&mut buf) {
                                    Ok(_) => match bucket.put(id, buf) {
                                        // store into db
                                        Ok(_) => tx
                                            .commit()
                                            .map(|_| Some(new_item))
                                            .map_err(|e| e.into()),
                                        Err(e) => Err(e.into()),
                                    },
                                    Err(e) => Err(e.into()),
                                }
                            }
                            Err(e) => {
                                error!("protobuf parse, {}", e);
                                Err(e.into())
                            }
                        }
                    } else {
                        Ok(None)
                    }
                }
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    fn read_all_from_db<M: Message + Default>(
        &self,
        bucket_name: &str,
    ) -> Result<Vec<M>, InternalError> {
        match self.db.tx(false) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => {
                    let mut items = Vec::new();
                    for data in bucket.cursor() {
                        match &*data {
                            Data::Bucket(_) => {
                                error!("internal error, BUCKET_USERS contains child bucket")
                            }
                            Data::KeyValue(kv) => {
                                let bin = kv.value();
                                match M::decode(bin) {
                                    Ok(user) => items.push(user),
                                    Err(e) => error!("internal error, {}", e),
                                }
                            }
                        }
                    }
                    Ok(items)
                }
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    fn remove_from_db<M: Message>(
        &self,
        bucket_name: &str,
        id: &[u8],
    ) -> Result<(), InternalError> {
        // does not require writable tx!
        // see https://docs.rs/jammdb/0.5.0/jammdb/struct.Bucket.html#method.delete
        match self.db.tx(false) {
            Ok(tx) => match tx.get_bucket(bucket_name) {
                Ok(bucket) => bucket
                    .delete(id)
                    .map(|_| ())
                    .map_err(|e| format!("{}", e).into()),
                Err(e) => Err(e.into()),
            },
            Err(e) => Err(e.into()),
        }
    }

    async fn notify_chat_updated(&self, chat: Chat) {
        let mut send_list = Vec::new();
        if let Ok(listeners) = self.chats_listeners.read() {
            for listener in listeners.values() {
                send_list.push(listener.clone());
            }
        }
        if !send_list.is_empty() {
            let send_chat = Arc::new(chat);
            for tx in send_list {
                if let Err(e) = tx.send(send_chat.clone()).await {
                    error!("failed to broadcast new chat: {}", e);
                    // no break;
                }
            }
        }
    }

    async fn notify_chat_closed(&self, _chat_id: ChatId) {
        //todo: implement notifying through "chat updated" channel
    }

    async fn notify_user_entered(&self, user: User) {
        let mut send_list = Vec::new();
        if let Ok(listeners) = self.users_listeners.read() {
            for listener in listeners.values() {
                send_list.push(listener.clone());
            }
        }
        if !send_list.is_empty() {
            let send_user = Arc::new(user);
            for tx in send_list {
                if let Err(e) = tx.send(send_user.clone()).await {
                    error!("failed to broadcast new user: {}", e);
                    //no break;
                }
            }
        }
    }

    async fn notify_user_gone(&self, _user_id: UserId) {
        //todo: implement sending gone info through listeners (enum?)
    }

    async fn notify_new_post(&self, post: Post) {
        // найти чат по id, запомнить список user_id-получателей - всех участников, включая автора поста
        let mut users = Vec::new();
        if let Ok(Some(chat)) =
            self.read_from_db::<proto::Chat>(BUCKET_CHATS, &post.chat_id.to_le_bytes())
        {
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

#[tonic::async_trait]
impl ChatRoomService for ChatRoomImpl {
    #[doc = " Sends a reqistration request"]
    async fn register(&self, request: Request<UserInfo>) -> Result<Response<Registration>, Status> {
        debug!("register(): {:?}", &request);
        let user_info = request.into_inner();
        let id = get_user_id(&user_info);
        // test existing
        match self.read_from_db::<proto::User>(BUCKET_USERS, &id.to_le_bytes()) {
            Err(e) => return Err(tonic::Status::internal(format!("{}", e))),
            Ok(opt) => {
                if let Some(u) = opt {
                    debug!("{} ({}) already registered", u.short_name, u.name);
                    return Ok(Response::new(Registration { user_id: id }));
                }
            }
        }
        let new_user = User {
            id,
            name: user_info.name,
            short_name: user_info.short_name,
        };
        // store new user
        self.notify_user_entered(new_user.clone()).await;
        if let Err(e) = self.write_to_db(BUCKET_USERS, &new_user.id.to_le_bytes(), &new_user) {
            Err(tonic::Status::internal(format!("{}", e)))
        } else {
            Ok(Response::new(Registration { user_id: id }))
        }
    }

    #[doc = "Server streaming response type for the GetInvitations method."]
    type GetInvitationsStream =
        Pin<Box<dyn Stream<Item = Result<Invitation, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for invitations"]
    async fn get_invitations(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Self::GetInvitationsStream>, tonic::Status> {
        // get source channel of invitations
        debug!("get_invitations(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel(4);
        if let Ok(mut listeners) = self.invitations_listeners.write() {
            // test alive
            listeners.retain(|k, v| {
                if v.is_closed() {
                    debug!("stop streaming invitations to {}", k);
                    false
                } else {
                    true
                }
            });
            // add new
            listeners.insert(user_id, listener);
        } else {
            return Err(tonic::Status::internal("no access to invitation channel"));
        };
        // launch stream source
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            debug!("start streaming invitations to {}", user_id);
            let mut notifier = notifier;
            while let Some(invitation) = notifier.recv().await {
                if let Err(e) = tx.send(Ok(invitation)).await {
                    error!("failed streaming invoitations: {}", e);
                    break;
                }
            }
            debug!("stream of invitations to {} has stopped", user_id);
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = " Sends a logout request using the first invitation from the server"]
    async fn logout(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("logout(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        if let Ok(mut listeners) = self.users_listeners.write() {
            if listeners.remove(&user_id).is_some() {
                debug!("stop streaming users to {}", user_id);
            }
        } else {
            error!("failed locking users listeners (logout)");
        }
        if let Ok(mut listeners) = self.chats_listeners.write() {
            if listeners.remove(&user_id).is_some() {
                debug!("stop streaming chats to {}", user_id);
            }
        } else {
            error!("failed locking chats listeners (logout)");
        }
        if let Ok(mut listeners) = self.invitations_listeners.write() {
            if listeners.remove(&user_id).is_some() {
                debug!("stop streaming invitations to {}", user_id);
            }
        } else {
            error!("failed locking invitations listeners (logout)");
        }
        if let Ok(mut listeners) = self.posts_listeners.write() {
            if listeners.remove(&user_id).is_some() {
                debug!("stop streaming posts to {}", user_id);
            }
        } else {
            error!("failed locking posts listeners (logout)");
        }
        self.notify_user_gone(user_id).await;
        Ok(Response::new(RpcResult {
            ok: true,
            description: String::from("logout successful"),
        }))
    }

    #[doc = "Server streaming response type for the GetPosts method."]
    type GetPostsStream =
        Pin<Box<dyn Stream<Item = Result<Post, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for incoming posts"]
    async fn get_posts(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Self::GetPostsStream>, tonic::Status> {
        debug!("get_posts(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel::<Arc<Post>>(4);
        if let Ok(mut listeners) = self.posts_listeners.write() {
            // test alive
            listeners.retain(|k, v| {
                if v.is_closed() {
                    debug!("stop streaming posts to {}", k);
                    false
                } else {
                    true
                }
            });
            // add new
            listeners.insert(user_id, listener);
        } else {
            return Err(tonic::Status::internal("no access to posts listeners"));
        }
        //todo: collect existing posts
        let existing = Vec::new();
        // if let Ok(users) = self.users.read() {
        //     for user in users.values() {
        //         if user.user_id != user_id {
        //             existing.push(user.clone());
        //         }
        //     }
        // } else {
        //     error!("failed to read existing users");
        // }
        // start permanent listener that streams data to remote client
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            debug!("start streaming posts to {}", user_id);
            if !existing.is_empty() {
                // send existing posts
                for post in existing {
                    if let Err(e) = tx.send(Ok(post)).await {
                        error!("failed sending existing post: {}", e);
                    }
                }
            }
            // re-translate new users
            let mut notifier = notifier;
            while let Some(post) = notifier.recv().await {
                debug!("re-translating new post to {}", user_id);
                if let Err(e) = tx.send(Ok((*post).clone())).await {
                    error!("failed sending post: {}, stop", e);
                    break;
                }
            }
            debug!("stream of posts to {} has stopped", user_id);
        });
        // start streaming activity, data consumer
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = "Server streaming response type for the GetUsers method."]
    type GetUsersStream =
        Pin<Box<dyn Stream<Item = Result<UpdateUsers, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for contacts list"]
    async fn get_users(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Self::GetUsersStream>, tonic::Status> {
        debug!("get_users(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel::<Arc<User>>(4);
        if let Ok(mut listeners) = self.users_listeners.write() {
            // test alive
            listeners.retain(|k, v| {
                if v.is_closed() {
                    debug!("stop streaming users to {}", k);
                    false
                } else {
                    true
                }
            });
            // add new
            listeners.insert(user_id, listener);
        } else {
            return Err(tonic::Status::internal("no access to users listeners"));
        }
        // collect existing users
        let mut existing = Vec::new();
        if let Ok(users) = self.read_all_from_db::<proto::User>(BUCKET_USERS) {
            for user in users {
                if user.id != user_id {
                    existing.push(user.clone());
                }
            }
        } else {
            error!("failed to read existing users");
        }
        // start permanent listener that streams data to remote client
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            debug!("start streaming users to {}", user_id);
            if !existing.is_empty() {
                // send existing users
                let start_update = UpdateUsers {
                    added: existing,
                    gone: Vec::new(),
                };
                debug!(
                    "sending {} existing users to {}",
                    start_update.added.len(),
                    user_id
                );
                if let Err(e) = tx.send(Ok(start_update)).await {
                    error!("failed sending existing users: {}", e);
                }
            }
            // re-translate new users
            let mut notifier = notifier;
            while let Some(user) = notifier.recv().await {
                debug!("re-translating new user to {}", user_id);
                let update = UpdateUsers {
                    added: vec![user.deref().clone()],
                    gone: Vec::new(),
                };
                if let Err(e) = tx.send(Ok(update)).await {
                    error!("failed sending users update: {}, stop", e);
                    break;
                }
            }
            debug!("stream of users to {} has stopped", user_id);
        });
        // start streaming activity, data consumer
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = "Server streaming response type for the GetChats method."]
    type GetChatsStream =
        Pin<Box<dyn Stream<Item = Result<UpdateChats, tonic::Status>> + Send + Sync + 'static>>;

    #[doc = " Asks for chats list"]
    async fn get_chats(
        &self,
        request: tonic::Request<Registration>,
    ) -> Result<tonic::Response<Self::GetChatsStream>, tonic::Status> {
        debug!("get_chats(): {:?}", &request);
        let user_id = request.into_inner().user_id;
        let (listener, notifier) = mpsc::channel::<Arc<Chat>>(4);
        if let Ok(mut listeners) = self.chats_listeners.write() {
            // test alive
            listeners.retain(|k, v| {
                if v.is_closed() {
                    debug!("stop streaming chats to {}", k);
                    false
                } else {
                    true
                }
            });
            // add new
            listeners.insert(user_id, listener);
        } else {
            // failed locking listeners
            return Err(tonic::Status::internal("no access to chat listeners"));
        }
        // collect existing chats
        let mut existing = Vec::new();
        if let Ok(chats) = self.read_all_from_db::<proto::Chat>(BUCKET_CHATS) {
            for chat in chats {
                existing.push(chat.clone());
            }
        } else {
            error!("failed to read existing chats");
        }
        // start permanent listener that streams data to remote client
        let (tx, rx) = mpsc::channel(4);
        tokio::spawn(async move {
            debug!("start streaming chats to {}", user_id);
            if !existing.is_empty() {
                // send existing chats
                let start_update = UpdateChats {
                    added: existing,
                    gone: Vec::new(),
                };
                debug!(
                    "sending {} existing chats to {}",
                    start_update.added.len(),
                    user_id
                );
                if let Err(e) = tx.send(Ok(start_update)).await {
                    error!("failed sending existing chats: {}", e);
                }
            }
            // re-translate new chats
            let mut notifier = notifier;
            while let Some(chat) = notifier.recv().await {
                debug!("re-translating new chat to {}", user_id);
                let update = UpdateChats {
                    added: vec![(*chat).clone()],
                    gone: Vec::new(),
                };
                if let Err(e) = tx.send(Ok(update)).await {
                    error!("failed sending chat: {}", e);
                    break;
                }
            }
            debug!("stop streaming chats to {}", user_id);
        });
        // start streaming activity, data consumer
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    #[doc = " Creates new post"]
    async fn create_post(
        &self,
        request: tonic::Request<Post>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("create_post(): {:?}", &request);
        let mut post = request.into_inner();
        if post.id != NOT_POST_ID {
            return Err(tonic::Status::invalid_argument(format!(
                "id must be {}",
                NOT_POST_ID
            )));
        }
        post.id = new_post_id();
        self.notify_new_post(post).await;
        Ok(Response::new(RpcResult {
            ok: true,
            description: String::from("accepted"),
        }))
    }

    #[doc = " Creates new chat"]
    async fn create_chat(
        &self,
        request: tonic::Request<ChatInfo>,
    ) -> Result<tonic::Response<Chat>, tonic::Status> {
        debug!("create_chat(): {:?}", &request);
        let info = request.get_ref();
        let users = if info.auto_enter {
            // filter out duplicated users
            let mut tmp = HashSet::with_capacity(info.desired_users.len() + 1);
            tmp.insert(info.user_id);
            for u in &info.desired_users {
                tmp.insert(*u);
            }
            tmp.drain().collect()
        } else {
            Vec::new()
        };
        let id = get_chat_id(&info.description, &users);
        // test chat exists and enter the chat if that has not been done before
        match self.update_in_db::<proto::Chat, _>(BUCKET_CHATS, &id.to_le_bytes(), |chat| {
            if info.auto_enter && !chat.users.contains(&info.user_id) {
                chat.users.push(info.user_id);
            }
        }) {
            Ok(Some(chat)) => {
                // chat was found & updated if needed
                self.notify_chat_updated(chat.clone()).await;
                Ok(Response::new(chat))
            }
            Ok(None) => {
                // chat was not found, add new
                let chat = proto::Chat {
                    id,
                    permanent: info.permanent,
                    description: info.description.clone(),
                    users,
                };
                if let Err(e) = self.write_to_db(BUCKET_CHATS, &id.to_le_bytes(), &chat) {
                    Err(tonic::Status::internal(format!(
                        "failed to create chat, {}",
                        e
                    )))
                } else {
                    self.notify_chat_updated(chat.clone()).await;
                    Ok(Response::new(chat))
                }
            }
            Err(e) => Err(tonic::Status::internal(format!(
                "failed to access chats, {}",
                e
            ))),
        }
    }

    #[doc = " Invites user to chat"]
    async fn invite_user(
        &self,
        request: tonic::Request<Invitation>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("invite_user(): {:?}", &request);
        let invitation = request.into_inner();
        // test chat exists
        match self.read_from_db::<proto::Chat>(BUCKET_CHATS, &invitation.chat_id.to_le_bytes()) {
            Err(e) => return Err(tonic::Status::internal(format!("failed read chats, {}", e))),
            Ok(None) => {
                return Err(tonic::Status::not_found(format!(
                    "chat {} does not exist",
                    invitation.chat_id
                )))
            }
            _ => {}
        }
        // test recepient exists
        match self.read_from_db::<proto::User>(BUCKET_USERS, &invitation.to_user_id.to_le_bytes()) {
            Err(e) => return Err(tonic::Status::internal(format!("{}", e))),
            Ok(opt) => {
                if opt.is_none() {
                    return Err(tonic::Status::not_found(format!(
                        "user {} is not registered",
                        invitation.to_user_id
                    )));
                }
            }
        }
        // try to get send channel and send invitation
        let tx = if let Ok(listeners) = self.invitations_listeners.read() {
            if let Some(tx) = listeners.get(&invitation.to_user_id) {
                tx.clone()
            } else {
                return Err(tonic::Status::not_found(format!(
                    "{} did not subscribe to invitations",
                    invitation.to_user_id
                )));
            }
        } else {
            return Err(tonic::Status::internal(
                "failed read invitation subscribers",
            ));
        };
        if let Err(e) = tx.send(invitation).await {
            error!("failed to send invitation: {}", e);
            Err(tonic::Status::internal("failed to send invitation"))
        } else {
            Ok(Response::new(RpcResult {
                ok: true,
                description: "invitation has been sent".to_string(),
            }))
        }
    }

    #[doc = " Enters the chat"]
    async fn enter_chat(
        &self,
        request: tonic::Request<ChatReference>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("enter_chat(): {:?}", &request);
        let chat_ref = request.into_inner();
        match self.update_in_db::<proto::Chat, _>(
            BUCKET_CHATS,
            &chat_ref.chat_id.to_le_bytes(),
            |chat| {
                if !chat.users.contains(&chat_ref.user_id) {
                    chat.users.push(chat_ref.user_id);
                }
            },
        ) {
            Ok(Some(chat)) => {
                self.notify_chat_updated(chat).await;
                Ok(Response::new(RpcResult {
                    ok: true,
                    description: String::from("entered the chat"),
                }))
            }
            Ok(None) => Err(tonic::Status::not_found("chat does not exist")),
            Err(e) => Err(tonic::Status::internal(format!(
                "failed access chats, {}",
                e
            ))),
        }
    }

    #[doc = " Leaves active chat"]
    async fn leave_chat(
        &self,
        request: tonic::Request<ChatReference>,
    ) -> Result<tonic::Response<RpcResult>, tonic::Status> {
        debug!("leave_chat(): {:?}", &request);
        let chat_ref = request.into_inner();
        let updated_chat = match self.update_in_db::<proto::Chat, _>(
            BUCKET_CHATS,
            &chat_ref.chat_id.to_le_bytes(),
            |chat| {
                if chat.users.contains(&chat_ref.user_id) {
                    chat.users.retain(|&id| id != chat_ref.user_id);
                }
            },
        ) {
            Ok(Some(chat)) => {
                self.notify_chat_updated(chat.clone()).await;
                chat
            }
            Ok(None) => return Err(tonic::Status::not_found("chat does not exist")),
            Err(e) => {
                return Err(tonic::Status::internal(format!(
                    "failed access chats, {}",
                    e
                )))
            }
        };
        if !updated_chat.permanent && updated_chat.users.is_empty() {
            //remove chat
            if let Err(e) =
                self.remove_from_db::<proto::Chat>(BUCKET_CHATS, &chat_ref.chat_id.to_le_bytes())
            {
                error!("internal, {}", e);
            }
            self.notify_chat_closed(chat_ref.chat_id).await;
        } else {
            self.notify_chat_updated(updated_chat).await;
        }
        Ok(Response::new(RpcResult {
            ok: true,
            description: String::from("entered the chat"),
        }))
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
